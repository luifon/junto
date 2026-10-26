//! What the window shows, derived from the state of the client core (ADR 0730).
//!
//! [`derive`] is a function without side effects: it takes what the client core holds of
//! each saved server and what the person has selected, and returns plain data that the
//! drawing code turns into elements. The drawing code holds no rule about a server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::RenderImage;
use junto_client_core::api::SettingsScope;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessState, ActionKind, Notice};
use junto_client_core::live::{StreamFault, StreamStatus};
use junto_client_core::model::{
    ChannelModel, Loadable, OpenRecording, PlaceId, ServerModel, Timeline, WorkspaceModel,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::account::{Departure, RecoveryNotice};
use junto_client_core::protocol::call::{
    BlockReason, CallPlace, EntryRequest, LeaveReason, ShareAudio,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{
    AccountId, AttachmentId, CallId, CallInvitationId, ChannelId, ConversationId, EntryRequestId,
    MessageId, RecordingId, RecordingSegmentId, ServerId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::recording::{
    Recording, RecordingFailure, RecordingState, RecordingStopReason,
};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::transfers::{DownloadProgress, UploadProgress};
use junto_client_core::updates::Version;
use junto_client_core::version::{Incompatibility, ServerVersion};
use junto_client_core::voice::{ShareResolution, VOLUME_DEFAULT_PERCENT, VoiceSettings};
use junto_client_core::{Reach, ServerHandle};
use junto_media::audio::{DeviceInfo, Devices};
use junto_media::calls::{ActiveCall, CallTarget, CallsView, EndReason, Outcome, Phase, Seat};
use junto_media::engine::{MicrophoneState, OutputState, ShareId, ShareState};
use junto_media::permission::MicrophoneAccess;
use junto_media::share::{CaptureError, ShareKind};
use time::OffsetDateTime;
use uuid::Uuid;

use super::feed::{self, Feed, MessageRow, Names, Quoted, Zone};
use super::followed::{Followed, held_root};
use super::notices::{Mute, NoticesPanel, NotifyPanel, QuietView};
use super::people::{self, Faces, People, Wanted};
use super::recorded::TranscriptShown;
use super::search::SearchPanel;

/// What the client core holds of one saved server at one moment.
#[derive(Debug, Clone)]
pub struct ServerSnapshot {
    pub id: ServerId,
    pub name: String,
    /// The address as the server book shows it: the origin without its scheme.
    pub address: String,
    /// The connection mode "always" (ADR 0107).
    pub keep_connected: bool,
    pub reach: Reach,
    pub access: AccessState,
    pub in_progress: Option<ActionKind>,
    pub error: Option<FlowError>,
    pub stream: StreamStatus,
    pub model: Option<Arc<ServerModel>>,
    /// The release version that the server stated on its event stream (ADR 0780).
    pub server_version: Option<ServerVersion>,
}

impl ServerSnapshot {
    pub fn of(server: &ServerHandle) -> Self {
        let entry = server.entry();
        let access = server.access();
        let live = server.live().view();
        Self {
            id: server.id(),
            name: entry.name,
            address: shown_address(entry.address.as_str()),
            keep_connected: entry.connection_mode
                == junto_client_core::book::ConnectionMode::Always,
            reach: server.reach(),
            access: access.state,
            in_progress: access.in_progress,
            error: access.error,
            stream: live.status,
            model: live.model,
            server_version: live.server_version,
        }
    }
}

/// The origin of a server without its scheme, as a person types and reads it.
pub fn shown_address(origin: &str) -> String {
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin)
        .to_owned()
}

/// What the person has selected. A selection that the state no longer holds is replaced
/// by the first choice of its kind when the view is derived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub server: Option<ServerId>,
    pub workspaces: HashMap<ServerId, WorkspaceId>,
    /// The channel that each workspace of a server shows. The same identifier of a
    /// workspace on two servers names two workspaces.
    pub channels: HashMap<(ServerId, WorkspaceId), ChannelId>,
    /// The direct conversation that a server shows in place of a channel.
    pub conversations: HashMap<ServerId, ConversationId>,
    /// The thread that the side panel shows: its server, its channel and its root.
    pub thread: Option<(ServerId, ChannelId, MessageId)>,
    /// The channel whose pinned messages the side panel shows.
    pub pins: Option<(ServerId, ChannelId)>,
    /// The recording whose transcript the side panel shows, with its server and its
    /// voice channel.
    pub transcript: Option<(ServerId, ChannelId, RecordingId)>,
    /// The server whose notices the side panel shows.
    pub notices: Option<ServerId>,
    /// The scope whose notification settings the dialog shows.
    pub notify: Option<(ServerId, SettingsScope)>,
}

/// The levels of the call that this device is in, as the meters show them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShownLevels {
    /// The lit segments of the input meter, of [`INPUT_SEGMENTS`].
    pub input: usize,
    /// The lit segments of the meter of the person, of [`LEVEL_SEGMENTS`], and whether the
    /// person speaks.
    pub own: (usize, bool),
    pub people: HashMap<AccountId, (usize, bool)>,
}

/// The segments of the meter of a person.
pub const LEVEL_SEGMENTS: usize = 5;
/// The segments of the input meter of the sound settings.
pub const INPUT_SEGMENTS: usize = 20;

/// What the media part of the client holds at one moment (ADR 0734).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaSnapshot {
    pub calls: CallsView,
    pub levels: ShownLevels,
    pub voice: VoiceSettings,
    /// The sound devices as they were read last.
    pub devices: Devices,
    /// The microphone permission of the system. Absent until it was read.
    pub access: Option<MicrophoneAccess>,
    /// The time of the derivation, for the time of a call. Absent: no time is shown.
    pub now: Option<OffsetDateTime>,
    /// This build can capture a screen on the system that it runs on.
    pub can_share: bool,
}

/// The profiles of accounts that were read, the workspaces whose members could not be
/// read, and what the media part holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    /// The names and the avatars of accounts, as their profiles gave them (ADR 0783).
    pub people: People,
    pub members_failed: Vec<(ServerId, WorkspaceId)>,
    /// The name of the profile preset.
    pub preset_name: String,
    /// The picture of the profile preset, as an image of the window.
    pub preset_picture: Option<Arc<RenderImage>>,
    /// The logos of the servers that have one (ADR 0101).
    pub logos: super::logo::ShownLogos,
    /// The servers that refused the picture of the profile preset for the account that
    /// this device created there (ADR 0717).
    pub preset_refused: HashSet<ServerId>,
    pub media: MediaSnapshot,
    /// The call window draws the call. The main window then shows the direct conversation
    /// of a direct-message call as a conversation, in the whole main area (ADR 0735).
    pub call_in_own_window: bool,
    /// The person put away the notice of the own share that ended. It stays away until
    /// the calls state the share in another way (see [`AppState::dismiss_share_notice`]).
    ///
    /// [`AppState::dismiss_share_notice`]: super::AppState::dismiss_share_notice
    pub share_notice_dismissed: bool,
    /// The last recording of the call of this device that failed or that the server
    /// stopped, with the server of that call. The state of a server holds a recording
    /// only while it runs (ADR 0719).
    pub recording_problem: Option<(ServerId, Box<Recording>)>,
    /// A request that starts or stops a recording is on its way.
    pub recording_busy: bool,
    /// The request of the person to enter a locked or full voice channel.
    pub entry: Option<OwnEntry>,
    /// The messages that replies name and that the loaded messages do not hold.
    pub quoted: HashMap<(ServerId, MessageId), Quoted>,
    /// The time zone of the times of messages.
    pub zone: Zone,
    /// The files that the person attached to the next message of a place.
    pub drafts: HashMap<(ServerId, PlaceId), Vec<DraftFile>>,
    /// The files of messages that the person saves, and those that were saved.
    pub downloads: HashMap<(ServerId, AttachmentId), DownloadProgress>,
    /// The uploads of the messages that wait to be sent, by their server and the number
    /// of the upload. Each server numbers its uploads from 1.
    pub uploads: HashMap<(ServerId, u64), UploadProgress>,
    /// The date of this moment in the time zone of the messages.
    pub today: Option<time::Date>,
    /// The fork of a thread that the person asked for.
    pub fork: Option<ForkAsked>,
    /// The follow state of the thread of the side panel, the followed threads of the
    /// shown channel, and the roots that were read for their titles (ADR 0412, ADR 0413).
    pub followed: super::followed::Followed,
    /// The direct conversation that the server did not open last.
    pub conversation_refused: Option<Refused>,
    /// The account whose direct conversation the server was asked for, while the answer
    /// is on its way.
    pub conversation_asked: Option<(ServerId, AccountId)>,
    /// The time of the derivation, for the end of a mute and for a quiet time.
    pub clock: Option<OffsetDateTime>,
    /// The servers whose notices are being marked as read in one request (ADR 0412), with
    /// the receipt of the answer while the state does not hold its event (ADR 0214).
    pub marking_all: HashMap<ServerId, Option<super::receipts::Awaited>>,
    /// Why the last mark of a notice did not reach its server.
    pub notices_failed: HashMap<ServerId, String>,
    /// The scopes whose notification settings are on their way to their server.
    pub notify_busy: HashSet<(ServerId, Uuid)>,
    /// The notification settings that a server took and that its state does not hold
    /// yet, by their scope.
    pub notify_sent: HashMap<(ServerId, Uuid), super::notices::SentSettings>,
    /// Why a server did not take the last notification settings of a scope.
    pub notify_refused: Option<(ServerId, Uuid, String)>,
    /// The moderation notices that a removed or banned device read with a proof of its
    /// key, by server (ADR 0120).
    pub moderation_read: HashMap<ServerId, super::moderation::ModerationRead>,
    /// The moderation notices that the person put away, as the server book holds them.
    pub moderation_shown: HashMap<ServerId, Vec<Uuid>>,
    /// Why the server book did not take that a notice was put away.
    pub moderation_away_failed: Option<(ServerId, String)>,
    /// The changes of the level of a workspace that are on their way or that the state
    /// does not hold yet (ADR 0214). The dialog of the notification settings and the
    /// page "Notifications" show them.
    pub defaults_sent: HashMap<(ServerId, WorkspaceId), super::notices::SentDefault>,
    /// The message that the person went to from a notice or a search result. Its list
    /// marks it.
    pub target: Option<JumpTarget>,
    /// The search of the window.
    pub search: Option<super::search::Search>,
    /// The files of recordings that the person saves, and those that were saved.
    pub segment_downloads: HashMap<(ServerId, RecordingSegmentId), DownloadProgress>,
    /// The transcripts that the person saves as text.
    pub text_downloads: HashMap<(ServerId, RecordingId), DownloadProgress>,
    /// The recordings whose removal is on its way.
    pub recordings_removing: HashSet<(ServerId, RecordingId)>,
    /// Why the server did not do what the person asked for with a recording.
    pub recording_refused: HashMap<(ServerId, RecordingId), String>,
    /// The recordings whose transcript the person asked for again, while the request is
    /// on its way (ADR 0616).
    pub transcripts_requesting: HashSet<(ServerId, RecordingId)>,
    /// The correction of a line or the name of a voice that the person sent for the
    /// transcript of a recording, while it is on its way or after the server refused it.
    pub transcript_asks: HashMap<(ServerId, RecordingId), super::transcript_work::TranscriptAsk>,
    /// The line that the transcript of the side panel was opened at.
    pub transcript_line: Option<u32>,
    /// The number of the last opening of a transcript at a line.
    pub transcript_jump: u64,
    /// A change of "do not disturb" or of the daily quiet time is on its way.
    pub quiet_busy: bool,
    /// The servers that did not take the last change of the quiet times, with the reason.
    pub quiet_failed: Vec<(ServerId, String)>,
    /// What the update check holds, and what the person did with it (ADR 0780).
    pub updates: super::book::UpdatesKnown,
}

/// Where the fork of a thread stands that the person asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkState {
    Sending,
    /// The server did not make the channel, for this reason.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkAsked {
    pub server_id: ServerId,
    pub root_id: MessageId,
    pub state: ForkState,
}

/// A file that the person attached to a message that is not sent yet, or that waits to be
/// sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftFile {
    /// The number of the upload on this client.
    pub upload: u64,
    pub name: String,
    pub progress: UploadProgress,
}

/// What a place holds that the person has not read (ADR 0408), and what of it the
/// notification settings of the person let through (ADR 0404, ADR 0409). A count of 100
/// means 100 or more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unread {
    /// The unread messages, whatever the settings say.
    pub messages: u64,
    /// The unread mentions that the settings let through.
    pub mentions: u64,
    /// The place shows its unread sign: its level wants what is unread.
    pub sign: bool,
    /// The person muted the place: no sign, and its name is drawn lighter.
    pub muted: bool,
}

impl Unread {
    /// True when a sign is drawn.
    pub const fn any(self) -> bool {
        self.sign || self.mentions > 0
    }

    /// True when something is unread, with a sign or without one.
    pub const fn unread(self) -> bool {
        self.messages > 0 || self.mentions > 0
    }

    fn add(&mut self, other: Self) {
        if !other.any() {
            return;
        }
        self.messages = self
            .messages
            .saturating_add(other.messages)
            .min(MOST_COUNTED);
        self.mentions = self
            .mentions
            .saturating_add(other.mentions)
            .min(MOST_COUNTED);
        self.sign |= other.sign;
    }

    /// The counts of a summary that a server stated. A server is not trusted to keep
    /// them at 100 (ADR 0714).
    fn of_state(state: &junto_client_core::protocol::message::MessageState) -> Self {
        Self {
            messages: state.unread.min(MOST_COUNTED),
            mentions: state.notification_mentions.min(MOST_COUNTED),
            sign: state.unread_sign,
            muted: state.muted,
        }
    }
}

/// The highest count of unread messages and of mentions that the window shows. It means
/// that many or more.
const MOST_COUNTED: u64 = 100;

/// What each place of a server holds unread, made once for a derivation of the window.
/// The summaries of a server are a list without an order; a search of the list for each
/// row would cost the square of its length.
struct Unreads(HashMap<PlaceId, Unread>);

// How many indexes of summaries the derivations of this thread made. A test counts them.
#[cfg(test)]
thread_local! {
    pub(super) static SUMMARIES_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Unreads {
    fn of(model: &ServerModel) -> Self {
        #[cfg(test)]
        SUMMARIES_READ.with(|read| read.set(read.get() + 1));
        Self(
            model
                .message_states
                .iter()
                .map(|state| (PlaceId::of(&state.place), Unread::of_state(state)))
                .collect(),
        )
    }

    fn at(&self, place: PlaceId) -> Unread {
        self.0.get(&place).copied().unwrap_or_default()
    }

    /// Every place of the server together.
    fn total(&self) -> Unread {
        let mut unread = Unread::default();
        for held in self.0.values() {
            unread.add(*held);
        }
        unread
    }
}

/// An unread thread of a channel, a thread of the shown channel that the account follows,
/// or the thread that the side panel shows, as the channel column lists it under its
/// channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadEntry {
    pub root_id: MessageId,
    /// The start of the text of the root. Absent while the root was not read.
    pub title: Option<String>,
    pub unread: u64,
    pub mentions: u64,
    /// The side panel shows this thread.
    pub open: bool,
    /// False for a thread of the voice channel of the call of this device: the view of
    /// that channel is the call, which shows no thread (ADR 0741).
    pub available: bool,
}

