//! Calls in voice channels and between the two participants of a direct message (ADR 0501).
//!
//! The server decides who is in a call and hands out the tokens for the media server; media
//! never passes through it (ADR 0506, ADR 0508).

use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;

use crate::ids::{
    AccountId, CallId, CallInvitationId, CallMoveId, ChannelId, DeviceId, EntryRequestId,
    WorkspaceId,
};
use crate::recording::Recording;
use crate::secret::CallToken;

/// Where a call takes place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallPlace {
    Channel {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// The direct message of two accounts. `low_account_id` is the smaller identifier.
    Direct {
        low_account_id: AccountId,
        high_account_id: AccountId,
    },
}

impl CallPlace {
    /// The place of the direct-message call of two different accounts.
    pub fn direct(one: AccountId, other: AccountId) -> Self {
        Self::Direct {
            low_account_id: one.min(other),
            high_account_id: one.max(other),
        }
    }

    pub const fn channel_id(&self) -> Option<ChannelId> {
        match self {
            Self::Channel { channel_id, .. } => Some(*channel_id),
            Self::Direct { .. } => None,
        }
    }
}

/// The sound that a screen share sends (ADR 0502).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ShareAudio {
    /// The sound of the shared application.
    Application,
    /// All system sound.
    System,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScreenShare {
    pub audio: ShareAudio,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// A person with `mute_members` set the block.
    Moderator,
    /// Meeting mode set the block.
    Meeting,
}

/// A block of a person's microphone. The server enforces it: the person cannot send audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MicrophoneBlock {
    pub by: AccountId,
    pub reason: BlockReason,
}

/// A block of a person's sound. The client of the person applies it (ADR 0503).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SoundBlock {
    pub by: AccountId,
}

/// The share that a person watches: the person who shares it, and the slot of the share
/// among the shares of that person (ADR 0502). A client that runs one share per person
/// states slot 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WatchedShare {
    pub account_id: AccountId,
    #[serde(default)]
    pub slot: u8,
}

/// The share that the person of the device watches now, or none (ADR 0502).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ReportWatching {
    pub watching: Option<WatchedShare>,
}

/// One person in a call. A block and the person's own setting are separate: removing a block
/// returns the microphone or the sound to `muted` or `deafened`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CallParticipant {
    /// Metadata of the avatar, without its image bytes.
    #[serde(default)]
    pub avatar: Option<crate::account::Avatar>,
    pub account_id: AccountId,
    /// The identity of the person's participant in the room of the media server.
    pub identity: String,
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at: OffsetDateTime,
    /// The person's own microphone setting, as their client reports it.
    pub muted: bool,
    /// The person's own sound setting, as their client reports it.
    pub deafened: bool,
    pub microphone_block: Option<MicrophoneBlock>,
    pub sound_block: Option<SoundBlock>,
    pub hand_raised: bool,
    pub screen_share: Option<ScreenShare>,
    /// The share of another person in the call that the person watches now, as the client
    /// of the person reports it (ADR 0502). Absent: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watching: Option<WatchedShare>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CallLock {
    pub by: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Meeting mode: every microphone except the presenter's is blocked (ADR 0505).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Meeting {
    pub presenter: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
}

/// A call as an account that may see who is inside sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Call {
    pub id: CallId,
    pub place: CallPlace,
    /// When the first person was seen in the room. Absent until then.
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    pub lock: Option<CallLock>,
    pub meeting: Option<Meeting>,
    /// The call shows who is inside only to the people inside. It keeps the setting that
    /// its voice channel had when the call began; a later change of the channel applies to
    /// the next call (ADR 0517). An account that is not inside such a call yet gets it
    /// without lock, meeting, participants and recording.
    #[serde(default)]
    pub hides_occupants: bool,
    /// The people whom the media server lists in the room and whom the server allows there.
    pub participants: Vec<CallParticipant>,
    /// The recording of the call that is starting or runs. The people in the call see an
    /// indicator while it is present (ADR 0601).
    #[serde(default)]
    pub recording: Option<Recording>,
}

/// What an account that may see a voice channel learns of the call in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelCall {
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    /// True while at least one person is in the room.
    pub in_use: bool,
    /// Absent when nobody is inside, and when the channel hides who is inside and the caller
    /// is not inside (ADR 0504).
    pub call: Option<Call>,
}

