//! Which tracks of this device each participant of the room receives (ADR 0510, ADR 0737).
//! A client from before several shares keys the share of a person by the person, so two
//! shares of one person would arrive there as one share with two pictures and two sounds.
//! While this device has two or more share tracks or shares on the way to the media server,
//! the permissions are lists: a participant whose token states that its client shows several
//! shares receives every track, and every other participant receives only the tracks that its
//! list names: the tracks of this device that are not share tracks, and the tracks of the
//! share of the lowest slot that runs. A track that is published later reaches such a
//! participant only after the engine has added it to the list. Otherwise every participant
//! receives every track.

use std::collections::HashMap;

use junto_client_core::protocol::call::SHOWN_SHARES_ATTRIBUTE;
use livekit::participant::{ParticipantKind, ParticipantTrackPermission};
use livekit::prelude::{
    LocalParticipant, ParticipantIdentity, RemoteParticipant, TrackSid, TrackSource,
};
use tokio::sync::{mpsc, watch};

use super::{Actor, Internal, Publication};

/// What the participants may receive: absent while every participant receives every track.
/// Otherwise each participant of the room with the tracks that it receives, absent for all.
pub(super) type Permissions = Option<Vec<(String, Option<Vec<TrackSid>>)>>;

/// A participant of the room as the permissions see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Viewer {
    pub identity: String,
    /// The client of the participant shows several shares of one person: its token states
    /// it, or the participant is a recorder of the media server.
    pub shows_several: bool,
}

impl Viewer {
    fn of(participant: &RemoteParticipant) -> Self {
        let stated = participant
            .attributes()
            .get(SHOWN_SHARES_ATTRIBUTE)
            .and_then(|shown| shown.parse::<u8>().ok())
            .is_some_and(|shown| shown > 1);
        Self {
            identity: participant.identity().as_str().to_owned(),
            shows_several: stated || participant.kind() == ParticipantKind::Egress,
        }
    }
}

/// A share of this device whose picture track is published or being published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShareTracks {
    pub slot: u8,
    /// The picture track, once its publication has answered.
    pub picture: Option<TrackSid>,
    /// The sound track, once its publication has answered.
    pub sound: Option<TrackSid>,
}

/// A track that the call library publishes for this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OwnTrack {
    pub sid: TrackSid,
    /// The track is the picture or the sound of a share.
    pub of_share: bool,
}

/// The permissions for the tracks of this device. `shares`: the own shares whose picture
/// track is published or being published. `arriving`: the share tracks that no share uses
/// any more and whose publication has not answered. `own`: every track that the call library
/// publishes; a share track among them that no share holds also counts.
pub(super) fn permissions(
    shares: &[ShareTracks],
    arriving: usize,
    own: &[OwnTrack],
    viewers: &[Viewer],
) -> Permissions {
    let tracks = |share: &ShareTracks| -> Vec<TrackSid> {
        share.picture.iter().chain(&share.sound).cloned().collect()
    };
    let placed: Vec<TrackSid> = shares.iter().flat_map(tracks).collect();
    let unplaced = own
        .iter()
        .filter(|track| track.of_share && !placed.contains(&track.sid))
        .count();
    if shares.len() + arriving + unplaced < 2 {
        return None;
    }
    let shown = shares
        .iter()
        .filter(|share| share.picture.is_some())
        .min_by_key(|share| share.slot)
        .map(tracks)
        .unwrap_or_default();
    let mut one_share: Vec<TrackSid> = own
        .iter()
        .filter(|track| !track.of_share || shown.contains(&track.sid))
        .map(|track| track.sid.clone())
        .collect();
    one_share.sort();
    let mut list: Vec<(String, Option<Vec<TrackSid>>)> = viewers
        .iter()
        .map(|viewer| {
            let allowed = (!viewer.shows_several).then(|| one_share.clone());
            (viewer.identity.clone(), allowed)
        })
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    Some(list)
}

