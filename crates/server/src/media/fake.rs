//! A media server in memory for tests. It behaves as step 0 measured LiveKit (ADR 0506): it
//! accepts a join token for a time after its stated end, a removal ends a connection and no
//! token, and a participant can come back with the token it holds.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use junto_protocol::secret::CallToken;
use serde_json::Value;
use time::OffsetDateTime;

use crate::clock::Clock;
use crate::media::token;
use crate::media::{
    JoinGrant, JoinToken, MediaError, MediaResult, MediaServer, Notification, Participant,
    ParticipantKind, Publishing, Room, Track, TrackKind, TrackSource,
};

pub const KEY: &str = "fake-key";
pub const SECRET: &[u8] = b"fake-secret-of-the-fake-media-server";
pub const CLIENT_URL: &str = "wss://media.example.test";

#[derive(Default)]
struct State {
    rooms: BTreeMap<String, Vec<Participant>>,
    unreachable: bool,
    failing_rooms: Vec<String>,
    stuck: Vec<String>,
    stalled: Vec<(String, std::time::Duration)>,
    requests: u64,
    /// The time that every request takes, and the requests that are answered at one time.
    delay: Option<std::time::Duration>,
    in_flight: usize,
    most_in_flight: usize,
    removed: Vec<(String, String)>,
    updated: Vec<(String, String, Publishing)>,
}

pub struct FakeMedia {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

/// Why the fake media server refused a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    InvalidToken,
    Expired,
}

const SOURCES: [TrackSource; 4] = [
    TrackSource::Camera,
    TrackSource::Microphone,
    TrackSource::ScreenShare,
    TrackSource::ScreenShareAudio,
];