/// The row under the shown channel that offers its further followed threads (ADR 0413).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoreFollowed {
    /// How many further followed threads the client holds. Absent while the server lists
    /// further ones: their number is not known.
    pub count: Option<usize>,
}

/// The control of the thread panel that states whether the account follows the thread,
/// and changes it (ADR 0412).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowControl {
    /// The replies of the thread count as unread for the account.
    pub following: bool,
    /// The choice of the account. Absent: the rule of the server decides.
    pub setting: Option<bool>,
    /// A change is on its way: the actions take no click.
    pub busy: bool,
    /// The channel is archived and takes no change (ADR 0410).
    pub read_only: bool,
    /// Why the server did not make the last change.
    pub refused: Option<String>,
}

/// One direct conversation of the server (ADR 0302).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectRow {
    pub id: ConversationId,
    /// The other account of the conversation.
    pub account_id: AccountId,
    /// Absent while the client has not read the name of the account.
    pub name: Option<String>,
    pub selected: bool,
    pub unread: Unread,
    /// The mute of the conversation as the person set it (ADR 0515).
    pub mute: Mute,
    /// The other account left the server or was deleted: the conversation is read only
    /// (ADR 0121).
    pub departure: Option<Departure>,
}

/// The thread that the side panel shows (ADR 0403).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPanel {
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub root_id: MessageId,
    /// The root of the thread. Absent until the first page of the thread has arrived.
    pub root: Option<MessageRow>,
    /// The replies. Absent until the client holds the timeline of the thread.
    pub feed: Option<Feed>,
    /// The channels that the thread was forked into and that the person can see.
    pub forks: Vec<(ChannelId, String, ChannelKind)>,
    /// The roles of the person allow a fork: "manage channel". The server decides.
    pub may_fork: bool,
    /// The files that the person attached to the next reply.
    pub draft: Vec<DraftFile>,
    /// The fork of this thread that the person asked for.
    pub fork: Option<ForkState>,
    /// The messages of the channel cannot be edited after they are sent (ADR 0402).
    pub permanent: bool,
    /// Whether the account follows the thread. Absent until the server has told it.
    pub follow: Option<FollowControl>,
}

/// The pinned messages of the shown channel, as the side panel shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinsState {
    Loading,
    Failed,
    Loaded(Vec<MessageRow>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinsPanel {
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub state: PinsState,
    /// The names for the mentions of the messages.
    pub names: Arc<Names>,
}

/// Where the request of the person to enter a voice channel stands (ADR 0514).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// The request is on its way to the server.
    Asking,
    /// The people inside have the request.
    Waiting,
    Denied,
    /// Nobody answered in time.
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnEntry {
    pub server_id: ServerId,
    pub channel_id: ChannelId,
    /// The request on the server. Absent until the server has answered.
    pub request_id: Option<EntryRequestId>,
    pub state: EntryState,
}

/// What one saved server is to this device now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    /// Logged in, and the state follows the server.
    Ready,
    /// Logged in without a connection, by the mode "connect only when selected".
    Standby,
    /// The client logs in or reads the first state.
    Connecting,
    /// Logged in, and the connection is interrupted. The state is the last known one.
    Reconnecting,
    /// The device belongs to an account, the server answers, and the login did not succeed.
    LoggedOut,
    /// The person logged out of the account on this device (ADR 0712).
    LoggedOutByPerson,
    WaitingForApproval {
        name: String,
    },
    WaitingForLink {
        code: String,
    },
    NoAccount(Option<Notice>),
    Banned,
    /// The account left the last workspace of the server (ADR 0121). An invite code
    /// returns it.
    Inactive,
    /// The account was deleted (ADR 0121).
    Deleted,
    Unreachable,
    Incompatible(Incompatibility),
    OtherServer,
}

impl ServerStatus {
    /// True when the lists of the server are shown.
    pub const fn shows_content(&self) -> bool {
        matches!(self, Self::Ready | Self::Reconnecting)
    }
}

/// The state of a server as the account of the session sees it. The event stream follows
/// the access flow with a delay (ADR 0718): for that time it can still hold the state of
/// the account before, or of a session that has ended. That state is not shown.
pub(crate) fn shown_model(server: &ServerSnapshot) -> Option<&ServerModel> {
    match &server.access {
        AccessState::LoggedIn { account_id, .. } => server
            .model
            .as_deref()
            .filter(|model| model.account.id == *account_id),
        _ => None,
    }
}

pub fn status(server: &ServerSnapshot) -> ServerStatus {
    match server.reach {
        Reach::Incompatible(reason) => return ServerStatus::Incompatible(reason),
        Reach::OtherServer { .. } => return ServerStatus::OtherServer,
        Reach::Unknown | Reach::Reachable { .. } | Reach::Unreachable => {}
    }
    match &server.access {
        AccessState::LoggedIn { .. } => match (shown_model(server), server.stream) {
            (Some(_), StreamStatus::Interrupted(_)) => ServerStatus::Reconnecting,
            (Some(_), _) => ServerStatus::Ready,
            (None, StreamStatus::Interrupted(StreamFault::SessionEnded(_))) => {
                ServerStatus::Connecting
            }
            (None, StreamStatus::Interrupted(_)) => ServerStatus::Unreachable,
            (None, StreamStatus::Off) => ServerStatus::Standby,
            (None, StreamStatus::Connecting | StreamStatus::Live) => ServerStatus::Connecting,
        },
        AccessState::LoggedOut { .. } => {
            if server.in_progress.is_some() || server.reach == Reach::Unknown {
                ServerStatus::Connecting
            } else if server.reach == Reach::Unreachable {
                ServerStatus::Unreachable
            } else {
                ServerStatus::LoggedOut
            }
        }
        AccessState::LoggedOutByPerson { .. } => {
            if server.in_progress.is_some() {
                ServerStatus::Connecting
            } else {
                ServerStatus::LoggedOutByPerson
            }
        }
        AccessState::WaitingForApproval { name, .. } => {
            ServerStatus::WaitingForApproval { name: name.clone() }
        }
        AccessState::WaitingForLink { code, .. } => ServerStatus::WaitingForLink {
            code: code.expose().to_owned(),
        },
        AccessState::NoAccount { notice } => ServerStatus::NoAccount(*notice),
        AccessState::Banned { .. } => ServerStatus::Banned,
        AccessState::Inactive { .. } => ServerStatus::Inactive,
        AccessState::Deleted { .. } => ServerStatus::Deleted,
    }
}

/// One workspace of a server in the server book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: WorkspaceId,
    pub name: String,
    pub selected: bool,
    /// What the channels of the workspace hold that the person has not read.
    pub unread: Unread,
}

/// One server of the server book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub id: ServerId,
    pub name: String,
    pub address: String,
    /// The image of the logo of the server, when the window holds it (ADR 0101). Without
    /// it the window draws the generated picture of the server.
    pub logo: Option<Arc<RenderImage>>,
    pub status: ServerStatus,
    pub selected: bool,
    /// The device has a recovery notice of this server that the person has not seen.
    pub has_recovery: bool,
    /// What the places of the server hold that the person has not read.
    pub unread: Unread,
    /// What of it no listed workspace shows: the direct conversations, and every place
    /// of a server without a listed workspace. The head of the server shows its sign.
    pub head_unread: Unread,
    /// The join requests that wait for a decision of the person (ADR 0761).
    pub waiting_join_requests: usize,
    pub workspaces: Vec<WorkspaceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionTitle {
    TextChannels,
    VoiceChannels,
    Category(String),
}

/// The microphone of a person in a voice channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Microphone {
    Open {
        speaking: bool,
    },
    /// The person closed it.
    Closed,
    /// A moderator or meeting mode closed it.
    Held(BlockReason),
}

/// Whether the media of a person in the call of this device arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    #[default]
    Here,
    /// This device has its place, and makes its first media connection or is not listed
    /// by the server yet: the server lists a person when it has seen the device in the
    /// room, and the person is inside the call from then on (ADR 0517).
    Connecting,
    /// This device restores its media connection.
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupant {
    pub account_id: AccountId,
    /// Absent while the client has not read the name of the account.
    pub name: Option<String>,
    pub is_self: bool,
    pub microphone: Microphone,
    /// The name of the person who closed the microphone, when it is held and the name is
    /// known.
    pub held_by: Option<String>,
    pub sharing: bool,
    pub hand_raised: bool,
    /// The person turned the sound off, or a moderator blocked it.
    pub sound_off: bool,
    /// A moderator blocked the sound of the person (ADR 0503).
    pub sound_blocked: bool,
    /// The lit segments of the level meter, of [`LEVEL_SEGMENTS`]. Present in the call of
    /// this device, where the media engine measures the sound (ADR 0507).
    pub level: Option<usize>,
    pub presence: Presence,
    /// The volume in percent that the person of this device set for the occupant.
    pub volume: u16,
}

/// What this device is to the call of a voice channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Relation {
    #[default]
    Outside,
    /// The server is asked for a place.
    Joining,
    Inside,
}

/// Why a person outside a voice channel cannot enter it without the help of a person
/// inside (ADR 0514).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    Locked,
    Full,
}

/// What a voice channel shows of its call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoiceState {
    pub relation: Relation,
    pub in_use: bool,
    /// The channel is in use and shows who is inside only to the people inside.
    pub hidden: bool,
    pub locked: bool,
    pub meeting: bool,
    /// The person of this device is inside the call and runs its meeting mode.
    pub presents: bool,
    pub recording: bool,
    pub user_limit: Option<u32>,
    pub full: bool,
    pub occupants: Vec<Occupant>,
    /// The request of the person to enter this voice channel.
    pub entry: Option<EntryState>,
    /// The person is outside and cannot enter without help: the state of the channel says
    /// so, or the server refused the last join for that reason.
    pub closed: Option<Closed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: ChannelId,
    pub kind: ChannelKind,
    pub name: String,
    pub selected: bool,
    /// Present for a voice channel.
    pub voice: Option<VoiceState>,
    pub unread: Unread,
    /// The messages of the channel cannot be edited after they are sent (ADR 0402).
    pub permanent: bool,
    /// The unread threads of the channel, for the shown channel the threads that the
    /// account follows, and the thread that the side panel shows.
    pub threads: Vec<ThreadEntry>,
    /// The mute of the channel as the person set it.
    pub mute: Mute,
    /// Present when the shown channel has further followed threads than its rows list.
    pub more_followed: Option<MoreFollowed>,
    /// What the person holds in the channel, as the server stated it (ADR 0310).
    pub allowed: Allowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSection {
    pub title: SectionTitle,
    pub rows: Vec<ChannelRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub account_id: AccountId,
    pub name: String,
    pub is_self: bool,
    /// The member list of the server states that the account set a picture (ADR 0119).
    pub pictured: bool,
    /// The names of the roles of the member, without the "everyone" role.
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Members {
    NotLoaded,
    Loading,
    Failed,
    Loaded(Vec<MemberRow>),
}

/// A member of the workspace who is not in the call that is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideMember {
    pub account_id: AccountId,
    pub name: String,
    /// The name of the voice channel that the member is in.
    pub place: Option<String>,
    /// The invitation into this call that the person of this device made and that waits.
    pub invitation: Option<CallInvitationId>,
}

/// A person outside who asks to enter the call of this device (ADR 0514).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPrompt {
    pub id: EntryRequestId,
    pub account_id: AccountId,
    pub name: Option<String>,
    /// The name of the voice channel of the call.
    pub channel: Option<String>,
}

/// Meeting mode in the call of this device (ADR 0505).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingView {
    /// The person of this device runs it.
    pub by_self: bool,
    pub presenter: Option<String>,
    /// The people with a raised hand whose microphone is closed, for the presenter.
    pub hands: Vec<(AccountId, Option<String>)>,
    /// The people whose block the presenter removed and whose own microphone setting is
    /// "closed", for the presenter.
    pub waiting: Vec<Option<String>>,
    /// The presenter removed the block of the person, and the person's own microphone
    /// setting is "closed".
    pub may_speak: bool,
}

/// What the person holds in one channel, or in the workspace where no channel is meant:
/// the list that the server stated (ADR 0310). The server decides each request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Allowed {
    /// Delete and pin the messages of other people.
    pub manage_messages: bool,
    pub mention_everyone: bool,
    pub lock: bool,
    pub run_meeting: bool,
    pub manage: bool,
    pub record: bool,
    pub mute: bool,
    pub deafen: bool,
    pub disconnect: bool,
    pub move_members: bool,
    /// Remove a recording with its files and its transcript.
    pub manage_recordings: bool,
}

/// Whether the person can turn meeting mode on in the call of this device (ADR 0512).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeetingStart {
    #[default]
    Possible,
    /// The person does not share a screen.
    NotSharing,
    NoPermission,
    /// The microphone of the person is blocked.
    MicrophoneBlocked,
}

/// The room of the call of this device: its rules and what the person may change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomView {
    pub ready: bool,
    /// The name of the voice channel.
    pub name: String,
    /// The number of people inside.
    pub inside: usize,
    pub meeting: Option<MeetingView>,
    pub meeting_start: MeetingStart,
    pub locked: bool,
    pub user_limit: Option<u32>,
    /// The setting of the voice channel.
    pub hides_occupants: bool,
    /// The call of now keeps another setting than its channel has: a call keeps the
    /// setting of its start, and the one of the channel holds for the next call
    /// (ADR 0517).
    pub hide_waits_for_next_call: bool,
    pub allowed: Allowed,
}

/// One device in a list of the sound settings. The first entry is the default of the
/// system and has no identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    pub id: Option<String>,
    pub name: String,
    pub selected: bool,
}

/// The microphone of this device in its call, for the sound settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicrophoneLine {
    /// No microphone is open: the server closed it, or there is no connection.
    Closed,
    /// The system asks the person for the permission.
    Asking,
    Running,
    NoPermission,
    Failed,
}

/// The sound settings of the person in a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSettings {
    pub inputs: Vec<DeviceChoice>,
    pub outputs: Vec<DeviceChoice>,
    /// The lit segments of the input meter, of [`INPUT_SEGMENTS`].
    pub input_level: usize,
    pub microphone: MicrophoneLine,
    pub access: MicrophoneAccess,
    pub output_failed: bool,
}

/// One share of the call of this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareCard {
    pub id: ShareId,
    /// The person who shares.
    pub account_id: AccountId,
    /// Absent while the client has not read the name of the account.
    pub name: Option<String>,
    /// What is shared. Absent for a share whose track does not state it.
    pub kind: Option<ShareKind>,
    /// The number of the share among the shares of its person of the same kind, from 1,
    /// while the person runs two or more of that kind: "Carla's screen 2".
    pub number: Option<usize>,
    /// The sound that the sharer chose. Absent until the server has told it.
    pub sound: Option<ShareAudio>,
    /// The share of this device has a sound that the server holds closed (ADR 0510).
    pub sound_held: bool,
    /// The size of the picture in pixels. Absent until the first picture.
    pub size: Option<(u32, u32)>,
    pub frame_rate: Option<u32>,
    /// The person of this device gets the picture and the sound of this share.
    pub watched: bool,
}