/// Applies the permissions that the engine sends, one after the other, the latest of those
/// that wait first, until the engine drops the sender with the room. Each list has a
/// number; after the call library has taken a list, the engine is told its number for the
/// connection `attempt`, and the lists before it count as taken. While `held` holds
/// `true`, nothing is applied: a test holds the permissions in this way.
pub(super) async fn apply(
    participant: LocalParticipant,
    mut wanted: mpsc::UnboundedReceiver<(u64, Permissions)>,
    mut held: watch::Receiver<bool>,
    internal: mpsc::UnboundedSender<Internal>,
    attempt: u64,
) {
    while let Some(mut latest) = wanted.recv().await {
        if held.wait_for(|held| !*held).await.is_err() {
            return;
        }
        while let Ok(newer) = wanted.try_recv() {
            latest = newer;
        }
        let (number, latest) = latest;
        let result = match latest {
            None => {
                participant
                    .set_track_subscription_permissions(true, Vec::new())
                    .await
            }
            Some(list) => {
                let permissions = list
                    .into_iter()
                    .map(|(identity, allowed)| ParticipantTrackPermission {
                        participant_identity: ParticipantIdentity(identity),
                        allow_all: allowed.is_none(),
                        allowed_track_sids: allowed.unwrap_or_default(),
                    })
                    .collect();
                participant
                    .set_track_subscription_permissions(false, permissions)
                    .await
            }
        };
        if let Err(error) = &result {
            tracing::warn!(%error, "the track permissions of the room were not set");
        }
        let result = result.map_err(|error| error.to_string());
        let _ = internal.send(Internal::PermissionsApplied(attempt, number, result));
    }
}

impl Actor {
    /// Starts the work that applies the permissions of the room that was connected.
    pub(super) fn start_permissions(&mut self) {
        let Some(room) = &self.room else {
            return;
        };
        let (sender, taken) = mpsc::unbounded_channel();
        self.runtime.spawn(apply(
            room.local_participant(),
            taken,
            self.permissions_held.subscribe(),
            self.internal.clone(),
            self.attempt,
        ));
        self.permissions = Some(sender);
        self.permissions_told = None;
        (self.permissions_sent, self.permissions_applied) = (0, 0);
        self.sync_permissions();
    }

    /// The call library took the permissions with the number `number`, or failed to.
    pub(super) fn permissions_applied(
        &mut self,
        attempt: u64,
        number: u64,
        result: Result<(), String>,
    ) {
        if attempt != self.attempt || self.permissions.is_none() {
            return;
        }
        match result {
            Ok(()) => {
                self.permissions_applied = self.permissions_applied.max(number);
                self.share_permissions_applied(self.permissions_applied, None);
            }
            Err(error) => {
                // They are sent again at the next change.
                self.permissions_told = None;
                self.share_permissions_applied(number, Some(error));
            }
        }
    }