impl FakeMedia {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Connects a participant with a join token, as a client does. A participant with the
    /// same identity is replaced. Returns the room and the identity of the token.
    pub fn connect(&self, token: &CallToken) -> Result<(String, String), Refusal> {
        let claims = token::verify(SECRET, token.expose()).ok_or(Refusal::InvalidToken)?;
        let text = |pointer: &str| claims.pointer(pointer).and_then(Value::as_str);
        let (Some(room), Some(identity)) = (text("/video/room"), text("/sub")) else {
            return Err(Refusal::InvalidToken);
        };
        if claims.pointer("/video/roomJoin") != Some(&Value::Bool(true)) {
            return Err(Refusal::InvalidToken);
        }
        let ends = claims
            .get("exp")
            .and_then(Value::as_i64)
            .ok_or(Refusal::InvalidToken)?;
        let now = self.clock.now().unix_timestamp();
        if now > ends.saturating_add(token::TOLERANCE.whole_seconds()) {
            return Err(Refusal::Expired);
        }
        let named: Vec<&str> = claims
            .pointer("/video/canPublishSources")
            .and_then(Value::as_array)
            .map(|sources| sources.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let publishing = Publishing {
            sources: SOURCES
                .into_iter()
                .filter(|source| named.contains(&source.token_name()))
                .collect(),
        };
        self.enter(room, identity, ParticipantKind::Standard, Some(publishing));
        Ok((room.to_owned(), identity.to_owned()))
    }

    /// Puts a participant into a room without a join token: a participant of the recorder,
    /// or a client that returns with the longer token that the media server gave it.
    pub fn enter(
        &self,
        room: &str,
        identity: &str,
        kind: ParticipantKind,
        publishing: Option<Publishing>,
    ) {
        let mut state = self.state();
        let participants = state.rooms.entry(room.to_owned()).or_default();
        participants.retain(|participant| participant.identity != identity);
        participants.push(Participant {
            identity: identity.to_owned(),
            kind,
            connected: true,
            publishing,
            tracks: Vec::new(),
        });
    }

    /// A participant leaves by itself.
    pub fn leave(&self, room: &str, identity: &str) {
        let mut state = self.state();
        if let Some(participants) = state.rooms.get_mut(room) {
            participants.retain(|participant| participant.identity != identity);
        }
    }

    /// Publishes a track. Returns false when the participant is not in the room or may not
    /// publish the source, as the media server refuses the track then.
    pub fn publish(
        &self,
        room: &str,
        identity: &str,
        kind: TrackKind,
        source: TrackSource,
    ) -> bool {
        let mut state = self.state();
        let Some(participant) = state
            .rooms
            .get_mut(room)
            .and_then(|room| room.iter_mut().find(|other| other.identity == identity))
        else {
            return false;
        };
        let allowed = participant
            .publishing
            .as_ref()
            .is_none_or(|publishing| publishing.allows(source));
        if allowed {
            participant.tracks.retain(|track| track.source != source);
            participant.tracks.push(Track {
                kind,
                source,
                muted: false,
            });
        }
        allowed
    }

    /// Publishes a track without regard to the permission: what a media server would list
    /// if it did not check the source of a track.
    pub fn force_track(&self, room: &str, identity: &str, kind: TrackKind, source: TrackSource) {
        let mut state = self.state();
        if let Some(participant) = state
            .rooms
            .get_mut(room)
            .and_then(|room| room.iter_mut().find(|other| other.identity == identity))
        {
            participant.tracks.push(Track {
                kind,
                source,
                muted: false,
            });
        }
    }

    pub fn unpublish(&self, room: &str, identity: &str, source: TrackSource) {
        let mut state = self.state();
        if let Some(participant) = state
            .rooms
            .get_mut(room)
            .and_then(|room| room.iter_mut().find(|other| other.identity == identity))
        {
            participant.tracks.retain(|track| track.source != source);
        }
    }

    pub fn participants(&self, room: &str) -> Vec<Participant> {
        self.state().rooms.get(room).cloned().unwrap_or_default()
    }

    pub fn identities(&self, room: &str) -> Vec<String> {
        self.participants(room)
            .into_iter()
            .map(|participant| participant.identity)
            .collect()
    }

    pub fn room_names(&self) -> Vec<String> {
        let state = self.state();
        state
            .rooms
            .iter()
            .filter(|(_, participants)| !participants.is_empty())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Makes every request fail, as while the media server or the path to it is down. The
    /// participants stay in their rooms.
    pub fn set_unreachable(&self, unreachable: bool) {
        self.state().unreachable = unreachable;
    }

    /// Makes every request about one room fail, while the list of rooms and the other rooms
    /// answer.
    pub fn fail_room(&self, room: &str, failing: bool) {
        let mut state = self.state();
        state.failing_rooms.retain(|other| other != room);
        if failing {
            state.failing_rooms.push(room.to_owned());
        }
    }

    /// Makes the removal of one identity fail, in whatever room.
    pub fn fail_removal_of(&self, identity: &str, failing: bool) {
        let mut state = self.state();
        state.stuck.retain(|other| other != identity);
        if failing {
            state.stuck.push(identity.to_owned());
        }
    }

    /// Makes every removal and every change of a permission of one identity take `delay`
    /// and fail then, as a request that reaches its time limit.
    pub fn stall_actions_on(&self, identity: &str, delay: Option<std::time::Duration>) {
        let mut state = self.state();
        state.stalled.retain(|(other, _)| other != identity);
        if let Some(delay) = delay {
            state.stalled.push((identity.to_owned(), delay));
        }
    }

    /// The removals that the server asked for, as room and identity, in order.
    pub fn removed(&self) -> Vec<(String, String)> {
        self.state().removed.clone()
    }

    pub fn updated(&self) -> Vec<(String, String, Publishing)> {
        self.state().updated.clone()
    }

    pub fn requests(&self) -> u64 {
        self.state().requests
    }

    /// Makes every request take `delay` from now on.
    pub fn set_delay(&self, delay: Option<std::time::Duration>) {
        self.state().delay = delay;
    }

    /// The largest number of delayed requests that were in progress at one time.
    pub fn most_concurrent(&self) -> usize {
        self.state().most_in_flight
    }

    /// Answers a request with `result`, after the delay of the fake when it has one.
    fn answered<'a, T: Send + 'a>(
        &'a self,
        result: MediaResult<T>,
    ) -> BoxFuture<'a, MediaResult<T>> {
        struct InFlight<'a>(&'a FakeMedia);
        impl Drop for InFlight<'_> {
            fn drop(&mut self) {
                self.0.state().in_flight -= 1;
            }
        }
        let delay = self.state().delay;
        async move {
            if let Some(delay) = delay {
                {
                    let mut state = self.state();
                    state.in_flight += 1;
                    state.most_in_flight = state.most_in_flight.max(state.in_flight);
                }
                let _in_flight = InFlight(self);
                tokio::time::sleep(delay).await;
            }
            result
        }
        .boxed()
    }

    /// The answer to an action on `identity` while its actions are stalled.
    fn stalled(&self, identity: &str) -> Option<BoxFuture<'_, MediaResult<()>>> {
        let mut state = self.state();
        let delay = state
            .stalled
            .iter()
            .find(|(other, _)| other == identity)
            .map(|(_, delay)| *delay)?;
        state.requests += 1;
        let answer = async move {
            tokio::time::sleep(delay).await;
            Err(MediaError::Unreachable(
                "the request reached its time limit".to_owned(),
            ))
        };
        Some(answer.boxed())
    }

    /// The header of a notification about `body`, signed as the media server signs it.
    pub fn sign_notification(&self, body: &[u8]) -> String {
        token::signed_notification(KEY, SECRET, body, self.clock.now())
    }

    fn request(&self, room: Option<&str>) -> MediaResult<MutexGuard<'_, State>> {
        let mut state = self.state();
        state.requests += 1;
        let fails =
            |failing: &[String]| room.is_some_and(|room| failing.iter().any(|other| other == room));
        if state.unreachable {
            Err(MediaError::Unreachable(
                "the fake is switched off".to_owned(),
            ))
        } else if fails(&state.failing_rooms) {
            Err(MediaError::Unreachable(
                "the room is made to fail".to_owned(),
            ))
        } else {
            Ok(state)
        }
    }
}