/// The shares of the call of this device. Present while at least one share runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareStage {
    /// The shares of the other people in the order in which they arrived, and then the
    /// share of this device.
    pub shares: Vec<ShareCard>,
    /// The volume in percent of the sound of the watched share for this listener. Zero
    /// keeps the picture without its sound.
    pub volume: u16,
}

impl ShareStage {
    pub fn watched(&self) -> Option<&ShareCard> {
        self.shares.iter().find(|share| share.watched)
    }
}

/// The shares of this device, as their button shows them: none, only shares that start, or
/// at least one share that runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OwnShare {
    #[default]
    Off,
    Starting,
    Running,
}

/// One share of this device, as the menu of the share button lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnShareItem {
    pub slot: u8,
    /// Absent while the share starts.
    pub kind: Option<ShareKind>,
    /// As [`ShareCard::number`].
    pub number: Option<usize>,
}

/// Why the last recording of the call ended without the person asking for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingFault {
    /// The recorder was lost while it recorded.
    RecorderLost,
    /// The free disk space of the server reached its reserve.
    StorageReserve,
    /// The recording holds as many files as a recording can have.
    SegmentLimit,
    /// No recorder took the recording. Nothing was recorded.
    RecorderUnavailable,
    /// Nothing of the recording was saved.
    NothingSaved,
    /// A part of the recording was saved, and a part was not.
    PartlySaved,
    /// The server gave a reason that this build does not know.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingProblem {
    pub fault: RecordingFault,
    /// When the recorder started. Absent when it never did.
    pub since: Option<OffsetDateTime>,
    /// When the recording stopped, or its last job ended.
    pub until: Option<OffsetDateTime>,
    /// The files of the recording, and how many of them failed.
    pub segments: u32,
    pub failed_segments: u32,
}

/// The recording of the call of this device, as the header of its voice channel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingShown {
    /// No recording runs.
    None,
    /// The server asks the recorder to start.
    Starting,
    Running {
        /// The time since the recorder started.
        elapsed: Option<Duration>,
        /// The end of the recording was asked for, and the recorder has not ended yet.
        stopping: bool,
    },
    /// The last recording of this call failed, or the server stopped it.
    Failed(RecordingProblem),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingControl {
    pub state: RecordingShown,
    /// The roles of the person in the workspace allow "record calls". The server decides
    /// each request: an override of the channel can differ (ADR 0719).
    pub may_record: bool,
    /// A request of this device that starts or stops a recording is on its way.
    pub busy: bool,
}

/// The call of this device, when the main area shows its place: its voice channel, or the
/// direct conversation of a direct-message call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPanel {
    pub phase: Phase,
    /// The time of day in UTC at which the media connection was interrupted.
    pub interrupted_at: Option<OffsetDateTime>,
    /// The people in the call, as its tiles and its member list show them.
    pub people: Vec<Occupant>,
    pub audio: AudioSettings,
    /// The shares of the call. Absent while nobody shares.
    pub stage: Option<ShareStage>,
    /// The strip of the call shows only the people who send a camera picture.
    pub hide_without_video: bool,
    pub share_resolution: ShareResolution,
    pub share_frame_rate: u32,
    pub place: PanelPlace,
}

/// What a call has beyond its people and its media, by its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelPlace {
    /// The call of a voice channel: its room and its recording.
    Channel(Box<ChannelCall>),
    /// A direct-message call (ADR 0501). It has no room and no recording.
    Direct(DirectCall),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCall {
    /// The name of the voice channel.
    pub name: String,
    pub outside: Vec<OutsideMember>,
    pub recording: RecordingControl,
    pub room: RoomView,
}

/// The lists of the selected server: its direct conversations and its notices, which
/// belong to the server, and the lists of a workspace when the account can see one
/// (ADR 0741).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLists {
    /// The shown workspace. Absent when the account can see no workspace.
    pub workspace: Option<WorkspaceView>,
    /// The direct conversations of the server.
    pub directs: Vec<DirectRow>,
    /// The direct conversation that the main area shows in place of a channel.
    pub conversation: Option<DirectRow>,
    /// Why the server did not open a direct conversation, with the account asked for.
    pub conversation_refused: Option<ConversationRefusal>,
    /// The account whose direct conversation the server was asked for, while the answer
    /// is on its way.
    pub conversation_asked: Option<AccountId>,
    /// The messages of the place of the main area: the shown conversation or channel.
    /// Absent until the client holds its timeline.
    pub feed: Option<Feed>,
    /// The files that the person attached to the next message of the place of the main
    /// area.
    pub draft: Vec<DraftFile>,
    /// The uploads of the messages that wait to be sent, by the number of the upload.
    pub uploads: Arc<HashMap<u64, UploadProgress>>,
    /// The notices of the server that the side panel shows.
    pub notices: Option<NoticesPanel>,
    /// The notices of the server that the person has not read.
    pub unread_notices: usize,
    /// Present when the main area shows the place of the call of this device: its voice
    /// channel, or the direct conversation of its direct-message call.
    pub call: Option<CallPanel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectCall {
    /// The other account of the call.
    pub other: AccountId,
    /// Absent while the client has not read the name of the account.
    pub name: Option<String>,
    /// The other account has a place in the call. Before that the call rings.
    pub joined: bool,
}

impl CallPanel {
    /// The parts of the call of a voice channel. Absent in a direct-message call.
    pub fn channel(&self) -> Option<&ChannelCall> {
        match &self.place {
            PanelPlace::Channel(channel) => Some(channel),
            PanelPlace::Direct(_) => None,
        }
    }

    pub fn room(&self) -> Option<&RoomView> {
        self.channel().map(|channel| &channel.room)
    }

    pub fn recording(&self) -> Option<&RecordingControl> {
        self.channel().map(|channel| &channel.recording)
    }

    /// The members of the workspace outside the call of a voice channel.
    pub fn outside(&self) -> &[OutsideMember] {
        self.channel()
            .map(|channel| channel.outside.as_slice())
            .unwrap_or_default()
    }

    pub fn direct(&self) -> Option<&DirectCall> {
        match &self.place {
            PanelPlace::Channel(_) => None,
            PanelPlace::Direct(direct) => Some(direct),
        }
    }

    /// The name of the place of the call: the voice channel, or the other person.
    pub fn place_name(&self) -> &str {
        match &self.place {
            PanelPlace::Channel(channel) => &channel.name,
            PanelPlace::Direct(direct) => direct
                .name
                .as_deref()
                .unwrap_or(crate::text::UNKNOWN_PERSON),
        }
    }
}

/// A direct conversation that the server did not open, as the state of the window keeps it
/// (ADR 0741).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub server_id: ServerId,
    /// The account that was asked for.
    pub account_id: AccountId,
    /// The name of the account that the window knew when the person asked, such as the name
    /// of the row that the person chose. A refusal can take away the right to read the
    /// profile of the account.
    pub name: Option<String>,
    pub reason: String,
    /// The place of the main area where the refusal is stated. It goes when the main area
    /// shows another place.
    pub place: Option<PlaceId>,
}

/// A direct conversation that the server did not open, as the main area states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRefusal {
    pub account_id: AccountId,
    /// Absent while the client has not read the name of the account.
    pub name: Option<String>,
    pub reason: String,
}

/// The selected workspace of the selected server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: WorkspaceId,
    pub name: String,
    pub sections: Vec<ChannelSection>,
    /// The selected channel, as the main area shows it.
    pub channel: Option<ChannelRow>,
    pub members: Members,
    /// What the person holds in the shown channel, and in the workspace while no channel
    /// is shown.
    pub allowed: Allowed,
    /// The roles of the workspace that a message can mention, without the "everyone" role.
    pub roles: Vec<(WorkspaceRoleId, String)>,
    /// The thread that the side panel shows.
    pub thread: Option<ThreadPanel>,
    /// The pinned messages that the side panel shows.
    pub pins: Option<PinsPanel>,
    /// The search that the side panel shows.
    pub search: Option<SearchPanel>,
    /// The transcript of a recording that the side panel shows.
    pub transcript: Option<TranscriptPanel>,
    /// Present when the shown channel is archived (ADR 0762): it is read and takes no
    /// message.
    pub archived: Option<ArchivedState>,
}

/// The transcript of one recording of the shown voice channel, as the side panel shows it
/// (ADR 0744).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptPanel {
    pub recording_id: RecordingId,
    /// The name of the voice channel.
    pub channel: String,
    pub state: TranscriptPanelState,
    /// The line that the panel was opened at.
    pub line: Option<u32>,
    /// The number of the opening of the panel at a line. The list goes to the line once
    /// for each number.
    pub jump: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptPanelState {
    Reading,
    Failed,
    /// The server has no such recording for the account.
    Gone,
    Shown {
        /// In a box: the transcript with its details is larger than the other states.
        transcript: Box<TranscriptShown>,
        /// The recording with the lines that were read. The panel derives a line when it
        /// draws it.
        open: Arc<OpenRecording>,
        /// The names of the accounts, for the speakers.
        names: Arc<HashMap<AccountId, String>>,
        /// The zone and the day by which the panel states the time of a change.
        when: super::recorded::When,
    },
}

/// The archive state of the shown channel, in the time zone of the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivedState {
    pub archived_at: OffsetDateTime,
    /// The server does not delete the channel before this time. Absent: the server does
    /// not delete it.
    pub deletes_at: Option<OffsetDateTime>,
    /// The person holds "manage channel" in the channel: it can be restored. False until
    /// the list of the archived channel is read (ADR 0310).
    pub may_restore: bool,
}

/// The menu of a person in a voice channel (ADR 0739): what the person of this device can
/// do with that person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonMenu {
    pub server_id: ServerId,
    pub channel_id: ChannelId,
    pub account_id: AccountId,
    pub name: Option<String>,
    /// The name of the voice channel that the person is in.
    pub place: String,
    /// The name of the voice channel of the call of this device, when the person can be
    /// invited into it.
    pub invite_to: Option<String>,
    /// The voice channels that the person can be moved to. Empty without "move members".
    pub destinations: Vec<(ChannelId, String)>,
    /// The volume in percent that the person of this device set for the person.
    pub volume: u16,
    /// With "mute members": whether the microphone of the person is blocked now.
    pub microphone_blocked: Option<bool>,
    /// With "deafen members": whether the sound of the person is blocked now.
    pub sound_blocked: Option<bool>,
    /// With "mute members", or for the presenter of meeting mode in that call: whether the
    /// hand of the person is raised now (ADR 0516).
    pub hand_raised: Option<bool>,
    pub disconnect: bool,
}

impl ShellView {
    pub fn may_remove_recording(&self, server_id: ServerId, recording_id: RecordingId) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|server| server.id == server_id)
            && self
                .lists()
                .and_then(|lists| lists.feed.as_ref())
                .is_some_and(|feed| {
                    feed.rows.iter().any(|row| {
                        matches!(row, feed::FeedRow::Recording(row)
                    if row.card.recording_id == recording_id && row.card.may_remove)
                    })
                })
    }

    /// The call and server named by the detached window.
    pub fn call_title(&self) -> String {
        let name = self
            .call
            .as_ref()
            .and_then(|call| call.name.as_deref())
            .unwrap_or(crate::text::IN_CALL);
        match self.call.as_ref().and_then(|call| {
            self.servers
                .iter()
                .find(|server| server.id == call.server_id)
        }) {
            Some(server) => format!("{name} · {}", server.name),
            None => name.to_owned(),
        }
    }

    /// The available actions for another person still in the selected workspace.
    pub fn person_menu(&self, channel_id: ChannelId, account_id: AccountId) -> Option<PersonMenu> {
        let server_id = self.selected.as_ref()?.id;
        let workspace = self.workspace()?;
        let rows = || workspace.sections.iter().flat_map(|section| &section.rows);
        let row = rows().find(|row| row.id == channel_id)?;
        let occupant = row
            .voice
            .as_ref()?
            .occupants
            .iter()
            .find(|occupant| occupant.account_id == account_id && !occupant.is_self)?;
        // The person menu acts in the voice channel of its row: the list of that channel
        // decides.
        let allowed = row.allowed;
        let own_call = self
            .call
            .as_ref()
            .filter(|call| call.server_id == server_id && call.workspace_id == Some(workspace.id));
        Some(PersonMenu {
            server_id,
            channel_id,
            account_id,
            name: occupant.name.clone(),
            place: row.name.clone(),
            invite_to: own_call
                .filter(|call| {
                    call.phase == Phase::Connected && call.channel_id != Some(channel_id)
                })
                .and_then(|call| call.name.clone()),
            // A move needs "move members" in both channels (ADR 0511).
            destinations: if allowed.move_members {
                rows()
                    .filter(|other| {
                        other.kind == ChannelKind::Voice
                            && other.id != channel_id
                            && other.allowed.move_members
                    })
                    .map(|other| (other.id, other.name.clone()))
                    .collect()
            } else {
                Vec::new()
            },
            volume: occupant.volume,
            microphone_blocked: allowed
                .mute
                .then_some(matches!(occupant.microphone, Microphone::Held(_))),
            sound_blocked: allowed.deafen.then_some(occupant.sound_blocked),
            hand_raised: (allowed.mute || row.voice.as_ref().is_some_and(|voice| voice.presents))
                .then_some(occupant.hand_raised),
            disconnect: allowed.disconnect,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The lists of the server, with or without a workspace.
    Lists(Box<ServerLists>),
    /// The server has no lists to show; its status says why.
    State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedServer {
    pub id: ServerId,
    pub name: String,
    pub address: String,
    pub status: ServerStatus,
    pub keep_connected: bool,
    /// An action of the access flow runs.
    pub busy: bool,
    /// The person can log out of the account of the server on this device (ADR 0712).
    pub can_log_out: bool,
    /// Why the last action of the access flow failed.
    pub error: Option<FlowError>,
    /// The recoveries that this device has not acknowledged, oldest first.
    pub recoveries: Vec<RecoveryNotice>,
    /// The key of this device belonged to another account at the last contact
    /// (ADR 0712).
    pub account_changed: bool,
    /// The private moderation notices that the window shows for the server (ADR 0120).
    pub moderation: Vec<super::moderation::ModerationLine>,
    /// Why the server book did not take that a notice was put away.
    pub moderation_failed: Option<String>,
    /// The version that the server states, for the menu of the server. Absent while the
    /// server has stated none.
    pub release: Option<ServerRelease>,
    pub content: Content,
}

/// The release version that a server states, and the newer release of the server that the
/// source of releases names (ADR 0780).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRelease {
    pub version: ServerVersion,
    /// The newest release, when it comes after `version`. Absent without a source.
    pub newer: Option<Version>,
}

/// The person as the profile strip shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The account on the selected server. Absent without one.
    pub account_id: Option<AccountId>,
    /// Empty when no name is known.
    pub name: String,
}

/// The microphone button of the profile strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicrophoneButton {
    #[default]
    Open,
    /// The person closed the microphone and can open it.
    Muted,
    /// The server closed it. The person cannot open it.
    Held,
}

