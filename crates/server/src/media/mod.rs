//! The media server as the chat server uses it (ADR 0508): it hands out join tokens, lists
//! rooms and participants, removes a participant and changes what a participant may publish.
//! Media never passes through the chat server.

pub mod fake;
pub mod livekit;
pub mod token;
pub mod twirp;

use std::collections::BTreeSet;
use std::fmt;

use futures_util::future::BoxFuture;
use junto_protocol::ids::{AccountId, ServerId};
use junto_protocol::secret::CallToken;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

use crate::secrets::{RandomnessUnavailable, random_bytes};

const ROOM_PREFIX: &str = "jr";
const NAMESPACE_HEX: usize = 12;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The part of every room name of one server. It is derived from the identifier of the
/// server, so that a server tells its own rooms from the rooms of another program on the
/// same media server.
pub fn namespace(server_id: ServerId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"junto:rooms:");
    hasher.update(server_id.as_uuid().as_bytes());
    hex(&hasher.finalize()[..NAMESPACE_HEX / 2])
}

/// The name of the room of one call: `jr-<namespace>-<32 random hexadecimal digits>`. It is
/// random for each call and names no channel, no workspace and no account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoomName(String);

impl RoomName {
    pub fn generate(namespace: &str) -> Result<Self, RandomnessUnavailable> {
        let random = random_bytes::<16>()?;
        Ok(Self(format!("{ROOM_PREFIX}-{namespace}-{}", hex(&random))))
    }

    pub fn from_stored(name: String) -> Self {
        Self(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `name` is a room name of the server with `namespace`.
    pub fn is_of(namespace: &str, name: &str) -> bool {
        name.strip_prefix(ROOM_PREFIX)
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.strip_prefix(namespace))
            .is_some_and(|rest| rest.starts_with('-'))
    }
}

/// A random seat: the part of an identity that changes with every new place in a call.
pub fn seat() -> Result<String, RandomnessUnavailable> {
    Ok(hex(&random_bytes::<16>()?))
}

/// The identity of an account in the room of one call: the account and a seat. A client
/// reads the account from it; the seat changes when the call moves to another device.
pub fn identity(account_id: AccountId, seat: &str) -> String {
    format!("{account_id}:{seat}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrackSource {
    Camera,
    Microphone,
    ScreenShare,
    ScreenShareAudio,
    Unknown,
}

impl TrackSource {
    /// The name in a join token.
    pub const fn token_name(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::ScreenShare => "screen_share",
            Self::ScreenShareAudio => "screen_share_audio",
            Self::Unknown => "unknown",
        }
    }

    /// The name in a request to the API of the media server.
    pub const fn api_name(self) -> &'static str {
        match self {
            Self::Camera => "CAMERA",
            Self::Microphone => "MICROPHONE",
            Self::ScreenShare => "SCREEN_SHARE",
            Self::ScreenShareAudio => "SCREEN_SHARE_AUDIO",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub const fn is_audio(self) -> bool {
        matches!(self, Self::Microphone | Self::ScreenShareAudio)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Audio,
    Video,
    Data,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub kind: TrackKind,
    pub source: TrackSource,
    pub muted: bool,
    /// The identifier of the track on the media server. A track that is published again has
    /// a new one (ADR 0601).
    pub id: String,
}

/// The sources that a participant may publish. The list is always explicit, so a track of an
/// unknown source is never accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publishing {
    pub sources: BTreeSet<TrackSource>,
}

impl Publishing {
    /// Microphone, camera, screen and the sound of a screen share.
    pub fn open() -> Self {
        Self {
            sources: BTreeSet::from([
                TrackSource::Camera,
                TrackSource::Microphone,
                TrackSource::ScreenShare,
                TrackSource::ScreenShareAudio,
            ]),
        }
    }

    /// What a person with a microphone block may publish: pictures only. The sound of a
    /// screen share is closed as well, because the server cannot tell it from a voice
    /// (ADR 0510).
    pub fn pictures_only() -> Self {
        Self {
            sources: BTreeSet::from([TrackSource::Camera, TrackSource::ScreenShare]),
        }
    }

    pub fn allows(&self, source: TrackSource) -> bool {
        self.sources.contains(&source)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantKind {
    /// A client of a person.
    Standard,
    /// A participant of the recorder. The admission check skips it (ADR 0506).
    Recorder,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub identity: String,
    pub kind: ParticipantKind,
    /// False for a participant that the media server lists as disconnected.
    pub connected: bool,
    /// Absent when the media server states no list of sources.
    pub publishing: Option<Publishing>,
    pub tracks: Vec<Track>,
}

impl Participant {
    pub fn publishes(&self, source: TrackSource) -> bool {
        self.tracks.iter().any(|track| track.source == source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub name: String,
}

pub struct JoinGrant<'a> {
    pub room: &'a RoomName,
    pub identity: &'a str,
    /// The display name of the account. The layout page of a recording shows it (ADR 0615).
    pub name: &'a str,
    pub publishing: &'a Publishing,
    pub now: OffsetDateTime,
    pub lifetime: Duration,
}

pub struct JoinToken {
    pub token: CallToken,
    pub expires_at: OffsetDateTime,
}

/// A notification of the media server whose signature was verified. It only makes the next
/// admission check run at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub event: String,
    pub room: Option<String>,
    /// The time until which the media server's signature of the notification is accepted.
    pub valid_until: OffsetDateTime,
    /// What tells this notification from every other one: a hash of its body and of the end
    /// of its signature. The same notification in another spelling of its header has the
    /// same key.
    pub key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// The media server did not answer, or answered that it cannot serve.
    Unreachable(String),
    /// The media server refused the request.
    Refused(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(reason) => {
                write!(formatter, "the media server is unreachable: {reason}")
            }
            Self::Refused(reason) => {
                write!(formatter, "the media server refused a request: {reason}")
            }
        }
    }
}

impl std::error::Error for MediaError {}

pub type MediaResult<T> = Result<T, MediaError>;

/// What the server needs of a media server. The product uses [`livekit::LiveKit`]; tests
/// use [`fake::FakeMedia`].
pub trait MediaServer: Send + Sync + 'static {
    /// The address that clients connect to.
    fn client_url(&self) -> &str;

    fn join_token(&self, grant: &JoinGrant<'_>) -> MediaResult<JoinToken>;

    fn list_rooms(&self) -> BoxFuture<'_, MediaResult<Vec<Room>>>;

    /// The participants of a room. A room that does not exist has none.
    fn list_participants<'a>(
        &'a self,
        room: &'a str,
    ) -> BoxFuture<'a, MediaResult<Vec<Participant>>>;

    /// Ends the connection of a participant. A participant that is not there is no error.
    /// The removal does not end the validity of a token (ADR 0506).
    fn remove_participant<'a>(
        &'a self,
        room: &'a str,
        identity: &'a str,
    ) -> BoxFuture<'a, MediaResult<()>>;

    /// Replaces what a participant may publish. The media server ends the tracks that the
    /// new permission no longer covers.
    fn update_publishing<'a>(
        &'a self,
        room: &'a str,
        identity: &'a str,
        publishing: &'a Publishing,
    ) -> BoxFuture<'a, MediaResult<()>>;