impl MediaServer for FakeMedia {
    fn client_url(&self) -> &str {
        CLIENT_URL
    }

    fn join_token(&self, grant: &JoinGrant<'_>) -> MediaResult<JoinToken> {
        let token = token::sign(SECRET, &token::join_claims(KEY, grant))
            .ok_or_else(|| MediaError::Refused("the join token could not be signed".to_owned()))?;
        Ok(JoinToken {
            token: CallToken::new(token),
            expires_at: grant.now + grant.lifetime,
        })
    }

    fn list_rooms(&self) -> BoxFuture<'_, MediaResult<Vec<Room>>> {
        let rooms = self.request(None).map(|state| {
            state
                .rooms
                .iter()
                .filter(|(_, participants)| !participants.is_empty())
                .map(|(name, _)| Room { name: name.clone() })
                .collect()
        });
        self.answered(rooms)
    }

    fn list_participants<'a>(
        &'a self,
        room: &'a str,
    ) -> BoxFuture<'a, MediaResult<Vec<Participant>>> {
        let participants = self
            .request(Some(room))
            .map(|state| state.rooms.get(room).cloned().unwrap_or_default());
        self.answered(participants)
    }

    fn remove_participant<'a>(
        &'a self,
        room: &'a str,
        identity: &'a str,
    ) -> BoxFuture<'a, MediaResult<()>> {
        if let Some(stalled) = self.stalled(identity) {
            return stalled;
        }
        let removed = self.request(Some(room)).and_then(|mut state| {
            if state.stuck.iter().any(|other| other == identity) {
                return Err(MediaError::Unreachable(
                    "the removal is made to fail".to_owned(),
                ));
            }
            state.removed.push((room.to_owned(), identity.to_owned()));
            if let Some(participants) = state.rooms.get_mut(room) {
                participants.retain(|participant| participant.identity != identity);
            }
            Ok(())
        });
        self.answered(removed)
    }

    fn update_publishing<'a>(
        &'a self,
        room: &'a str,
        identity: &'a str,
        publishing: &'a Publishing,
    ) -> BoxFuture<'a, MediaResult<()>> {
        if let Some(stalled) = self.stalled(identity) {
            return stalled;
        }
        let updated = self.request(Some(room)).map(|mut state| {
            state
                .updated
                .push((room.to_owned(), identity.to_owned(), publishing.clone()));
            if let Some(participant) = state
                .rooms
                .get_mut(room)
                .and_then(|room| room.iter_mut().find(|other| other.identity == identity))
            {
                participant
                    .tracks
                    .retain(|track| publishing.allows(track.source));
                participant.publishing = Some(publishing.clone());
            }
        });
        self.answered(updated)
    }

    fn verify_notification(
        &self,
        authorization: &str,
        body: &[u8],
        now: OffsetDateTime,
    ) -> Option<Notification> {
        token::notification(KEY, SECRET, authorization, body, now)
    }
}