/// What the line above the profile strip tells about the sound of this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundWarning {
    MicrophoneHeld {
        reason: BlockReason,
        by: Option<String>,
    },
    /// The system does not allow this application to use a microphone.
    NoMicrophoneAccess,
    MicrophoneFailed,
    SoundHeld {
        by: Option<String>,
    },
    OutputFailed,
}

/// What the microphone control does when the person uses it. A control holds the action
/// from its drawing: the call can be another one when the person uses it (ADR 0788).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MicrophoneAction {
    #[default]
    Close,
    /// Opens the microphone at this seat. Absent: the device was in no call.
    Open(Option<Seat>),
}

/// The person's own controls, in a call and outside one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Controls {
    pub microphone: MicrophoneButton,
    pub microphone_action: MicrophoneAction,
    /// The person turned the sound off.
    pub sound_off: bool,
    pub warning: Option<SoundWarning>,
    /// The call and the seat of this device that the controls were derived for.
    pub seat: Option<Seat>,
}

/// The call of this device, as the strip above the profile strip shows it. It is shown
/// whatever server and channel are selected: a client is in one call (ADR 0107).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallStrip {
    pub server_id: ServerId,
    pub workspace_id: Option<WorkspaceId>,
    pub channel_id: Option<ChannelId>,
    /// The name of the voice channel. Absent when the state does not hold the channel.
    pub name: Option<String>,
    pub phase: Phase,
    /// The time since the call started.
    pub elapsed: Option<Duration>,
    /// The main area shows this call.
    pub shown: bool,
    pub share: OwnShare,
    /// The shares of this device in the order of their slots.
    pub own_shares: Vec<OwnShareItem>,
    /// How many shares this device can run at once in the call (ADR 0502).
    pub share_limit: u8,
    /// This build can capture a screen on the system that it runs on.
    pub can_share: bool,
    /// The hand of the person is raised. Absent in a direct-message call, which has no
    /// meeting mode.
    pub hand_raised: Option<bool>,
    /// Meeting mode is on, the presenter removed the block of the person, and the person's
    /// own microphone setting is "closed".
    pub may_speak: bool,
}

/// An invitation into a call that waits for the answer of the person (ADR 0514).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invited {
    pub id: CallInvitationId,
    pub caller: AccountId,
    pub target: CallTarget,
    pub by: Option<String>,
    /// The name of the voice channel. Absent for a direct-message call.
    pub channel: Option<String>,
    /// When the invitation was made: the window shows the newest ones first.
    pub created_at: OffsetDateTime,
}

/// How the last join or call ended, when the person did not end it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallNotice {
    DirectJoinFailed {
        error: FlowError,
    },
    DirectEnded {
        reason: EndReason,
    },
    JoinFailed {
        channel: Option<String>,
        error: FlowError,
    },
    Ended {
        channel: Option<String>,
        reason: EndReason,
    },
    /// A person with "move members" moved the person into this voice channel.
    Moved {
        channel: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellView {
    pub servers: Vec<ServerRow>,
    pub selected: Option<SelectedServer>,
    pub person: Person,
    pub controls: Controls,
    pub call: Option<CallStrip>,
    pub call_notice: Option<CallNotice>,
    /// Why the share of this device ended, when the person did not end it.
    pub share_notice: Option<CaptureError>,
    /// The invitations that wait for the answer of the person, on every server.
    pub invitations: Vec<Invited>,
    /// The people outside who ask to enter the call of this device.
    pub entry_prompts: Vec<EntryPrompt>,
    /// The files of messages that the person saves, and those that were saved.
    pub downloads: Arc<HashMap<(ServerId, AttachmentId), DownloadProgress>>,
    /// The notification settings that the dialog shows.
    pub notify: Option<NotifyPanel>,
    /// "Do not disturb" and the daily quiet time of the application.
    pub quiet: QuietView,
    /// The text of the search of the shown server, while it has one.
    pub search_text: Option<String>,
    /// The pictures of the people that the window shows (ADR 0783). A drawing takes the
    /// pictures of its server once, with [`Faces::of`], and asks for each person by the
    /// account.
    pub faces: Faces,
    /// The people that this view names, by server. The state of the application reads
    /// their profiles; the derivation starts nothing.
    pub wanted_people: HashMap<ServerId, Wanted>,
    /// The picture of the profile preset. The profile strip shows it for a person
    /// without an account on the selected server.
    pub preset_picture: Option<Arc<RenderImage>>,
    /// The names of the servers that refused the picture of the profile preset, in the
    /// order of the server book.
    pub preset_refused: Vec<(ServerId, String)>,
    /// A newer build of the client that the source of releases offers (ADR 0780).
    pub update: Option<super::book::Available>,
}

impl ShellView {
    /// The lists of the selected server, while it shows them.
    pub fn lists(&self) -> Option<&ServerLists> {
        match &self.selected.as_ref()?.content {
            Content::Lists(lists) => Some(lists),
            Content::State => None,
        }
    }

    pub fn workspace(&self) -> Option<&WorkspaceView> {
        self.lists()?.workspace.as_ref()
    }

    /// Puts these levels into a view that [`derive`] made with other levels. The result is
    /// the view that `derive` makes with these levels: only the meters and the speaking
    /// marks of the call of this device and the input meter of its panel read them. A
    /// change of the levels alone, at most every 50 ms, therefore does not derive the view
    /// again (ADR 0730).
    pub fn show_levels(&mut self, levels: &ShownLevels) {
        let Some(SelectedServer {
            content: Content::Lists(lists),
            ..
        }) = &mut self.selected
        else {
            return;
        };
        let ServerLists {
            workspace, call, ..
        } = &mut **lists;
        // The rows of the channel list, the shown channel, which holds a copy of its row,
        // and the people of the panel of the call, which are a copy too.
        let occupants = workspace
            .iter_mut()
            .flat_map(|workspace| {
                workspace
                    .sections
                    .iter_mut()
                    .flat_map(|section| &mut section.rows)
                    .chain(workspace.channel.as_mut())
            })
            .filter_map(|row| row.voice.as_mut())
            .flat_map(|voice| &mut voice.occupants)
            .chain(call.iter_mut().flat_map(|call| &mut call.people));
        for occupant in occupants {
            // Only an occupant of the call of this device has a measured level. The others
            // show the marks of the server.
            if occupant.level.is_none() {
                continue;
            }
            let (level, speaking) = if occupant.is_self {
                levels.own
            } else {
                levels
                    .people
                    .get(&occupant.account_id)
                    .copied()
                    .unwrap_or_default()
            };
            occupant.level = Some(level);
            if let Microphone::Open { speaking: shown } = &mut occupant.microphone {
                *shown = speaking;
            }
        }
        if let Some(call) = call {
            call.audio.input_level = levels.input;
        }
    }
}

/// The selection as the view resolved it: what is shown now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shown {
    pub server: Option<ServerId>,
    pub workspace: Option<WorkspaceId>,
    pub channel: Option<ChannelId>,
    /// The direct conversation that the main area shows in place of a channel.
    pub conversation: Option<ConversationId>,
    /// The thread of the side panel: its channel and its root.
    pub thread: Option<(ChannelId, MessageId)>,
    /// The channel whose pinned messages the side panel shows.
    pub pins: Option<ChannelId>,
    /// The side panel shows the notices of the server.
    pub notices: bool,
    /// The recording whose transcript the side panel shows.
    pub transcript: Option<RecordingId>,
}

impl Shown {
    /// What a window shows that shows the messages of one place and nothing else, as the
    /// call window shows the chat of its call.
    pub fn of_place(place: Option<(ServerId, PlaceId)>) -> Self {
        let Some((server_id, place)) = place else {
            return Self::default();
        };
        Self {
            server: Some(server_id),
            channel: match place {
                PlaceId::Channel(channel_id) => Some(channel_id),
                _ => None,
            },
            conversation: match place {
                PlaceId::Direct(conversation_id) => Some(conversation_id),
                _ => None,
            },
            ..Self::default()
        }
    }

    /// The place whose messages the main area shows.
    pub fn place(&self) -> Option<PlaceId> {
        self.conversation
            .map(PlaceId::Direct)
            .or(self.channel.map(PlaceId::Channel))
    }

    /// The thread of the side panel as a place of messages.
    pub fn thread_place(&self) -> Option<PlaceId> {
        self.thread.map(|(channel_id, root_id)| PlaceId::Thread {
            channel_id,
            root_id,
        })
    }
}

impl ShellView {
    pub fn shown(&self) -> Shown {
        let workspace = self.workspace();
        Shown {
            server: self.selected.as_ref().map(|server| server.id),
            workspace: workspace.map(|workspace| workspace.id),
            channel: workspace
                .and_then(|workspace| workspace.channel.as_ref())
                .map(|channel| channel.id),
            conversation: self
                .lists()
                .and_then(|lists| lists.conversation.as_ref())
                .map(|conversation| conversation.id),
            thread: workspace
                .and_then(|workspace| workspace.thread.as_ref())
                .map(|thread| (thread.channel_id, thread.root_id)),
            pins: workspace
                .and_then(|workspace| workspace.pins.as_ref())
                .map(|pins| pins.channel_id),
            notices: self.lists().is_some_and(|lists| lists.notices.is_some()),
            transcript: workspace
                .and_then(|workspace| workspace.transcript.as_ref())
                .map(|transcript| transcript.recording_id),
        }
    }
}

/// The workspace of a server that is shown: the selected one when the state holds it, the
/// default workspace otherwise, and the first one when there is no default.
pub(crate) fn shown_workspace(
    model: &ServerModel,
    selected: Option<WorkspaceId>,
) -> Option<&WorkspaceModel> {
    selected
        .and_then(|id| model.workspace(id))
        .or_else(|| {
            model
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace.is_default)
                .map(Arc::as_ref)
        })
        .or_else(|| model.workspaces.first().map(Arc::as_ref))
}

fn target_of(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    channel: &ChannelModel,
) -> CallTarget {
    CallTarget {
        server_id,
        place: CallPlace::Channel {
            workspace_id: workspace.workspace.id,
            channel_id: channel.channel.id,
        },
    }
}

/// The name of an account: from the member list of the shown workspace, when there is one,
/// and from the profiles that were read otherwise.
fn name_of(
    server_id: ServerId,
    account_id: AccountId,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
) -> Option<String> {
    if account_id == model.account.id {
        return Some(crate::text::label(&model.account.name));
    }
    workspace
        .and_then(|workspace| workspace.members.loaded())
        .and_then(|members| {
            members
                .iter()
                .find(|member| member.account_id == account_id)
                .map(|member| crate::text::label(&member.name))
        })
        .or_else(|| known.people.name(server_id, account_id))
}

/// The people of a call as the window shows them: the participants that the server lists,
/// and the person of this device from the moment the device has its place.
fn call_people(
    server_id: ServerId,
    call: Option<&junto_client_core::model::CallModel>,
    active: Option<&ActiveCall>,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
) -> Vec<Occupant> {
    let own = model.account.id;
    let media = &known.media;
    let name_of = |account_id: AccountId| name_of(server_id, account_id, workspace, model, known);
    let own_presence = match active.map(|call| call.phase) {
        Some(Phase::Connecting | Phase::Entering) => Presence::Connecting,
        Some(Phase::Reconnecting) => Presence::Reconnecting,
        Some(Phase::Connected) | None => Presence::Here,
    };
    // The server lists a person after the media server has listed them (ADR 0510). The
    // person of this device is shown from the moment the device has its place, and as
    // connecting until the server lists the person.
    let unlisted = match own_presence {
        Presence::Here => Presence::Connecting,
        other => other,
    };
    // The row of the person in the call of this device says what the device does, read
    // from the calls of the device: the person's own setting, the microphone of the
    // engine and the share of the device. A block is a rule of the server and is shown as
    // one, from the calls or from the list of the server. The settings that the server
    // lists for the person are a report that the device sent, and can be older; they are
    // not shown for this device (ADR 0789). The hand is held by the server.
    type Listed = junto_client_core::protocol::call::CallParticipant;
    let own_row = |active: &ActiveCall, listed: Option<&Listed>, presence: Presence| {
        let held = active
            .microphone_block
            .as_ref()
            .or(listed.and_then(|listed| listed.microphone_block.as_ref()));
        let sound_blocked = active.sound_block.is_some()
            || listed.is_some_and(|listed| listed.sound_block.is_some());
        let sends = !media.calls.muted && active.microphone == MicrophoneState::Running;
        Occupant {
            account_id: own,
            name: name_of(own),
            is_self: true,
            microphone: match held {
                Some(block) => Microphone::Held(block.reason),
                None if sends => Microphone::Open {
                    speaking: media.levels.own.1,
                },
                None => Microphone::Closed,
            },
            held_by: held.and_then(|block| name_of(block.by)),
            sharing: active
                .own_shares
                .values()
                .any(|share| matches!(share, ShareState::Running(_))),
            hand_raised: listed.is_some_and(|listed| listed.hand_raised),
            sound_off: media.calls.deafened || sound_blocked,
            sound_blocked,
            level: Some(media.levels.own.0),
            presence,
            volume: VOLUME_DEFAULT_PERCENT,
        }
    };
    let Some(call) = call else {
        return active
            .map(|active| vec![own_row(active, None, unlisted)])
            .unwrap_or_default();
    };
    let mut occupants: Vec<Occupant> = call
        .participants
        .iter()
        .map(|participant| {
            let is_self = participant.account_id == own;
            if is_self && let Some(active) = active {
                return own_row(active, Some(participant), own_presence);
            }
            let measured = active.map(|_| {
                media
                    .levels
                    .people
                    .get(&participant.account_id)
                    .copied()
                    .unwrap_or_default()
            });
            Occupant {
                account_id: participant.account_id,
                name: name_of(participant.account_id),
                is_self,
                microphone: match (&participant.microphone_block, participant.muted) {
                    (Some(block), _) => Microphone::Held(block.reason),
                    (None, true) => Microphone::Closed,
                    (None, false) => Microphone::Open {
                        // In the call of this device the sound itself tells who speaks;
                        // outside, the marks of the server do (ADR 0513).
                        speaking: match measured {
                            Some((_, speaking)) => speaking,
                            None => call.speaking.contains(&participant.account_id),
                        },
                    },
                },
                held_by: participant
                    .microphone_block
                    .as_ref()
                    .and_then(|block| name_of(block.by)),
                sharing: !participant.shares().is_empty(),
                hand_raised: participant.hand_raised,
                sound_off: participant.deafened || participant.sound_block.is_some(),
                sound_blocked: participant.sound_block.is_some(),
                level: measured.map(|(level, _)| level),
                presence: Presence::Here,
                volume: media.voice.volume(server_id, participant.account_id),
            }
        })
        .collect();
    if let Some(active) = active
        && !occupants.iter().any(|occupant| occupant.is_self)
    {
        occupants.push(own_row(active, None, unlisted));
    }
    occupants
}