    /// Verifies the signature of a notification. `None` for a notification that the media
    /// server did not sign for exactly this body.
    fn verify_notification(
        &self,
        authorization: &str,
        body: &[u8],
        now: OffsetDateTime,
    ) -> Option<Notification>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_name_is_random_and_names_only_its_server() {
        let server_id = ServerId::generate();
        let namespace = namespace(server_id);
        assert_eq!(namespace.len(), 12);
        assert_eq!(namespace, super::namespace(server_id));
        assert_ne!(namespace, super::namespace(ServerId::generate()));
        assert!(!namespace.contains(&server_id.to_string()[..8]));

        let first = RoomName::generate(&namespace).unwrap();
        let second = RoomName::generate(&namespace).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), 2 + 1 + 12 + 1 + 32);
        assert!(RoomName::is_of(&namespace, first.as_str()));
        let other = super::namespace(ServerId::generate());
        assert!(!RoomName::is_of(&other, first.as_str()));
        assert!(!RoomName::is_of(&namespace, "lobby"));
        assert!(!RoomName::is_of(&namespace, &format!("jr-{namespace}")));
        assert!(!RoomName::is_of(&namespace[..6], first.as_str()));
    }

    #[test]
    fn an_identity_names_its_account_and_a_random_seat() {
        let account_id = AccountId::generate();
        let first = identity(account_id, &seat().unwrap());
        let second = identity(account_id, &seat().unwrap());
        assert_ne!(first, second);
        let (account, seat) = first.split_once(':').unwrap();
        assert_eq!(account, account_id.to_string());
        assert_eq!(seat.len(), 32);
    }

    #[test]
    fn a_microphone_block_closes_every_sound_source() {
        let blocked = Publishing::pictures_only();
        for source in [
            TrackSource::Microphone,
            TrackSource::ScreenShareAudio,
            TrackSource::Unknown,
        ] {
            assert!(!blocked.allows(source), "{source:?}");
        }
        assert!(blocked.allows(TrackSource::Camera) && blocked.allows(TrackSource::ScreenShare));
        let open = Publishing::open();
        assert!(open.allows(TrackSource::Microphone) && !open.allows(TrackSource::Unknown));
    }
}