    /// Sends the permissions of now when they differ from the ones sent last. Returns the
    /// number of the permissions of now: those sent now or before. Absent without a
    /// room, or when the work that applies them has ended.
    pub(super) fn sync_permissions(&mut self) -> Option<u64> {
        let (Some(room), Some(sender)) = (&self.room, &self.permissions) else {
            return None;
        };
        let shares: Vec<ShareTracks> = self
            .own_shares
            .iter()
            .filter_map(|(slot, share)| {
                let picture = match &share.picture {
                    Publication::None => return None,
                    Publication::Publishing => None,
                    Publication::Published(picture) => Some(picture.sid()),
                };
                let sound = match &share.sound {
                    Publication::Published(sound) => Some(sound.sid()),
                    Publication::None | Publication::Publishing => None,
                };
                Some(ShareTracks {
                    slot: *slot,
                    picture,
                    sound,
                })
            })
            .collect();
        let own: Vec<OwnTrack> = room
            .local_participant()
            .track_publications()
            .into_iter()
            .map(|(sid, publication)| OwnTrack {
                sid,
                of_share: matches!(
                    publication.source(),
                    TrackSource::Screenshare | TrackSource::ScreenshareAudio
                ),
            })
            .collect();
        let participants: HashMap<ParticipantIdentity, RemoteParticipant> =
            room.remote_participants();
        let viewers: Vec<Viewer> = participants.values().map(Viewer::of).collect();
        let wanted = permissions(&shares, self.arriving.len(), &own, &viewers);
        if self.permissions_told.as_ref() == Some(&wanted) {
            return Some(self.permissions_sent);
        }
        let number = self.permissions_sent + 1;
        sender.send((number, wanted.clone())).ok()?;
        self.permissions_sent = number;
        self.permissions_told = Some(wanted);
        Some(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(name: &str) -> TrackSid {
        TrackSid::try_from(format!("TR_{name}")).unwrap()
    }

    fn viewer(identity: &str, shows_several: bool) -> Viewer {
        Viewer {
            identity: identity.to_owned(),
            shows_several,
        }
    }

    fn share(slot: u8, picture: Option<&str>, sound: Option<&str>) -> ShareTracks {
        ShareTracks {
            slot,
            picture: picture.map(sid),
            sound: sound.map(sid),
        }
    }

    fn own(other: &[&str], of_shares: &[&str]) -> Vec<OwnTrack> {
        let track = |name: &&str, of_share| OwnTrack {
            sid: sid(name),
            of_share,
        };
        other
            .iter()
            .map(|name| track(name, false))
            .chain(of_shares.iter().map(|name| track(name, true)))
            .collect()
    }

    fn sorted(names: &[&str]) -> Vec<TrackSid> {
        let mut sids: Vec<TrackSid> = names.iter().map(|name| sid(name)).collect();
        sids.sort();
        sids
    }

    #[test]
    fn every_participant_receives_every_track_while_one_share_runs() {
        let own = own(&["mic"], &["screen0", "sound0"]);
        let viewers = [viewer("old", false), viewer("new", true)];
        assert_eq!(permissions(&[], 0, &[], &viewers), None);
        let one = [share(1, Some("screen0"), Some("sound0"))];
        assert_eq!(permissions(&one, 0, &own, &viewers), None);
    }

    #[test]
    fn a_client_of_one_share_receives_the_share_of_the_lowest_slot_and_every_other_track() {
        let own = own(&["mic", "camera"], &["screen1", "sound1", "screen2"]);
        let shares = [
            share(2, Some("screen2"), None),
            share(1, Some("screen1"), Some("sound1")),
        ];
        let viewers = [
            viewer("old", false),
            viewer("new", true),
            viewer("other old", false),
        ];
        let expected = sorted(&["camera", "mic", "screen1", "sound1"]);
        assert_eq!(
            permissions(&shares, 0, &own, &viewers),
            Some(vec![
                ("new".to_owned(), None),
                ("old".to_owned(), Some(expected.clone())),
                ("other old".to_owned(), Some(expected)),
            ])
        );
        // Without a participant the list is empty: a participant who comes in receives
        // nothing until the permissions of its arrival are sent.
        assert_eq!(permissions(&shares, 0, &own, &[]), Some(Vec::new()));
    }

    #[test]
    fn a_share_whose_picture_is_being_published_counts_toward_two_shares() {
        let own = own(&["mic"], &["screen0", "sound0"]);
        let shares = [
            share(0, Some("screen0"), Some("sound0")),
            share(1, None, None),
        ];
        let viewers = [viewer("old", false), viewer("new", true)];
        assert_eq!(
            permissions(&shares, 0, &own, &viewers),
            Some(vec![
                ("new".to_owned(), None),
                (
                    "old".to_owned(),
                    Some(sorted(&["mic", "screen0", "sound0"]))
                ),
            ])
        );
        // The share in the lower slot whose picture is being published is not yet the
        // share that a client of one share receives: the share that runs stays.
        let shares = [
            share(0, None, None),
            share(1, Some("screen1"), Some("sound1")),
        ];
        let own = self::own(&["mic"], &["screen1", "sound1"]);
        assert_eq!(
            permissions(&shares, 0, &own, &[viewer("old", false)]),
            Some(vec![(
                "old".to_owned(),
                Some(sorted(&["mic", "screen1", "sound1"]))
            )])
        );
    }

    #[test]
    fn a_client_of_one_share_never_receives_a_share_track_that_the_engine_has_not_placed() {
        // The call library publishes the sound of the share in slot 1, and its answer has
        // not reached the engine; the call library also published a track of the share in
        // slot 0 again under a new identifier, which the engine has not taken yet.
        let own = own(
            &["mic", "camera"],
            &["screen0", "sound0", "screen1", "sound1", "screen0new"],
        );
        let shares = [
            share(0, Some("screen0"), Some("sound0")),
            share(1, Some("screen1"), None),
        ];
        let viewers = [viewer("old", false), viewer("new", true)];
        assert_eq!(
            permissions(&shares, 0, &own, &viewers),
            Some(vec![
                ("new".to_owned(), None),
                (
                    "old".to_owned(),
                    Some(sorted(&["camera", "mic", "screen0", "sound0"]))
                ),
            ])
        );
    }

    #[test]
    fn the_tracks_of_an_ended_share_count_until_they_are_gone() {
        let viewers = [viewer("old", false)];
        let running = [share(1, Some("screen1"), Some("sound1"))];
        let allowed = Some(vec![(
            "old".to_owned(),
            Some(sorted(&["mic", "screen1", "sound1"])),
        )]);
        // The picture of the ended share in slot 0 is still published.
        let own_before = own(&["mic"], &["screen0", "screen1", "sound1"]);
        assert_eq!(permissions(&running, 0, &own_before, &viewers), allowed);
        // The publication of its picture has not answered.
        let own_after = own(&["mic"], &["screen1", "sound1"]);
        assert_eq!(permissions(&running, 1, &own_after, &viewers), allowed);
        assert_eq!(permissions(&running, 0, &own_after, &viewers), None);
    }
}