/// What the window shows of the request of the person to enter one voice channel.
pub(super) struct ShownEntry {
    pub(super) state: Option<EntryState>,
    /// The request of another device of the account that is shown as waiting.
    pub(super) elsewhere: Option<EntryRequestId>,
}

/// A request of this device that waits or is on its way is shown as it is. Otherwise a
/// request of the account that another device made waits as the state of the server lists
/// it (ADR 0719), also over a request of this device that ended; then the end of the
/// request of this device is shown. The state of the server can still list the request of
/// this device that ended, which is not shown as waiting.
pub(super) fn shown_entry(
    own: Option<&OwnEntry>,
    model: &ServerModel,
    server_id: ServerId,
    channel_id: ChannelId,
) -> ShownEntry {
    let own = own.filter(|entry| entry.server_id == server_id && entry.channel_id == channel_id);
    if let Some(entry) =
        own.filter(|entry| matches!(entry.state, EntryState::Asking | EntryState::Waiting))
    {
        return ShownEntry {
            state: Some(entry.state),
            elsewhere: None,
        };
    }
    let elsewhere = own_entry_request(model, channel_id)
        .map(|request| request.id)
        .filter(|id| own.is_none_or(|entry| entry.request_id != Some(*id)));
    ShownEntry {
        state: match elsewhere {
            Some(_) => Some(EntryState::Waiting),
            None => own.map(|entry| entry.state),
        },
        elsewhere,
    }
}

/// The request of the account to enter a voice channel, from whichever device of the
/// account it was made, as the state of the server lists it.
pub(super) fn own_entry_request(
    model: &ServerModel,
    channel_id: ChannelId,
) -> Option<&EntryRequest> {
    model.own_entry_requests.iter().find(|request| {
        matches!(request.place, CallPlace::Channel { channel_id: of, .. } if of == channel_id)
    })
}

