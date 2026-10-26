//! Which tracks of this device each participant of the room receives (ADR 0510). A client
//! from before several shares keys the share of a person by the person, so two shares of
//! one person would arrive there as one share with two pictures and two sounds. While this
//! device runs two or more shares, a participant whose token does not state that its client
//! shows several shares receives the shares of the lowest slot only, with every other track
//! of this device. Otherwise every participant receives every track.

use std::collections::HashMap;

use junto_client_core::protocol::call::SHOWN_SHARES_ATTRIBUTE;
use livekit::participant::{ParticipantKind, ParticipantTrackPermission};
use livekit::prelude::{LocalParticipant, ParticipantIdentity, RemoteParticipant, TrackSid};
use tokio::sync::mpsc;

use super::{Actor, Publication};

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

/// The permissions for the tracks of this device. `shares`: the tracks of each own share
/// whose picture track is published, by slot. `own`: every track that this device
/// publishes.
pub(super) fn permissions(
    shares: &[(u8, Vec<TrackSid>)],
    own: &[TrackSid],
    viewers: &[Viewer],
) -> Permissions {
    if shares.len() < 2 {
        return None;
    }
    let lowest = shares.iter().map(|(slot, _)| *slot).min()?;
    let withheld: Vec<&TrackSid> = shares
        .iter()
        .filter(|(slot, _)| *slot != lowest)
        .flat_map(|(_, tracks)| tracks)
        .collect();
    let mut one_share: Vec<TrackSid> = own
        .iter()
        .filter(|sid| !withheld.contains(sid))
        .cloned()
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
/// that wait first, until the engine drops the sender with the room.
pub(super) async fn apply(
    participant: LocalParticipant,
    mut wanted: mpsc::UnboundedReceiver<Permissions>,
) {
    while let Some(mut latest) = wanted.recv().await {
        while let Ok(newer) = wanted.try_recv() {
            latest = newer;
        }
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
        if let Err(error) = result {
            tracing::warn!(%error, "the track permissions of the room were not set");
        }
    }
}

impl Actor {
    /// Starts the work that applies the permissions of the room that was connected.
    pub(super) fn start_permissions(&mut self) {
        let Some(room) = &self.room else {
            return;
        };
        let (sender, taken) = mpsc::unbounded_channel();
        self.runtime.spawn(apply(room.local_participant(), taken));
        self.permissions = Some(sender);
        self.permissions_told = None;
        self.sync_permissions();
    }

    /// Sends the permissions of now when they differ from the ones sent last.
    pub(super) fn sync_permissions(&mut self) {
        let (Some(room), Some(sender)) = (&self.room, &self.permissions) else {
            return;
        };
        let shares: Vec<(u8, Vec<TrackSid>)> = self
            .own_shares
            .iter()
            .filter_map(|(slot, share)| match &share.picture {
                Publication::Published(picture) => {
                    let mut tracks = vec![picture.sid()];
                    if let Publication::Published(sound) = &share.sound {
                        tracks.push(sound.sid());
                    }
                    Some((*slot, tracks))
                }
                Publication::None | Publication::Publishing => None,
            })
            .collect();
        let own: Vec<TrackSid> = room
            .local_participant()
            .track_publications()
            .into_keys()
            .collect();
        let participants: HashMap<ParticipantIdentity, RemoteParticipant> =
            room.remote_participants();
        let viewers: Vec<Viewer> = participants.values().map(Viewer::of).collect();
        let wanted = permissions(&shares, &own, &viewers);
        if self.permissions_told.as_ref() == Some(&wanted) {
            return;
        }
        if sender.send(wanted.clone()).is_ok() {
            self.permissions_told = Some(wanted);
        }
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

    #[test]
    fn every_participant_receives_every_track_while_one_share_runs() {
        let own = [sid("mic"), sid("screen0"), sid("sound0")];
        let viewers = [viewer("old", false), viewer("new", true)];
        assert_eq!(permissions(&[], &own, &viewers), None);
        let one = [(1, vec![sid("screen0"), sid("sound0")])];
        assert_eq!(permissions(&one, &own, &viewers), None);
    }

    #[test]
    fn a_client_of_one_share_receives_the_share_of_the_lowest_slot_and_every_other_track() {
        let own = [
            sid("mic"),
            sid("camera"),
            sid("screen1"),
            sid("sound1"),
            sid("screen2"),
        ];
        let shares = [
            (2, vec![sid("screen2")]),
            (1, vec![sid("screen1"), sid("sound1")]),
        ];
        let viewers = [
            viewer("old", false),
            viewer("new", true),
            viewer("other old", false),
        ];
        let mut expected = vec![sid("camera"), sid("mic"), sid("screen1"), sid("sound1")];
        expected.sort();
        assert_eq!(
            permissions(&shares, &own, &viewers),
            Some(vec![
                ("new".to_owned(), None),
                ("old".to_owned(), Some(expected.clone())),
                ("other old".to_owned(), Some(expected)),
            ])
        );
        // Without a participant the list is empty: a participant who comes in receives
        // nothing until the permissions of its arrival are sent.
        assert_eq!(permissions(&shares, &own, &[]), Some(Vec::new()));
    }
}