/// The call that the account is in, and the device that it is in the call with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OwnCall {
    /// The retained hide setting, or unknown when an older server omitted it.
    #[serde(default)]
    pub hides_occupants: Option<bool>,
    pub call_id: CallId,
    pub place: CallPlace,
    pub device_id: DeviceId,
    pub identity: String,
}

/// What a device needs to join the room of a call. The token is valid for about one minute
/// and for the first connection only; a device that lost its connection for longer asks
/// again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CallJoin {
    /// The position in the event log up to which this answer holds the events. It is read
    /// with the call and the blocks, after the place was made (ADR 0519). A server from
    /// before that ADR sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<crate::events::Cursor>,
    pub call: Call,
    /// The address of the media server for the device.
    pub media_url: String,
    pub token: CallToken,
    #[serde(with = "time::serde::rfc3339")]
    pub token_expires_at: OffsetDateTime,
    pub identity: String,
    /// The requests to enter that wait for an answer of a person inside.
    pub entry_requests: Vec<EntryRequest>,
    /// The block of the caller's microphone in this call. The call lists the caller as a
    /// participant only after the device has connected, so the answer states the block,
    /// except before admission to a call that hides its occupants (ADR 0517).
    #[serde(default)]
    pub microphone_block: Option<MicrophoneBlock>,
    /// The block of the caller's sound in this call. The client applies it from the first
    /// moment of its connection (ADR 0503), except before admission to a call that hides
    /// its occupants (ADR 0517).
    #[serde(default)]
    pub sound_block: Option<SoundBlock>,
    /// True when a microphone block of the caller ended as a grant of meeting mode and the
    /// device has not reported an open microphone since (ADR 0521). Absent while the call
    /// hides who is inside and the caller is not inside, and from a server from before
    /// that ADR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_grant: Option<bool>,
}

/// The query of the request that leaves a call.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct LeaveCallQuery {
    /// The identity of the place that the device leaves. The call of the account ends only
    /// when the account holds its place with this identity, or with the place that a
    /// completed move gave in place of this identity. Absent: the call of the account ends,
    /// from whichever device it is in the call.
    #[serde(default)]
    pub identity: Option<String>,
}

/// The query of a request for a place in a call.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct CallJoinQuery {
    /// The identity of the seat that the request renews. The answer is a token for that
    /// seat, or `seat_ended` when the device can no longer use it; such a request makes,
    /// ends and takes no seat (ADR 0520). Absent: the request asks for a place.
    #[serde(default)]
    pub renews: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OwnCallState {
    /// True after admission has observed this seat. Absent means not stated by the server (ADR 0517).
    #[serde(default)]
    pub inside: Option<bool>,
    pub own: OwnCall,
    pub call: Call,
    pub entry_requests: Vec<EntryRequest>,
    /// As `CallJoin::meeting_grant` (ADR 0521).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_grant: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OwnCallView {
    /// The position in the event log up to which this answer holds the events (ADR 0519).
    /// A server from before that ADR sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<crate::events::Cursor>,
    /// Absent when the account is in no call.
    pub call: Option<OwnCallState>,
}