fn voice_state(
    server_id: ServerId,
    channel: &ChannelModel,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> VoiceState {
    let own = model.account.id;
    let media = &known.media;
    let target = target_of(server_id, workspace, channel);
    let active = media
        .calls
        .call
        .as_ref()
        .filter(|call| call.target == target);
    let relation = if active.is_some() {
        Relation::Inside
    } else if media.calls.joining == Some(target) {
        Relation::Joining
    } else {
        Relation::Outside
    };
    let occupants = call_people(
        server_id,
        channel.call.as_ref(),
        active,
        Some(workspace),
        model,
        known,
    );
    let entry = shown_entry(known.entry.as_ref(), model, server_id, channel.channel.id)
        .state
        .filter(|_| relation == Relation::Outside);
    // What the server answered to the last join of this voice channel.
    let refused = match &media.calls.outcome {
        Some(Outcome::JoinFailed {
            target: failed,
            error: FlowError::Refused { code, .. },
        }) if *failed == target => match code {
            ErrorCode::CallLocked => Some(Closed::Locked),
            ErrorCode::CallFull => Some(Closed::Full),
            _ => None,
        },
        _ => None,
    };
    let outside = relation == Relation::Outside;
    let Some(call) = &channel.call else {
        return VoiceState {
            relation,
            in_use: channel.in_use,
            // The server tells nothing of a call in use only when that call hides who is
            // inside. The setting of the channel holds for its next call (ADR 0517).
            hidden: channel.in_use,
            entry,
            closed: refused.filter(|_| outside && channel.in_use),
            user_limit: channel.channel.user_limit,
            occupants,
            ..VoiceState::default()
        };
    };
    let in_use = channel.in_use;
    // A lock can be in the state before the first person of a call is seen (ADR 0719). It
    // is shown only for a channel in use.
    let locked = in_use && call.lock.is_some();
    let full = channel
        .channel
        .user_limit
        .is_some_and(|limit| occupants.len() >= limit as usize && limit > 0);
    VoiceState {
        relation,
        in_use,
        hidden: false,
        locked,
        meeting: in_use && call.meeting.is_some(),
        presents: in_use
            && active.is_some()
            && call
                .meeting
                .as_ref()
                .is_some_and(|meeting| meeting.presenter == own),
        recording: in_use && call.recording.is_some(),
        user_limit: channel.channel.user_limit,
        full,
        closed: if !outside || !in_use {
            None
        } else if locked {
            Some(Closed::Locked)
        } else if full {
            Some(Closed::Full)
        } else {
            refused
        },
        occupants,
        entry,
    }
}

fn channel_row(
    server_id: ServerId,
    channel: &ChannelModel,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
    unreads: &Unreads,
) -> ChannelRow {
    ChannelRow {
        id: channel.channel.id,
        kind: channel.channel.kind,
        name: channel.channel.name.clone(),
        selected: false,
        voice: (channel.channel.kind == ChannelKind::Voice)
            .then(|| voice_state(server_id, channel, workspace, model, known)),
        unread: unreads.at(PlaceId::Channel(channel.channel.id)),
        permanent: channel.channel.permanent_messages,
        threads: Vec::new(),
        mute: Mute::of_channel(channel.channel.id, model, known),
        more_followed: None,
        allowed: allowed(workspace.permissions_in(channel.channel.id)),
    }
}

/// The characters of the root of a thread that the channel column shows.
const THREAD_TITLE_CHARACTERS: usize = 48;

/// The unread threads of a channel, in the order of the server; with `followed`, for the
/// shown channel, the threads that the account follows and that are not among them, the
/// one with the newest reply first (ADR 0413); and after them the thread that the side
/// panel shows when no row lists it. With them, the row that offers the further followed
/// threads.
fn thread_entries(
    channel_id: ChannelId,
    model: &ServerModel,
    names: &Names,
    open: Option<MessageId>,
    followed: Option<(ServerId, &Followed)>,
) -> (Vec<ThreadEntry>, Option<MoreFollowed>) {
    use junto_client_core::model::Named;
    let title = |root: &junto_client_core::protocol::message::Message| {
        root.document
            .as_ref()
            .map(|document| feed::plain_text(document, names, THREAD_TITLE_CHARACTERS))
            .filter(|title| !title.is_empty())
            .or_else(|| {
                root.attachments
                    .first()
                    .map(|file| crate::text::label(&file.name))
            })
    };
    let mut entries: Vec<ThreadEntry> = model
        .threads
        .iter()
        .filter_map(|state| match model.named(state.root_id) {
            Some(Named::Read(root)) if PlaceId::of(&root.place) == PlaceId::Channel(channel_id) => {
                Some(ThreadEntry {
                    root_id: state.root_id,
                    title: title(root),
                    unread: state.unread,
                    mentions: state.mentions,
                    open: open == Some(state.root_id),
                    available: true,
                })
            }
            _ => None,
        })
        .collect();
    let mut more = None;
    if let Some((server_id, followed)) = followed {
        let listed: Vec<MessageId> = entries.iter().map(|entry| entry.root_id).collect();
        let (further, offered) = followed.list.listed((server_id, channel_id), &listed, open);
        more = offered;
        for state in further {
            // The state of the server holds every unread thread, or the first pages of
            // them: beyond those pages the list of followed threads tells what is unread.
            let (unread, mentions) = match model.thread_state(state.root_id) {
                Some(held) => (held.unread, held.mentions),
                None if model.threads_more => (state.unread, state.mentions),
                None => (0, 0),
            };
            entries.push(ThreadEntry {
                root_id: state.root_id,
                title: held_root(model, channel_id, state.root_id)
                    .or_else(|| followed.roots.get(server_id, state.root_id))
                    .and_then(&title),
                unread,
                mentions,
                open: open == Some(state.root_id),
                available: true,
            });
        }
    }
    if let Some(root_id) = open
        && entries.iter().all(|entry| entry.root_id != root_id)
    {
        let place = PlaceId::Thread {
            channel_id,
            root_id,
        };
        entries.push(ThreadEntry {
            root_id,
            title: model
                .timeline(place)
                .and_then(|timeline| timeline.root.as_deref())
                .and_then(title),
            unread: 0,
            mentions: 0,
            open: true,
            available: true,
        });
    }
    (entries, more)
}

/// The channels of a workspace in the groups of the channel column: first the channels
/// without a category, by kind, then each category that holds a visible channel.
fn sections(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
    unreads: &Unreads,
) -> Vec<ChannelSection> {
    let in_category = |channel: &ChannelModel| {
        channel.channel.category_id.filter(|id| {
            workspace
                .categories
                .iter()
                .any(|category| category.id == *id)
        })
    };
    let row =
        |channel: &ChannelModel| channel_row(server_id, channel, workspace, model, known, unreads);
    let mut made = Vec::new();
    for (kind, title) in [
        (ChannelKind::Text, SectionTitle::TextChannels),
        (ChannelKind::Voice, SectionTitle::VoiceChannels),
    ] {
        let rows: Vec<ChannelRow> = workspace
            .channels
            .iter()
            .filter(|channel| channel.channel.kind == kind && in_category(channel).is_none())
            .map(row)
            .collect();
        if !rows.is_empty() {
            made.push(ChannelSection { title, rows });
        }
    }
    for category in &workspace.categories {
        let rows: Vec<ChannelRow> = workspace
            .channels
            .iter()
            .filter(|channel| in_category(channel) == Some(category.id))
            .map(row)
            .collect();
        if !rows.is_empty() {
            made.push(ChannelSection {
                title: SectionTitle::Category(category.name.clone()),
                rows,
            });
        }
    }
    made
}

fn members(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Members {
    match &workspace.members {
        Loadable::Loaded(members) => {
            let mut rows: Vec<MemberRow> = members
                .iter()
                .map(|member| MemberRow {
                    pictured: member.avatar.is_some(),
                    account_id: member.account_id,
                    name: crate::text::label(&member.name),
                    is_self: member.account_id == model.account.id,
                    roles: workspace
                        .roles
                        .iter()
                        .filter(|role| !role.is_everyone && member.role_ids.contains(&role.id))
                        .map(|role| crate::text::label(&role.name))
                        .collect(),
                })
                .collect();
            rows.sort_by(|one, other| {
                one.name
                    .to_lowercase()
                    .cmp(&other.name.to_lowercase())
                    .then(one.account_id.cmp(&other.account_id))
            });
            Members::Loaded(rows)
        }
        Loadable::Loading => Members::Loading,
        Loadable::NotLoaded => {
            if known
                .members_failed
                .contains(&(server_id, workspace.workspace.id))
            {
                Members::Failed
            } else {
                Members::NotLoaded
            }
        }
    }
}

/// The direct conversations of a server, by the name of the other person. A person whose
/// name is not known yet stands at the end.
fn directs(
    server_id: ServerId,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
    unreads: &Unreads,
    selected: Option<ConversationId>,
) -> Vec<DirectRow> {
    let own = model.account.id;
    let mut rows: Vec<DirectRow> = model
        .conversations
        .iter()
        .map(|conversation| {
            let other = if conversation.low_account_id == own {
                conversation.high_account_id
            } else {
                conversation.low_account_id
            };
            DirectRow {
                id: conversation.id,
                account_id: other,
                name: name_of(server_id, other, workspace, model, known),
                selected: selected == Some(conversation.id),
                unread: unreads.at(PlaceId::Direct(conversation.id)),
                mute: Mute::of_direct(other, model, known),
                departure: known.people.departure(server_id, other),
            }
        })
        .collect();
    rows.sort_by(|one, other| {
        let key = |row: &DirectRow| {
            (
                row.name.is_none(),
                row.name.as_ref().map(|name| name.to_lowercase()),
            )
        };
        key(one).cmp(&key(other)).then(one.id.cmp(&other.id))
    });
    rows
}

/// A feed whose messages offer no change: a direct conversation with an account that left
/// the server or was deleted (ADR 0121).
fn read_only_feed(mut feed: Feed) -> Feed {
    for row in Arc::make_mut(&mut feed.rows) {
        if let feed::FeedRow::Message(row) = row {
            row.can_react = false;
            row.can_reply = false;
            row.can_edit = false;
            row.can_delete = false;
            row.can_pin = false;
            row.can_thread = false;
        }
    }
    feed
}

/// The uploads of the waiting messages of one server, by the number of the upload.
fn uploads_of(server_id: ServerId, known: &Known) -> HashMap<u64, UploadProgress> {
    known
        .uploads
        .iter()
        .filter(|((of, _), _)| *of == server_id)
        .map(|((_, upload), progress)| (*upload, *progress))
        .collect()
}

/// The lists of a server that shows them: its direct conversations, its notices, and the
/// shown workspace when the account can see one (ADR 0741).
fn server_lists(
    server_id: ServerId,
    model: &ServerModel,
    selection: &Selection,
    known: &Known,
) -> ServerLists {
    let unreads = Unreads::of(model);
    let workspace = shown_workspace(model, selection.workspaces.get(&server_id).copied());
    // A direct conversation is shown in place of a channel while the state holds it.
    let conversation_id = selection
        .conversations
        .get(&server_id)
        .copied()
        .filter(|id| model.conversations.iter().any(|held| held.id == *id));
    let directs = directs(
        server_id,
        workspace,
        model,
        known,
        &unreads,
        conversation_id,
    );
    let conversation = directs.iter().find(|row| row.selected).cloned();
    // The direct-message call of this device in the shown conversation, unless the call
    // window draws the call.
    let direct_call = conversation
        .as_ref()
        .filter(|_| !known.call_in_own_window)
        .and_then(|row| {
            let active = known.media.calls.call.as_ref().filter(|active| {
                active.target.server_id == server_id
                    && active.target.place == CallPlace::direct(model.account.id, row.account_id)
            })?;
            Some(direct_panel(
                server_id,
                row.account_id,
                active,
                workspace,
                model,
                known,
            ))
        });
    let (shown, channel_feed, channel_call) = match workspace {
        Some(workspace) => {
            let parts = workspace_view(
                server_id,
                workspace,
                model,
                selection,
                known,
                &unreads,
                conversation.as_ref().map(|_| ConversationShown {
                    in_call: direct_call.is_some(),
                }),
            );
            (Some(parts.view), parts.feed, parts.call)
        }
        None => (None, None, None),
    };
    let call = channel_call.or(direct_call);
    // A direct conversation has no workspace: no role of one counts in it, and nobody
    // manages its messages (ADR 0302).
    let feed = match &conversation {
        Some(row) => model.timeline(PlaceId::Direct(row.id)).map(|timeline| {
            let feed = feed::derive(&feed::Source {
                server_id,
                model,
                timeline,
                names: Arc::new(names(server_id, workspace, model, known)),
                own_roles: &[],
                may_manage: false,
                administrator: model.is_administrator(),
                open_thread: None,
                target: target_of_jump(server_id, timeline, known),
                threads: true,
                quoted: &known.quoted,
                zone: known.zone,
                today: known.today,
                cards: None,
            });
            let feed = with_jump(feed, server_id, timeline, known);
            // A conversation with a departed account is read only (ADR 0121).
            if row.departure.is_some() {
                read_only_feed(feed)
            } else {
                feed
            }
        }),
        None => channel_feed,
    };
    let place = conversation
        .as_ref()
        .map(|row| PlaceId::Direct(row.id))
        .or(shown
            .as_ref()
            .and_then(|view| view.channel.as_ref())
            .map(|row| PlaceId::Channel(row.id)));
    // The notices of the server lie under a thread, under the pinned messages and under
    // a transcript. In the view of the call of this device they lie in the panel of the
    // call, which shows no thread, pin list or transcript (ADR 0742).
    let notices = selection
        .notices
        .filter(|of| *of == server_id)
        .filter(|_| {
            call.is_some()
                || shown.as_ref().is_none_or(|view| {
                    view.thread.is_none() && view.pins.is_none() && view.transcript.is_none()
                })
        })
        .map(|_| super::notices::notices_panel(server_id, model, known));
    ServerLists {
        workspace: shown,
        directs,
        conversation,
        conversation_refused: known
            .conversation_refused
            .as_ref()
            .filter(|refused| refused.server_id == server_id)
            .map(|refused| ConversationRefusal {
                account_id: refused.account_id,
                name: name_of(server_id, refused.account_id, workspace, model, known)
                    .or_else(|| refused.name.clone()),
                reason: refused.reason.clone(),
            }),
        conversation_asked: known
            .conversation_asked
            .filter(|(server, _)| *server == server_id)
            .map(|(_, account_id)| account_id),
        feed,
        draft: place
            .and_then(|place| known.drafts.get(&(server_id, place)).cloned())
            .unwrap_or_default(),
        uploads: Arc::new(uploads_of(server_id, known)),
        notices,
        unread_notices: super::notices::unread_notices(model),
        call,
    }
}

/// The direct conversation that the main area shows in place of a channel.
#[derive(Debug, Clone, Copy)]
struct ConversationShown {
    /// The main area shows the direct-message call of this device in it.
    in_call: bool,
}

/// What [`workspace_view`] derives for [`server_lists`].
struct WorkspaceParts {
    view: WorkspaceView,
    /// The messages of the shown channel.
    feed: Option<Feed>,
    /// The call of this device, when the shown channel is its voice channel.
    call: Option<CallPanel>,
}

/// The message of a jump of the window, in the place whose list shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpTarget {
    pub server_id: ServerId,
    pub place: PlaceId,
    pub message_id: MessageId,
    pub position: u64,
    /// The number of the jump: a second jump to the same message goes there again.
    pub number: u64,
    /// The list is to show the message: the person did not ask for the latest messages of
    /// the place since the jump.
    pub waiting: bool,
}

impl JumpTarget {
    fn of(server_id: ServerId, timeline: &Timeline, known: &Known) -> Option<Self> {
        known
            .target
            .filter(|target| target.server_id == server_id && target.place == timeline.place)
    }
}

/// The message that the person went to from a notice or a search result, with the number of
/// that jump, when the timeline is the place of that message.
fn target_of_jump(
    server_id: ServerId,
    timeline: &Timeline,
    known: &Known,
) -> Option<(MessageId, u64)> {
    JumpTarget::of(server_id, timeline, known).map(|target| (target.message_id, target.number))
}

/// The feed of a timeline, with the wait for the messages around the message of a jump:
/// the timeline holds neither the message nor the messages around its position.
fn with_jump(mut feed: Feed, server_id: ServerId, timeline: &Timeline, known: &Known) -> Feed {
    feed.jump_waits = JumpTarget::of(server_id, timeline, known).is_some_and(|target| {
        target.waiting
            && timeline.message(target.message_id).is_none()
            && !super::searching::covers(timeline, target.position)
    });
    feed
}

/// The lists of a workspace, and the messages and the call of its shown channel.
/// `conversation`: the main area shows a direct conversation in place of a channel.
fn workspace_view(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    selection: &Selection,
    known: &Known,
    unreads: &Unreads,
    conversation: Option<ConversationShown>,
) -> WorkspaceParts {
    let mut sections = sections(server_id, workspace, model, known, unreads);
    let wanted = selection
        .channels
        .get(&(server_id, workspace.workspace.id))
        .copied();
    // An archived channel is in no list. The main area shows it while the state knows it
    // and the person has it selected (ADR 0762).
    let archived = wanted
        .filter(|_| conversation.is_none())
        .and_then(|id| model.admin.archived_channel(id))
        .filter(|held| held.channel.workspace_id == workspace.workspace.id);
    let rows = || sections.iter().flat_map(|section| section.rows.iter());
    let shown = if conversation.is_some() {
        None
    } else if let Some(archived) = archived {
        Some(archived.channel.id)
    } else {
        wanted
            .filter(|id| rows().any(|row| row.id == *id))
            .or_else(|| {
                rows()
                    .find(|row| row.kind == ChannelKind::Text)
                    .map(|row| row.id)
            })
            .or_else(|| rows().next().map(|row| row.id))
    };
    // The thread of the side panel belongs to the shown channel.
    let open_thread = selection
        .thread
        .filter(|(server, channel_id, _)| *server == server_id && Some(*channel_id) == shown)
        .map(|(_, channel_id, root_id)| (channel_id, root_id));
    let names = Arc::new(names(server_id, Some(workspace), model, known));
    let mut channel = None;
    for row in sections
        .iter_mut()
        .flat_map(|section| section.rows.iter_mut())
    {
        let open = open_thread
            .filter(|(channel_id, _)| *channel_id == row.id)
            .map(|(_, root_id)| root_id);
        // The followed threads are listed under the shown channel only.
        let followed = (Some(row.id) == shown).then_some((server_id, &known.followed));
        (row.threads, row.more_followed) = thread_entries(row.id, model, &names, open, followed);
        // The view of the voice channel of the call of this device is the call.
        let in_call = row
            .voice
            .as_ref()
            .is_some_and(|voice| voice.relation == Relation::Inside);
        for entry in &mut row.threads {
            entry.available = !in_call;
        }
        if Some(row.id) == shown {
            row.selected = true;
            channel = Some(row.clone());
        }
    }
    if let (None, Some(archived)) = (&channel, archived) {
        let channel_id = archived.channel.id;
        let open = open_thread
            .filter(|(of, _)| *of == channel_id)
            .map(|(_, root_id)| root_id);
        let followed = Some((server_id, &known.followed));
        let (threads, more_followed) = thread_entries(channel_id, model, &names, open, followed);
        // A call of an archived voice channel has ended: the row has no voice state, and
        // the main area shows its messages.
        channel = Some(ChannelRow {
            id: channel_id,
            kind: archived.channel.kind,
            name: archived.channel.name.clone(),
            selected: true,
            voice: None,
            unread: unreads.at(PlaceId::Channel(channel_id)),
            permanent: archived.channel.permanent_messages,
            mute: Mute::of_channel(channel_id, model, known),
            threads,
            more_followed,
            // No snapshot states the list of an archived channel: nothing is offered
            // until its own read answered (ADR 0310).
            allowed: allowed(model.permissions_in(channel_id).unwrap_or_default()),
        });
    }
    let call = channel
        .as_ref()
        .filter(|row| {
            row.voice
                .as_ref()
                .is_some_and(|voice| voice.relation == Relation::Inside)
        })
        .zip(known.media.calls.call.as_ref())
        .map(|(row, active)| call_panel(server_id, row, active, workspace, model, known));
    let allowed = channel
        .as_ref()
        .map_or_else(|| allowed(&workspace.permissions), |row| row.allowed);
    let accounts = super::notices::account_names(server_id, model, known);
    let cards = channel
        .as_ref()
        .filter(|row| row.kind == ChannelKind::Voice && call.is_none())
        .map(|row| feed::Cards {
            channel: row.name.as_str(),
            channel_id: row.id,
            accounts: &accounts,
            may_remove: allowed.manage_recordings && archived.is_none(),
            downloads: &known.segment_downloads,
            texts: &known.text_downloads,
            removing: &known.recordings_removing,
            requesting: &known.transcripts_requesting,
            refused: &known.recording_refused,
        });
    let source = |timeline| feed::Source {
        server_id,
        model,
        timeline,
        names: names.clone(),
        own_roles: &workspace.own_role_ids,
        may_manage: allowed.manage_messages,
        administrator: model.is_administrator(),
        open_thread: open_thread.map(|(_, root_id)| root_id),
        target: target_of_jump(server_id, timeline, known),
        threads: call.is_none(),
        quoted: &known.quoted,
        zone: known.zone,
        today: known.today,
        cards: cards.as_ref(),
    };
    let feed = channel
        .as_ref()
        .and_then(|row| model.timeline(PlaceId::Channel(row.id)))
        .map(|timeline| with_jump(feed::derive(&source(timeline)), server_id, timeline, known));
    // An archived channel takes no change (ADR 0410): its messages offer none, in the
    // main area, in a thread and in the pin list.
    let read_only = archived.is_some();
    let fixed = |mut row: MessageRow| {
        if read_only {
            row.can_react = false;
            row.can_reply = false;
            row.can_edit = false;
            row.can_delete = false;
            row.can_pin = false;
            row.can_thread = false;
        }
        row
    };
    let fixed_feed = |mut feed: Feed| {
        if read_only {
            for row in Arc::make_mut(&mut feed.rows) {
                if let feed::FeedRow::Message(row) = row {
                    *row = fixed(row.clone());
                }
            }
        }
        feed
    };
    let feed = feed.map(fixed_feed);
    let draft_of = |place: PlaceId| {
        known
            .drafts
            .get(&(server_id, place))
            .cloned()
            .unwrap_or_default()
    };
    // The side panel of the call of this device is the panel of the call: its chat
    // shows no thread and no pin list.
    let thread = open_thread
        .filter(|_| call.is_none())
        .zip(channel.as_ref())
        .map(|((channel_id, root_id), row)| {
            let place = PlaceId::Thread {
                channel_id,
                root_id,
            };
            let timeline = model.timeline(place);
            ThreadPanel {
                channel_id,
                channel_name: row.name.clone(),
                root_id,
                root: timeline
                    .and_then(|timeline| Some((timeline, timeline.root.as_ref()?)))
                    .map(|(timeline, root)| {
                        fixed(feed::message_row(
                            &source(timeline),
                            root,
                            &mut feed::Loaded::default(),
                        ))
                    }),
                feed: timeline.map(|timeline| {
                    fixed_feed(with_jump(
                        feed::derive(&source(timeline)),
                        server_id,
                        timeline,
                        known,
                    ))
                }),
                forks: timeline
                    .map(|timeline| timeline.forks.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|fork| {
                        let held = model.channel(*fork)?;
                        Some((*fork, held.channel.name.clone(), held.channel.kind))
                    })
                    .collect(),
                may_fork: allowed.manage && !read_only,
                draft: draft_of(place),
                fork: known
                    .fork
                    .as_ref()
                    .filter(|asked| asked.server_id == server_id && asked.root_id == root_id)
                    .map(|asked| asked.state.clone()),
                permanent: row.permanent,
                follow: known.followed.thread.control(server_id, root_id, read_only),
            }
        });
    let pins = selection
        .pins
        .filter(|(server, channel_id)| *server == server_id && Some(*channel_id) == shown)
        .filter(|_| thread.is_none() && call.is_none())
        .zip(channel.as_ref())
        .map(|((_, channel_id), row)| {
            let list = model.pin_list(channel_id);
            let timeline = model.timeline(PlaceId::Channel(channel_id));
            let state = match (list, timeline) {
                (Some(list), Some(timeline)) => match (&list.messages, list.failed) {
                    (Loadable::Loaded(pinned), _) => {
                        let source = source(timeline);
                        let mut loaded = feed::Loaded::default();
                        PinsState::Loaded(
                            pinned
                                .iter()
                                .map(|message| {
                                    fixed(feed::message_row(&source, message, &mut loaded))
                                })
                                .collect(),
                        )
                    }
                    (_, Some(_)) => PinsState::Failed,
                    _ => PinsState::Loading,
                },
                _ => PinsState::Loading,
            };
            PinsPanel {
                channel_id,
                channel_name: row.name.clone(),
                state,
                names: names.clone(),
            }
        });
    // The transcript of a recording of the shown voice channel.
    let transcript = selection
        .transcript
        .filter(|(of, channel_id, _)| *of == server_id && Some(*channel_id) == shown)
        .filter(|_| thread.is_none() && pins.is_none() && call.is_none())
        .zip(channel.as_ref())
        .map(|((_, _, recording_id), row)| TranscriptPanel {
            recording_id,
            channel: row.name.clone(),
            state: match model.recording(recording_id) {
                None => TranscriptPanelState::Reading,
                Some(open) if open.gone => TranscriptPanelState::Gone,
                // A recording of another channel than the shown one is not shown
                // (ADR 0714).
                Some(open)
                    if open
                        .detail
                        .loaded()
                        .is_some_and(|detail| detail.recording.channel_id != row.id) =>
                {
                    TranscriptPanelState::Gone
                }
                Some(open) => match (&open.transcript, open.failed) {
                    (Loadable::Loaded(_), _) => {
                        let when = super::recorded::When {
                            zone: known.zone,
                            today: known.today,
                        };
                        let source = super::recorded::TranscriptSource {
                            names: &accounts,
                            when,
                            // A recording of an archived channel takes no change
                            // (ADR 0410).
                            may_change: archived.is_none(),
                            ask: known.transcript_asks.get(&(server_id, recording_id)),
                        };
                        TranscriptPanelState::Shown {
                            transcript: Box::new(super::recorded::transcript(open, source)),
                            open: open.clone(),
                            names: Arc::new(accounts.clone()),
                            when,
                        }
                    }
                    (_, Some(_)) => TranscriptPanelState::Failed,
                    _ => TranscriptPanelState::Reading,
                },
            },
            line: known.transcript_line,
            jump: known.transcript_jump,
        });
    // The notices of the server (`server_lists`) lie under a thread, under the pinned
    // messages and under a transcript, and over a search.
    let notices_wanted = selection.notices == Some(server_id);
    let in_call = call.is_some() || conversation.is_some_and(|shown| shown.in_call);
    let (no_thread, no_pins) = (thread.is_none(), pins.is_none());
    let view = WorkspaceView {
        id: workspace.workspace.id,
        name: workspace.workspace.name.clone(),
        sections,
        channel,
        members: members(server_id, workspace, model, known),
        allowed,
        roles: workspace
            .roles
            .iter()
            .filter(|role| !role.is_everyone)
            .map(|role| (role.id, crate::text::label(&role.name)))
            .collect(),
        thread,
        pins,
        search: super::search::search_panel(server_id, workspace, model, known).filter(|_| {
            no_thread && no_pins && transcript.is_none() && !notices_wanted && !in_call
        }),
        transcript,
        archived: archived
            .filter(|held| workspace.channel(held.channel.id).is_none())
            .map(|held| ArchivedState {
                archived_at: known.zone.local(held.archived_at),
                deletes_at: held.deletes_at.map(|at| known.zone.local(at)),
                may_restore: allowed.manage,
            }),
    };
    WorkspaceParts { view, feed, call }
}

/// The names that the messages of a place can mention: the members of the shown workspace,
/// the accounts whose names were read, the account itself, and the roles of the workspace.
fn names(
    server_id: ServerId,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
) -> Names {
    let mut accounts: HashMap<AccountId, String> = known.people.names(server_id).collect();
    if let Some(members) = workspace.and_then(|workspace| workspace.members.loaded()) {
        accounts.extend(
            members
                .iter()
                .map(|member| (member.account_id, crate::text::label(&member.name))),
        );
    }
    accounts.insert(model.account.id, crate::text::label(&model.account.name));
    Names {
        accounts,
        roles: workspace
            .map(|workspace| workspace.roles.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|role| (role.id, crate::text::label(&role.name)))
            .collect(),
        departed: known.people.departures(server_id),
    }
}

/// What the channels of a workspace hold that the person has not read.
fn workspace_unread(workspace: &WorkspaceModel, unreads: &Unreads) -> Unread {
    let mut unread = Unread::default();
    for channel in &workspace.channels {
        unread.add(unreads.at(PlaceId::Channel(channel.channel.id)));
    }
    unread
}

/// What a server holds unread that no row of a workspace shows: its direct conversations,
/// and every place when the server lists no workspace.
fn head_unread(model: &ServerModel, unreads: &Unreads) -> Unread {
    if model.workspaces.is_empty() {
        return unreads.total();
    }
    let mut unread = Unread::default();
    for (place, held) in &unreads.0 {
        if let PlaceId::Direct(_) = place {
            unread.add(*held);
        }
    }
    unread
}

/// The devices of one kind as the sound settings list them: the default of the system
/// first, then each connected device. A chosen device that is not connected is listed too,
/// so that the person sees what is stored.
fn device_choices(connected: &[DeviceInfo], chosen: Option<&str>) -> Vec<DeviceChoice> {
    let default = connected.iter().find(|device| device.is_default);
    let mut choices = vec![DeviceChoice {
        id: None,
        name: default
            .map(|device| device.name.clone())
            .unwrap_or_default(),
        selected: chosen.is_none(),
    }];
    choices.extend(connected.iter().map(|device| DeviceChoice {
        id: Some(device.id.0.clone()),
        name: device.name.clone(),
        selected: chosen == Some(device.id.0.as_str()),
    }));
    if let Some(chosen) = chosen
        && !connected.iter().any(|device| device.id.0 == chosen)
    {
        choices.push(DeviceChoice {
            id: Some(chosen.to_owned()),
            name: String::new(),
            selected: true,
        });
    }
    choices
}

/// The numbers of shares of one kind of one person, while the person runs two or more of
/// that kind: `keys` holds the person and the kind of each share, in the order of the
/// shares.
fn share_numbers<K: PartialEq>(keys: &[K]) -> Vec<Option<usize>> {
    keys.iter()
        .enumerate()
        .map(|(index, key)| {
            let same = keys.iter().filter(|other| *other == key).count();
            let before = keys[..index].iter().filter(|other| *other == key).count();
            (same > 1).then_some(before + 1)
        })
        .collect()
}

/// The shares of a call: the ones of the other people as the media connection shows them,
/// with the sound that the server tells of each, and the ones of this device in the order
/// of their slots.
fn share_stage(
    server_id: ServerId,
    active: &ActiveCall,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
) -> Option<ShareStage> {
    let call = model.call_at(&active.target.place);
    let name_of = |account_id: AccountId| name_of(server_id, account_id, workspace, model, known);
    let mut shares: Vec<ShareCard> = active
        .shares
        .iter()
        .map(|seen| {
            let id = ShareId::of(seen.account_id, seen.slot);
            ShareCard {
                id,
                account_id: seen.account_id,
                name: name_of(seen.account_id),
                kind: seen.kind,
                number: None,
                sound: call
                    .and_then(|call| call.participant(seen.account_id))
                    .and_then(|participant| {
                        participant
                            .shares()
                            .iter()
                            .find(|share| share.slot == seen.slot)
                    })
                    .map(|share| share.audio),
                sound_held: false,
                size: seen.size,
                frame_rate: seen.frame_rate,
                watched: active.watched == Some(id),
            }
        })
        .collect();
    for (slot, share) in &active.own_shares {
        if let ShareState::Running(info) = share {
            let id = ShareId::own(*slot);
            shares.push(ShareCard {
                id,
                account_id: model.account.id,
                name: name_of(model.account.id),
                kind: Some(info.kind),
                number: None,
                sound: Some(info.sound),
                sound_held: info.sound_held,
                size: Some(info.size),
                frame_rate: Some(info.frame_rate),
                watched: active.watched == Some(id),
            });
        }
    }
    let keys: Vec<(junto_media::engine::Person, Option<ShareKind>)> = shares
        .iter()
        .map(|share| (share.id.person, share.kind))
        .collect();
    for (share, number) in shares.iter_mut().zip(share_numbers(&keys)) {
        share.number = number;
    }
    (!shares.is_empty()).then_some(ShareStage {
        shares,
        volume: known.media.voice.share_volume,
    })
}

/// The shares of this device for the menu of the share button, in the order of their slots.
fn own_share_items(active: &ActiveCall) -> Vec<OwnShareItem> {
    let kinds: Vec<Option<ShareKind>> = active
        .own_shares
        .values()
        .map(|share| match share {
            ShareState::Running(info) => Some(info.kind),
            ShareState::Starting => None,
        })
        .collect();
    let numbers = share_numbers(&kinds);
    active
        .own_shares
        .keys()
        .zip(kinds)
        .zip(numbers)
        .map(|((slot, kind), number)| OwnShareItem {
            slot: *slot,
            kind,
            number: number.filter(|_| kind.is_some()),
        })
        .collect()
}

/// What a recording that ended with a problem tells the people of its call.
fn recording_problem(recording: &Recording) -> Option<RecordingProblem> {
    let stopped = recording.stop.as_ref().map(|stop| stop.reason);
    let fault = match (recording.state, stopped, recording.failure) {
        (RecordingState::Failed, _, Some(RecordingFailure::RecorderUnavailable)) => {
            RecordingFault::RecorderUnavailable
        }
        (RecordingState::Failed, _, _) => RecordingFault::NothingSaved,
        (_, Some(RecordingStopReason::RecorderLost), _) => RecordingFault::RecorderLost,
        (_, Some(RecordingStopReason::StorageReserve), _) => RecordingFault::StorageReserve,
        (_, Some(RecordingStopReason::SegmentLimit), _) => RecordingFault::SegmentLimit,
        (_, Some(RecordingStopReason::Unknown), _) => RecordingFault::Other,
        (RecordingState::PartlyFailed, _, _) => RecordingFault::PartlySaved,
        // A person stopped it, or the call ended, and nothing failed.
        _ => return None,
    };
    Some(RecordingProblem {
        fault,
        since: recording.recording_since,
        until: recording
            .stop
            .as_ref()
            .map(|stop| stop.at)
            .or(recording.ended_at),
        segments: recording.segments,
        failed_segments: recording.failed_segments,
    })
}

/// Whether a recording is one that the header of its call tells as a problem.
pub fn is_recording_problem(recording: &Recording) -> bool {
    recording_problem(recording).is_some()
}

fn recording_control(
    server_id: ServerId,
    call_id: Option<CallId>,
    running: Option<&Recording>,
    may_record: bool,
    known: &Known,
) -> RecordingControl {
    let state = match running {
        Some(recording) if recording.state == RecordingState::Starting => RecordingShown::Starting,
        Some(recording) => RecordingShown::Running {
            elapsed: recording
                .recording_since
                .zip(known.media.now)
                .map(|(since, now)| Duration::try_from(now - since).unwrap_or_default()),
            stopping: recording.stop.is_some(),
        },
        None => known
            .recording_problem
            .as_ref()
            .filter(|(of, recording)| *of == server_id && Some(recording.call_id) == call_id)
            .and_then(|(_, recording)| recording_problem(recording))
            .map_or(RecordingShown::None, RecordingShown::Failed),
    };
    RecordingControl {
        state,
        may_record,
        busy: known.recording_busy,
    }
}

fn call_panel(
    server_id: ServerId,
    row: &ChannelRow,
    active: &ActiveCall,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> CallPanel {
    let call = model.call_at(&active.target.place);
    let inside = |account_id: AccountId| {
        row.voice.as_ref().is_some_and(|voice| {
            voice
                .occupants
                .iter()
                .any(|held| held.account_id == account_id)
        })
    };
    // The voice channel that a member is in, from the calls that the account may see.
    let place_of = |account_id: AccountId| {
        workspace.channels.iter().find_map(|channel| {
            channel
                .call
                .as_ref()
                .filter(|call| call.participant(account_id).is_some())
                .map(|_| channel.channel.name.clone())
        })
    };
    let mut outside: Vec<OutsideMember> = workspace
        .members
        .loaded()
        .map(|members| {
            members
                .iter()
                .filter(|member| !inside(member.account_id))
                .map(|member| OutsideMember {
                    account_id: member.account_id,
                    name: crate::text::label(&member.name),
                    place: place_of(member.account_id),
                    invitation: model
                        .invitations
                        .iter()
                        .find(|invitation| {
                            invitation.account_id == member.account_id
                                && invitation.invited_by == model.account.id
                                && invitation.place == active.target.place
                        })
                        .map(|invitation| invitation.id),
                })
                .collect()
        })
        .unwrap_or_default();
    outside.sort_by(|one, other| {
        one.name
            .to_lowercase()
            .cmp(&other.name.to_lowercase())
            .then(one.account_id.cmp(&other.account_id))
    });
    let people = row
        .voice
        .as_ref()
        .map(|voice| voice.occupants.clone())
        .unwrap_or_default();
    let place = PanelPlace::Channel(Box::new(ChannelCall {
        name: row.name.clone(),
        outside: if active.phase == Phase::Connected {
            outside
        } else {
            Vec::new()
        },
        recording: recording_control(
            server_id,
            call.map(|call| call.id),
            call.and_then(|call| call.recording.as_ref()),
            row.allowed.record,
            known,
        ),
        room: room_view(server_id, row, active, workspace, model, known),
    }));
    media_panel(
        server_id,
        active,
        Some(workspace),
        model,
        known,
        people,
        place,
    )
}

/// The call of this device in the direct conversation with `other` that the main area
/// shows.
fn direct_panel(
    server_id: ServerId,
    other: AccountId,
    active: &ActiveCall,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
) -> CallPanel {
    let call = model.call_at(&active.target.place);
    let people = call_people(server_id, call, Some(active), workspace, model, known);
    let place = PanelPlace::Direct(DirectCall {
        other,
        name: name_of(server_id, other, workspace, model, known),
        joined: call.is_some_and(|call| call.participant(other).is_some()),
    });
    media_panel(server_id, active, workspace, model, known, people, place)
}

/// The panel of a call with its people and its place, and what every call has: its phase,
/// its sound settings, its shares and the settings of the view and of the own share.
fn media_panel(
    server_id: ServerId,
    active: &ActiveCall,
    workspace: Option<&WorkspaceModel>,
    model: &ServerModel,
    known: &Known,
    people: Vec<Occupant>,
    place: PanelPlace,
) -> CallPanel {
    let media = &known.media;
    CallPanel {
        phase: active.phase,
        interrupted_at: active.interrupted_at,
        people,
        audio: AudioSettings {
            inputs: device_choices(&media.devices.inputs, media.voice.input_device.as_deref()),
            outputs: device_choices(&media.devices.outputs, media.voice.output_device.as_deref()),
            input_level: media.levels.input,
            microphone: match &active.microphone {
                MicrophoneState::Closed => MicrophoneLine::Closed,
                MicrophoneState::Asking => MicrophoneLine::Asking,
                MicrophoneState::Running => MicrophoneLine::Running,
                MicrophoneState::NoPermission => MicrophoneLine::NoPermission,
                MicrophoneState::Failed(_) => MicrophoneLine::Failed,
            },
            access: active.access,
            output_failed: matches!(active.output, OutputState::Failed(_)),
        },
        stage: share_stage(server_id, active, workspace, model, known),
        hide_without_video: media.voice.hide_people_without_video,
        share_resolution: media.voice.share_resolution,
        share_frame_rate: media.voice.share_frame_rate,
        place,
    }
}

/// Meeting mode is on, another person runs it, the microphone of the person of this device
/// has no block, and the person's own setting is "closed": the person may speak and opens
/// the microphone (ADR 0505).
fn may_speak(
    active: &ActiveCall,
    call: Option<&junto_client_core::model::CallModel>,
    model: Option<&ServerModel>,
    known: &Known,
) -> bool {
    active.phase == Phase::Connected
        && call
            .and_then(|call| call.meeting.as_ref())
            .zip(model)
            .is_some_and(|(meeting, model)| meeting.presenter != model.account.id)
        && active.microphone_block.is_none()
        && known.media.calls.muted
}

/// What a list of permissions that the server stated allows (ADR 0310).
fn allowed(held: &[WorkspacePermission]) -> Allowed {
    let may = |permission| held.contains(&permission);
    Allowed {
        manage_messages: may(WorkspacePermission::ManageMessages),
        mention_everyone: may(WorkspacePermission::MentionEveryone),
        lock: may(WorkspacePermission::LockRoom),
        run_meeting: may(WorkspacePermission::RunMeetingMode),
        manage: may(WorkspacePermission::ManageChannel),
        record: may(WorkspacePermission::RecordCalls),
        mute: may(WorkspacePermission::MuteMembers),
        deafen: may(WorkspacePermission::DeafenMembers),
        disconnect: may(WorkspacePermission::DisconnectMembers),
        move_members: may(WorkspacePermission::MoveMembers),
        manage_recordings: may(WorkspacePermission::ManageRecordings),
    }
}

fn room_view(
    server_id: ServerId,
    row: &ChannelRow,
    active: &ActiveCall,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> RoomView {
    let call = model.call_at(&active.target.place);
    let name_of =
        |account_id: AccountId| name_of(server_id, account_id, Some(workspace), model, known);
    let own = model.account.id;
    let meeting = call
        .filter(|_| active.phase == Phase::Connected)
        .and_then(|call| call.meeting.as_ref())
        .map(|meeting| {
            let by_self = meeting.presenter == own;
            MeetingView {
                by_self,
                presenter: name_of(meeting.presenter),
                hands: call
                    .map(|call| call.participants.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter(|participant| {
                        by_self && participant.hand_raised && participant.microphone_block.is_some()
                    })
                    .map(|participant| (participant.account_id, name_of(participant.account_id)))
                    .collect(),
                waiting: call
                    .map(|call| call.participants.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter(|participant| {
                        by_self
                            && participant.account_id != own
                            && participant.microphone_block.is_none()
                            && participant.muted
                    })
                    .map(|participant| name_of(participant.account_id))
                    .collect(),
                may_speak: may_speak(active, call, Some(model), known),
            }
        });
    let voice = row.voice.as_ref();
    let allowed = row.allowed;
    let hides = active
        .target
        .place
        .channel_id()
        .and_then(|channel_id| model.channel(channel_id))
        .is_some_and(|channel| channel.channel.hides_occupants);
    RoomView {
        ready: active.phase == Phase::Connected,
        name: row.name.clone(),
        inside: voice.map_or(0, |voice| voice.occupants.len()),
        meeting_start: if !allowed.run_meeting {
            MeetingStart::NoPermission
        } else if active.microphone_block.is_some() {
            MeetingStart::MicrophoneBlocked
        } else if !active
            .own_shares
            .values()
            .any(|share| matches!(share, ShareState::Running(_)))
        {
            MeetingStart::NotSharing
        } else {
            MeetingStart::Possible
        },
        meeting,
        locked: voice.is_some_and(|voice| voice.locked),
        user_limit: voice.and_then(|voice| voice.user_limit),
        hides_occupants: hides,
        hide_waits_for_next_call: call.is_some_and(|call| call.hides_occupants != hides),
        allowed,
    }
}

/// The invitations that wait for the answer of the person, on every server.
fn invitations(servers: &[ServerSnapshot], known: &Known) -> Vec<Invited> {
    let mut waiting = Vec::new();
    for server in servers {
        let Some(model) = shown_model(server) else {
            continue;
        };
        for invitation in model.incoming_invitations() {
            let target = CallTarget {
                server_id: server.id,
                place: invitation.place,
            };
            let workspace = match invitation.place {
                CallPlace::Channel { workspace_id, .. } => model.workspace(workspace_id),
                CallPlace::Direct { .. } => None,
            };
            let by = workspace
                .and_then(|workspace| {
                    name_of(
                        server.id,
                        invitation.invited_by,
                        Some(workspace),
                        model,
                        known,
                    )
                })
                .or_else(|| {
                    model.workspaces.iter().find_map(|workspace| {
                        name_of(
                            server.id,
                            invitation.invited_by,
                            Some(workspace),
                            model,
                            known,
                        )
                    })
                })
                .or_else(|| known.people.name(server.id, invitation.invited_by));
            waiting.push(Invited {
                id: invitation.id,
                caller: invitation.invited_by,
                target,
                by,
                channel: channel_of(servers, &target),
                created_at: invitation.created_at,
            });
        }
    }
    waiting
}

/// The people outside who ask to enter the call of this device (ADR 0514).
fn entry_prompts(servers: &[ServerSnapshot], known: &Known) -> Vec<EntryPrompt> {
    let Some(active) = known.media.calls.call.as_ref() else {
        return Vec::new();
    };
    if active.phase != Phase::Connected {
        return Vec::new();
    }
    let Some((server, model)) = servers
        .iter()
        .find(|server| server.id == active.target.server_id)
        .and_then(|server| Some((server, shown_model(server)?)))
    else {
        return Vec::new();
    };
    let workspace = match active.target.place {
        CallPlace::Channel { workspace_id, .. } => model.workspace(workspace_id),
        CallPlace::Direct { .. } => None,
    };
    model
        .entry_requests
        .iter()
        .filter(|request| {
            request.place == active.target.place && request.account_id != model.account.id
        })
        .map(|request| EntryPrompt {
            id: request.id,
            account_id: request.account_id,
            name: workspace
                .and_then(|workspace| {
                    name_of(server.id, request.account_id, Some(workspace), model, known)
                })
                .or_else(|| known.people.name(server.id, request.account_id)),
            channel: channel_of(servers, &active.target),
        })
        .collect()
}

/// The name of the voice channel of a call, from the state of its server.
fn channel_of(servers: &[ServerSnapshot], target: &CallTarget) -> Option<String> {
    let server = servers
        .iter()
        .find(|server| server.id == target.server_id)?;
    let channel_id = target.place.channel_id()?;
    shown_model(server)?
        .channel(channel_id)
        .map(|channel| channel.channel.name.clone())
}

fn controls(servers: &[ServerSnapshot], known: &Known) -> Controls {
    let calls = &known.media.calls;
    let active = calls.call.as_ref();
    let name = |account_id: AccountId| {
        let active = active?;
        let server = servers
            .iter()
            .find(|server| server.id == active.target.server_id)?;
        let model = shown_model(server)?;
        let workspace = match active.target.place {
            CallPlace::Channel { workspace_id, .. } => model.workspace(workspace_id)?,
            CallPlace::Direct { .. } => return None,
        };
        name_of(server.id, account_id, Some(workspace), model, known)
    };
    let held = active.and_then(|call| call.microphone_block.as_ref());
    let warning = if let Some(block) = held {
        Some(SoundWarning::MicrophoneHeld {
            reason: block.reason,
            by: name(block.by),
        })
    } else {
        match active.map(|call| (&call.microphone, &call.output, &call.sound_block)) {
            Some((MicrophoneState::NoPermission, _, _)) => Some(SoundWarning::NoMicrophoneAccess),
            Some((MicrophoneState::Failed(_), _, _)) => Some(SoundWarning::MicrophoneFailed),
            Some((_, _, Some(block))) => Some(SoundWarning::SoundHeld { by: name(block.by) }),
            Some((_, OutputState::Failed(_), _)) => Some(SoundWarning::OutputFailed),
            _ => None,
        }
    };
    let seat = active.map(ActiveCall::seat);
    Controls {
        microphone: if held.is_some() {
            MicrophoneButton::Held
        } else if calls.muted {
            MicrophoneButton::Muted
        } else {
            MicrophoneButton::Open
        },
        microphone_action: if calls.muted {
            MicrophoneAction::Open(seat.clone())
        } else {
            MicrophoneAction::Close
        },
        sound_off: calls.deafened,
        warning,
        seat,
    }
}

fn call_strip(servers: &[ServerSnapshot], known: &Known, shown: Shown) -> Option<CallStrip> {
    let active = known.media.calls.call.as_ref()?;
    let (workspace_id, channel_id) = match active.target.place {
        CallPlace::Channel {
            workspace_id,
            channel_id,
        } => (Some(workspace_id), Some(channel_id)),
        CallPlace::Direct { .. } => (None, None),
    };
    let model = servers
        .iter()
        .find(|server| server.id == active.target.server_id)
        .and_then(shown_model);
    let call = model.and_then(|model| model.call_at(&active.target.place));
    let started_at = call.and_then(|call| call.started_at);
    Some(CallStrip {
        server_id: active.target.server_id,
        workspace_id,
        channel_id,
        name: channel_of(servers, &active.target).or_else(|| {
            let CallPlace::Direct {
                low_account_id,
                high_account_id,
            } = active.target.place
            else {
                return None;
            };
            let other = model.map(|model| {
                if model.account.id == low_account_id {
                    high_account_id
                } else {
                    low_account_id
                }
            })?;
            model
                .and_then(|model| {
                    model.workspaces.iter().find_map(|workspace| {
                        name_of(
                            active.target.server_id,
                            other,
                            Some(workspace),
                            model,
                            known,
                        )
                    })
                })
                .or_else(|| known.people.name(active.target.server_id, other))
        }),
        phase: active.phase,
        elapsed: started_at.zip(known.media.now).map(|(started_at, now)| {
            // The clock of the server and the clock of this machine can differ.
            Duration::try_from(now - started_at).unwrap_or_default()
        }),
        shown: shown.server == Some(active.target.server_id)
            && channel_id.is_some()
            && shown.channel == channel_id,
        share: if active
            .own_shares
            .values()
            .any(|share| matches!(share, ShareState::Running(_)))
        {
            OwnShare::Running
        } else if active.own_shares.is_empty() {
            OwnShare::Off
        } else {
            OwnShare::Starting
        },
        own_shares: own_share_items(active),
        share_limit: active.share_limit,
        can_share: known.media.can_share,
        hand_raised: channel_id
            .zip(model)
            .filter(|_| active.phase == Phase::Connected)
            .map(|(_, model)| {
                call.and_then(|call| call.participant(model.account.id))
                    .is_some_and(|own| own.hand_raised)
            }),
        may_speak: may_speak(active, call, model, known),
    })
}

fn call_notice(servers: &[ServerSnapshot], known: &Known) -> Option<CallNotice> {
    match known.media.calls.outcome.as_ref()? {
        Outcome::JoinFailed { target, error }
            if matches!(target.place, CallPlace::Direct { .. }) =>
        {
            Some(CallNotice::DirectJoinFailed {
                error: error.clone(),
            })
        }
        Outcome::JoinFailed { target, error } => Some(CallNotice::JoinFailed {
            channel: channel_of(servers, target),
            error: error.clone(),
        }),
        // A person who left a call by an action of their own, on this device or by
        // entering another call, is told nothing.
        Outcome::Ended {
            reason: EndReason::Server(Some(LeaveReason::Left | LeaveReason::Switched)),
            ..
        } => None,
        Outcome::Ended { target, reason } if matches!(target.place, CallPlace::Direct { .. }) => {
            Some(CallNotice::DirectEnded {
                reason: reason.clone(),
            })
        }
        Outcome::Ended { target, reason } => Some(CallNotice::Ended {
            channel: channel_of(servers, target),
            reason: reason.clone(),
        }),
        Outcome::Moved { target } => Some(CallNotice::Moved {
            channel: channel_of(servers, target),
        }),
    }
}

/// The view of the whole window.
pub fn derive(servers: &[ServerSnapshot], selection: &Selection, known: &Known) -> ShellView {
    let selected_id = selection
        .server
        .filter(|id| servers.iter().any(|server| server.id == *id))
        .or_else(|| servers.first().map(|server| server.id));
    let rows = servers
        .iter()
        .map(|server| {
            let status = status(server);
            let selected = Some(server.id) == selected_id;
            let unreads = shown_model(server)
                .filter(|_| status.shows_content())
                .map(|model| (model, Unreads::of(model)));
            let shown = shown_model(server).and_then(|model| {
                shown_workspace(model, selection.workspaces.get(&server.id).copied())
            });
            ServerRow {
                id: server.id,
                name: server.name.clone(),
                address: server.address.clone(),
                logo: known.logos.image(server.id),
                selected,
                has_recovery: matches!(
                    &server.access,
                    AccessState::LoggedIn { recoveries, .. } if !recoveries.is_empty()
                ),
                unread: unreads
                    .as_ref()
                    .map(|(_, unreads)| unreads.total())
                    .unwrap_or_default(),
                head_unread: unreads
                    .as_ref()
                    .map(|(model, unreads)| head_unread(model, unreads))
                    .unwrap_or_default(),
                waiting_join_requests: unreads.as_ref().map_or(0, |(model, _)| {
                    super::admin::view::waiting_join_requests(model)
                }),
                workspaces: unreads
                    .as_ref()
                    .map(|(model, unreads)| {
                        model
                            .workspaces
                            .iter()
                            .map(|workspace| WorkspaceRow {
                                unread: workspace_unread(workspace, unreads),
                                id: workspace.workspace.id,
                                name: workspace.workspace.name.clone(),
                                selected: selected
                                    && shown.is_some_and(|shown| {
                                        shown.workspace.id == workspace.workspace.id
                                    }),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                status,
            }
        })
        .collect();
    let server = servers.iter().find(|server| Some(server.id) == selected_id);
    let selected = server.map(|server| {
        let status = status(server);
        let content = match shown_model(server).filter(|_| status.shows_content()) {
            None => Content::State,
            Some(model) => {
                Content::Lists(Box::new(server_lists(server.id, model, selection, known)))
            }
        };
        SelectedServer {
            id: server.id,
            name: server.name.clone(),
            address: server.address.clone(),
            status,
            keep_connected: server.keep_connected,
            busy: server.in_progress.is_some(),
            can_log_out: junto_client_core::flow::access::actions(&server.access)
                .contains(&junto_client_core::flow::access::ActionKind::LogOutByPerson),
            error: server.error.clone(),
            recoveries: match &server.access {
                AccessState::LoggedIn { recoveries, .. } => recoveries.clone(),
                _ => Vec::new(),
            },
            account_changed: matches!(
                &server.access,
                AccessState::LoggedIn {
                    previous_account: Some(_),
                    ..
                }
            ),
            moderation: super::moderation::lines(server, known),
            moderation_failed: known
                .moderation_away_failed
                .as_ref()
                .filter(|(of, _)| *of == server.id)
                .map(|(_, reason)| reason.clone()),
            release: server.server_version.as_ref().map(|version| ServerRelease {
                version: version.clone(),
                newer: super::book::newer_server_release(&known.updates, version),
            }),
            content,
        }
    });
    let account = server.and_then(shown_model).map(|model| Person {
        account_id: Some(model.account.id),
        name: crate::text::label(&model.account.name),
    });
    let mut view = ShellView {
        servers: rows,
        selected,
        person: account.unwrap_or_else(|| Person {
            account_id: None,
            name: known.preset_name.clone(),
        }),
        controls: controls(servers, known),
        call: None,
        call_notice: call_notice(servers, known),
        invitations: invitations(servers, known),
        entry_prompts: entry_prompts(servers, known),
        downloads: Arc::new(known.downloads.clone()),
        notify: selection.notify.and_then(|(server_id, scope)| {
            let server = servers.iter().find(|server| server.id == server_id)?;
            let model = shown_model(server).filter(|_| status(server).shows_content())?;
            super::notices::notify_panel(server_id, &server.name, scope, model, known)
        }),
        quiet: {
            let with_session: Vec<(ServerId, &str, &ServerModel)> = servers
                .iter()
                .filter_map(|server| Some((server.id, server.name.as_str(), shown_model(server)?)))
                .collect();
            let names: Vec<(ServerId, &str)> = servers
                .iter()
                .map(|server| (server.id, server.name.as_str()))
                .collect();
            super::notices::quiet_view(&with_session, &names, selected_id, known)
        },
        search_text: known
            .search
            .as_ref()
            .filter(|search| Some(search.server_id) == selected_id)
            .map(|search| search.text.clone()),
        faces: Faces::default(),
        wanted_people: HashMap::new(),
        preset_picture: known.preset_picture.clone(),
        preset_refused: servers
            .iter()
            .filter(|server| known.preset_refused.contains(&server.id))
            .map(|server| (server.id, server.name.clone()))
            .collect(),
        update: super::book::available(&known.updates),
        share_notice: known
            .media
            .calls
            .call
            .as_ref()
            .filter(|_| !known.share_notice_dismissed)
            .and_then(|call| call.share_ended.clone()),
    };
    view.call = call_strip(servers, known, view.shown());
    view.faces = people::faces(servers, &known.people);
    view.wanted_people = people::wanted(servers, &view);
    view
}

#[cfg(test)]
mod book_tests;
#[cfg(test)]
mod followed_tests;
#[cfg(test)]
mod panel_tests;
#[cfg(test)]
mod people_tests;
#[cfg(test)]
mod permission_tests;
#[cfg(test)]
mod tests;