/// The device's report of the person's own microphone and sound settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReportCallState {
    pub muted: bool,
    pub deafened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReportScreenShare {
    pub audio: ShareAudio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReportSpeaking {
    pub speaking: bool,
}

/// A request of a person outside to enter a locked or full voice channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EntryRequest {
    pub id: EntryRequestId,
    pub call_id: CallId,
    pub place: CallPlace,
    pub account_id: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnswerEntryRequest {
    pub accept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EntryOutcome {
    /// The person can now join, for a short time.
    Accepted,
    Denied,
    Expired,
    Cancelled,
}

/// An invitation into a call: from a person inside a voice channel, or the ringing of a
/// direct-message call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CallInvitation {
    pub id: CallInvitationId,
    pub call_id: CallId,
    pub place: CallPlace,
    pub account_id: AccountId,
    pub invited_by: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InviteToCall {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum InvitationOutcome {
    Accepted,
    Declined,
    Expired,
    Cancelled,
}

/// A muted direct-message conversation: the account that muted it gets no call invitation
/// from the other account (ADR 0515).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DirectMute {
    /// The other account of the conversation.
    pub account_id: AccountId,
    /// The time at which the mute ends. Absent: it lasts until it is turned off.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub until: Option<OffsetDateTime>,
}

/// An absent `until` mutes the conversation until the mute is turned off.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MuteDirectConversation {
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub until: Option<OffsetDateTime>,
}

/// A move of a person from one voice channel to another. The person's device joins the
/// destination first; the server takes the person out of the source when it has seen that
/// join (ADR 0511).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CallMove {
    pub id: CallMoveId,
    pub account_id: AccountId,
    pub from: CallPlace,
    pub to: CallPlace,
    pub moved_by: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MoveParticipant {
    pub to_channel_id: ChannelId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MoveOutcome {
    /// The person is in the destination and no longer in the source.
    Completed,
    /// The device of the person did not join the destination in time. Nothing changed.
    TimedOut,
    /// The device of the person reported that it could not join. Nothing changed.
    Refused,
    /// The person left the source or lost the access to one of the two channels, or the
    /// person who moved lost the permission, before the move completed.
    Abandoned,
}

/// Why a person is no longer in a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LeaveReason {
    Left,
    /// A person with `disconnect_members` removed the person.
    Disconnected,
    Moved,
    /// The account joined another call.
    Switched,
    /// The media server did not list the person for longer than the server waits.
    ConnectionLost,
    /// The person was allowed in and did not connect while the join token was usable.
    NeverConnected,
    /// The account lost the access to the channel, its session ended, or it was removed or
    /// banned.
    AccessEnded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MeetingEnd {
    TurnedOff,
    ShareStopped,
    PresenterLeft,
}

/// An absent field keeps its value. `user_limit: null` removes the limit.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateCallSettings {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<u32>))]
    pub user_limit: Option<Option<u32>>,
    #[serde(default)]
    pub hides_occupants: Option<bool>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_names_its_kind_and_a_direct_place_orders_its_accounts() {
        let (one, other) = (AccountId::generate(), AccountId::generate());
        assert_eq!(CallPlace::direct(one, other), CallPlace::direct(other, one));
        let direct = serde_json::to_value(CallPlace::direct(other, one)).unwrap();
        assert_eq!(direct["kind"], "direct");
        assert_eq!(direct["low_account_id"], one.to_string());
        let channel = CallPlace::Channel {
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
        };
        let json = serde_json::to_value(channel).unwrap();
        assert_eq!(json["kind"], "channel");
        assert_eq!(serde_json::from_value::<CallPlace>(json).unwrap(), channel);
        assert_eq!(CallPlace::direct(one, other).channel_id(), None);
    }

    #[test]
    fn a_block_and_the_own_setting_are_separate_fields() {
        let participant = CallParticipant {
            avatar: None,
            account_id: AccountId::generate(),
            identity: "seat".to_owned(),
            joined_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
            muted: false,
            deafened: true,
            microphone_block: Some(MicrophoneBlock {
                by: AccountId::generate(),
                reason: BlockReason::Meeting,
            }),
            sound_block: None,
            hand_raised: true,
            watching: None,
            screen_share: Some(ScreenShare {
                audio: ShareAudio::Application,
                started_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
            }),
        };
        let json = serde_json::to_value(&participant).unwrap();
        assert_eq!(json["muted"], false);
        assert_eq!(json["microphone_block"]["reason"], "meeting");
        assert_eq!(json["deafened"], true);
        assert_eq!(json["sound_block"], serde_json::Value::Null);
        assert_eq!(json["screen_share"]["audio"], "application");
        assert_eq!(
            serde_json::from_value::<CallParticipant>(json).unwrap(),
            participant
        );
    }

    #[test]
    fn call_settings_tell_an_absent_limit_from_a_removed_one() {
        let absent: UpdateCallSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.user_limit, None);
        let removed: UpdateCallSettings = serde_json::from_str(r#"{"user_limit":null}"#).unwrap();
        assert_eq!(removed.user_limit, Some(None));
        let set: UpdateCallSettings = serde_json::from_str(r#"{"user_limit":4}"#).unwrap();
        assert_eq!(set.user_limit, Some(Some(4)));
    }
}
