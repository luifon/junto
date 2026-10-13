//! What the window shows, derived from the state of the client core (ADR 0730).
//!
//! [`derive`] is a function without side effects: it takes what the client core holds of
//! each saved server and what the person has selected, and returns plain data that the
//! drawing code turns into elements. The drawing code holds no rule about a server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::RenderImage;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessState, ActionKind, Notice};
use junto_client_core::live::{StreamFault, StreamStatus};
use junto_client_core::model::{ChannelModel, Loadable, PlaceId, ServerModel, WorkspaceModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::account::RecoveryNotice;
use junto_client_core::protocol::call::{BlockReason, CallPlace, LeaveReason, ShareAudio};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{
    AccountId, AttachmentId, CallId, CallInvitationId, ChannelId, ConversationId, EntryRequestId,
    MessageId, ServerId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::recording::{
    Recording, RecordingFailure, RecordingState, RecordingStopReason,
};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::transfers::{DownloadProgress, UploadProgress};
use junto_client_core::version::Incompatibility;
use junto_client_core::voice::{ShareResolution, VOLUME_DEFAULT_PERCENT, VoiceSettings};
use junto_client_core::{Reach, ServerHandle};
use junto_media::audio::{DeviceInfo, Devices};
use junto_media::calls::{ActiveCall, CallTarget, CallsView, EndReason, Outcome, Phase};
use junto_media::engine::{MicrophoneState, OutputState, ShareId, ShareState};
use junto_media::permission::MicrophoneAccess;
use junto_media::share::{CaptureError, ShareKind};
use time::OffsetDateTime;

use super::feed::{self, Feed, MessageRow, Names, Quoted, Zone};
use super::people::{self, Faces, People, Wanted};

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
    pub channels: HashMap<WorkspaceId, ChannelId>,
    /// The direct conversation that a server shows in place of a channel.
    pub conversations: HashMap<ServerId, ConversationId>,
    /// The thread that the side panel shows: its server, its channel and its root.
    pub thread: Option<(ServerId, ChannelId, MessageId)>,
    /// The channel whose pinned messages the side panel shows.
    pub pins: Option<(ServerId, ChannelId)>,
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
    /// The servers that refused the picture of the profile preset for the account that
    /// this device created there (ADR 0717).
    pub preset_refused: HashSet<ServerId>,
    pub media: MediaSnapshot,
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
    /// The uploads of the messages that wait to be sent, by the number of the upload.
    pub uploads: HashMap<u64, UploadProgress>,
    /// The date of this moment in the time zone of the messages.
    pub today: Option<time::Date>,
    /// The fork of a thread that the person asked for.
    pub fork: Option<ForkAsked>,
    /// Why the server did not open a direct conversation, with the account that was asked
    /// for.
    pub conversation_refused: Option<(ServerId, AccountId, String)>,
    /// The account whose direct conversation the server was asked for, while the answer
    /// is on its way.
    pub conversation_asked: Option<(ServerId, AccountId)>,
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
        self.messages = (self.messages + other.messages).min(100);
        self.mentions = (self.mentions + other.mentions).min(100);
        self.sign |= other.sign;
    }

    fn of(model: &ServerModel, place: PlaceId) -> Self {
        model
            .message_state(place)
            .map_or(Self::default(), |state| Self {
                messages: state.unread,
                mentions: state.notification_mentions,
                sign: state.unread_sign,
                muted: state.muted,
            })
    }
}

/// An unread thread of a channel, or the thread that the side panel shows, as the channel
/// column lists it under its channel.
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
    WaitingForApproval {
        name: String,
    },
    WaitingForLink {
        code: String,
    },
    NoAccount(Option<Notice>),
    Banned,
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
        AccessState::WaitingForApproval { name, .. } => {
            ServerStatus::WaitingForApproval { name: name.clone() }
        }
        AccessState::WaitingForLink { code, .. } => ServerStatus::WaitingForLink {
            code: code.expose().to_owned(),
        },
        AccessState::NoAccount { notice } => ServerStatus::NoAccount(*notice),
        AccessState::Banned { .. } => ServerStatus::Banned,
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
    pub status: ServerStatus,
    pub selected: bool,
    /// The device has a recovery notice of this server that the person has not seen.
    pub has_recovery: bool,
    /// What the places of the server hold that the person has not read.
    pub unread: Unread,
    /// What of it no listed workspace shows: the direct conversations, and every place
    /// of a server without a listed workspace. The head of the server shows its sign.
    pub head_unread: Unread,
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
    /// The unread threads of the channel, and the thread that the side panel shows.
    pub threads: Vec<ThreadEntry>,
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

/// The share of this device, as its button shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OwnShare {
    #[default]
    Off,
    Starting,
    Running,
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

/// The call of this device, when the main area shows its voice channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPanel {
    pub phase: Phase,
    /// The time of day in UTC at which the media connection was interrupted.
    pub interrupted_at: Option<OffsetDateTime>,
    pub outside: Vec<OutsideMember>,
    pub audio: AudioSettings,
    /// The shares of the call. Absent while nobody shares.
    pub stage: Option<ShareStage>,
    /// The strip of the call shows only the people who send a camera picture.
    pub hide_without_video: bool,
    pub share_resolution: ShareResolution,
    pub share_frame_rate: u32,
    pub recording: RecordingControl,
    pub room: RoomView,
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
    /// Present when the selected channel is the voice channel of the call of this device.
    pub call: Option<CallPanel>,
    /// What the person holds in the shown channel, and in the workspace while no channel
    /// is shown.
    pub allowed: Allowed,
    /// The messages of the selected channel. Absent until the client holds its timeline.
    pub feed: Option<Feed>,
    /// The files that the person attached to the next message of the selected channel.
    pub draft: Vec<DraftFile>,
    /// The roles of the workspace that a message can mention, without the "everyone" role.
    pub roles: Vec<(WorkspaceRoleId, String)>,
    /// The uploads of the messages that wait to be sent, by the number of the upload.
    pub uploads: Arc<HashMap<u64, UploadProgress>>,
    /// The direct conversations of the server.
    pub directs: Vec<DirectRow>,
    /// The direct conversation that the main area shows in place of a channel.
    pub conversation: Option<DirectRow>,
    /// The thread that the side panel shows.
    pub thread: Option<ThreadPanel>,
    /// The pinned messages that the side panel shows.
    pub pins: Option<PinsPanel>,
    /// Why the server did not open a direct conversation, with the account asked for.
    pub conversation_refused: Option<(AccountId, String)>,
    /// The account whose direct conversation the server was asked for, while the answer
    /// is on its way.
    pub conversation_asked: Option<AccountId>,
    /// Present when the shown channel is archived (ADR 0762): it is read and takes no
    /// message.
    pub archived: Option<ArchivedState>,
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
    pub disconnect: bool,
}

impl ShellView {
    /// The menu of a person in a voice channel of the workspace that is shown. Absent for
    /// the person of this device, and for a person who is no longer there.
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
                .filter(|call| call.channel_id != Some(channel_id))
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
            disconnect: allowed.disconnect,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The lists of a workspace.
    Workspace(Box<WorkspaceView>),
    /// The account can see no workspace.
    NoWorkspace,
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
    /// Why the last action of the access flow failed.
    pub error: Option<FlowError>,
    /// The recoveries that this device has not acknowledged, oldest first.
    pub recoveries: Vec<RecoveryNotice>,
    /// The key of this device belonged to another account at the last contact
    /// (ADR 0712).
    pub account_changed: bool,
    pub content: Content,
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

/// The person's own controls, in a call and outside one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Controls {
    pub microphone: MicrophoneButton,
    /// The person turned the sound off.
    pub sound_off: bool,
    pub warning: Option<SoundWarning>,
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
    pub target: CallTarget,
    pub by: Option<String>,
    /// The name of the voice channel. Absent for a direct-message call.
    pub channel: Option<String>,
}

/// How the last join or call ended, when the person did not end it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallNotice {
    JoinFailed {
        channel: Option<String>,
        error: FlowError,
    },
    Ended {
        channel: Option<String>,
        reason: EndReason,
    },
    /// A person with "move members" moved the person into this voice channel.
    Moved { channel: Option<String> },
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
    pub fn workspace(&self) -> Option<&WorkspaceView> {
        match &self.selected.as_ref()?.content {
            Content::Workspace(workspace) => Some(workspace),
            Content::NoWorkspace | Content::State => None,
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
}

impl Shown {
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
            conversation: workspace
                .and_then(|workspace| workspace.conversation.as_ref())
                .map(|conversation| conversation.id),
            thread: workspace
                .and_then(|workspace| workspace.thread.as_ref())
                .map(|thread| (thread.channel_id, thread.root_id)),
            pins: workspace
                .and_then(|workspace| workspace.pins.as_ref())
                .map(|pins| pins.channel_id),
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

fn name_of(
    server_id: ServerId,
    account_id: AccountId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Option<String> {
    if account_id == model.account.id {
        return Some(model.account.name.clone());
    }
    workspace
        .members
        .loaded()
        .and_then(|members| {
            members
                .iter()
                .find(|member| member.account_id == account_id)
                .map(|member| member.name.clone())
        })
        .or_else(|| known.people.name(server_id, account_id))
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
    let name_of = |account_id: AccountId| name_of(server_id, account_id, workspace, model, known);
    let own_presence = match active.map(|call| call.phase) {
        Some(Phase::Connecting) => Presence::Connecting,
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
    let own_row = |held: Option<&junto_client_core::protocol::call::MicrophoneBlock>| Occupant {
        account_id: own,
        name: name_of(own),
        is_self: true,
        microphone: match (held, media.calls.muted) {
            (Some(block), _) => Microphone::Held(block.reason),
            (None, true) => Microphone::Closed,
            (None, false) => Microphone::Open {
                speaking: media.levels.own.1,
            },
        },
        held_by: held.and_then(|block| name_of(block.by)),
        sharing: active.is_some_and(|call| matches!(call.share, ShareState::Running(_))),
        hand_raised: false,
        sound_off: media.calls.deafened,
        sound_blocked: active.is_some_and(|call| call.sound_block.is_some()),
        level: Some(media.levels.own.0),
        presence: unlisted,
        volume: VOLUME_DEFAULT_PERCENT,
    };
    let entry = known
        .entry
        .as_ref()
        .filter(|entry| entry.server_id == server_id && entry.channel_id == channel.channel.id)
        .filter(|_| relation == Relation::Outside)
        .map(|entry| entry.state);
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
            occupants: active
                .map(|active| vec![own_row(active.microphone_block.as_ref())])
                .unwrap_or_default(),
            ..VoiceState::default()
        };
    };
    let mut occupants: Vec<Occupant> = call
        .participants
        .iter()
        .map(|participant| {
            let is_self = participant.account_id == own;
            let measured = active.map(|_| {
                if is_self {
                    media.levels.own
                } else {
                    media
                        .levels
                        .people
                        .get(&participant.account_id)
                        .copied()
                        .unwrap_or_default()
                }
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
                // The share of this device is shown from the moment its track is sent;
                // the server tells it a moment later.
                sharing: participant.screen_share.is_some()
                    || (is_self
                        && active.is_some_and(|call| matches!(call.share, ShareState::Running(_)))),
                hand_raised: participant.hand_raised,
                sound_off: participant.deafened || participant.sound_block.is_some(),
                sound_blocked: participant.sound_block.is_some(),
                level: measured.map(|(level, _)| level),
                presence: if is_self {
                    own_presence
                } else {
                    Presence::Here
                },
                volume: media.voice.volume(server_id, participant.account_id),
            }
        })
        .collect();
    if let Some(active) = active
        && !occupants.iter().any(|occupant| occupant.is_self)
    {
        occupants.push(own_row(active.microphone_block.as_ref()));
    }
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
) -> ChannelRow {
    ChannelRow {
        id: channel.channel.id,
        kind: channel.channel.kind,
        name: channel.channel.name.clone(),
        selected: false,
        voice: (channel.channel.kind == ChannelKind::Voice)
            .then(|| voice_state(server_id, channel, workspace, model, known)),
        unread: Unread::of(model, PlaceId::Channel(channel.channel.id)),
        permanent: channel.channel.permanent_messages,
        threads: Vec::new(),
        allowed: allowed(workspace.permissions_in(channel.channel.id)),
    }
}

/// The characters of the root of a thread that the channel column shows.
const THREAD_TITLE_CHARACTERS: usize = 48;

/// The unread threads of a channel, in the order of the server, and after them the thread
/// that the side panel shows when nothing of it is unread.
fn thread_entries(
    channel_id: ChannelId,
    model: &ServerModel,
    names: &Names,
    open: Option<MessageId>,
) -> Vec<ThreadEntry> {
    use junto_client_core::model::Named;
    let title = |root: &junto_client_core::protocol::message::Message| {
        root.document
            .as_ref()
            .map(|document| feed::plain_text(document, names, THREAD_TITLE_CHARACTERS))
            .filter(|title| !title.is_empty())
            .or_else(|| root.attachments.first().map(|file| file.name.clone()))
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
    entries
}

/// The channels of a workspace in the groups of the channel column: first the channels
/// without a category, by kind, then each category that holds a visible channel.
fn sections(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Vec<ChannelSection> {
    let in_category = |channel: &ChannelModel| {
        channel.channel.category_id.filter(|id| {
            workspace
                .categories
                .iter()
                .any(|category| category.id == *id)
        })
    };
    let row = |channel: &ChannelModel| channel_row(server_id, channel, workspace, model, known);
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
                    account_id: member.account_id,
                    name: member.name.clone(),
                    is_self: member.account_id == model.account.id,
                    roles: workspace
                        .roles
                        .iter()
                        .filter(|role| !role.is_everyone && member.role_ids.contains(&role.id))
                        .map(|role| role.name.clone())
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
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
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
                unread: Unread::of(model, PlaceId::Direct(conversation.id)),
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

fn workspace_view(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    selection: &Selection,
    known: &Known,
) -> WorkspaceView {
    let mut sections = sections(server_id, workspace, model, known);
    // A direct conversation is shown in place of a channel while the state holds it.
    let conversation_id = selection
        .conversations
        .get(&server_id)
        .copied()
        .filter(|id| model.conversations.iter().any(|held| held.id == *id));
    let directs = directs(server_id, workspace, model, known, conversation_id);
    let conversation = directs.iter().find(|row| row.selected).cloned();
    let wanted = selection.channels.get(&workspace.workspace.id).copied();
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
    let names = Arc::new(names(server_id, workspace, model, known));
    let mut channel = None;
    for row in sections
        .iter_mut()
        .flat_map(|section| section.rows.iter_mut())
    {
        let open = open_thread
            .filter(|(channel_id, _)| *channel_id == row.id)
            .map(|(_, root_id)| root_id);
        row.threads = thread_entries(row.id, model, &names, open);
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
        // A call of an archived voice channel has ended: the row has no voice state, and
        // the main area shows its messages.
        channel = Some(ChannelRow {
            id: channel_id,
            kind: archived.channel.kind,
            name: archived.channel.name.clone(),
            selected: true,
            voice: None,
            unread: Unread::of(model, PlaceId::Channel(channel_id)),
            permanent: archived.channel.permanent_messages,
            threads: thread_entries(channel_id, model, &names, open),
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
    let place = conversation
        .as_ref()
        .map(|row| PlaceId::Direct(row.id))
        .or(channel.as_ref().map(|row| PlaceId::Channel(row.id)));
    // A direct conversation has no workspace: no role of one counts in it (ADR 0302).
    let direct = conversation.is_some();
    let source = |timeline| feed::Source {
        server_id,
        model,
        timeline,
        names: names.clone(),
        own_roles: if direct { &[] } else { &workspace.own_role_ids },
        may_manage: allowed.manage_messages && !direct,
        open_thread: open_thread.map(|(_, root_id)| root_id),
        threads: call.is_none(),
        quoted: &known.quoted,
        zone: known.zone,
        today: known.today,
    };
    let feed = place
        .and_then(|place| model.timeline(place))
        .map(|timeline| feed::derive(&source(timeline)));
    // An archived channel takes no change (ADR 0410): its messages offer none, in the
    // main area, in a thread and in the pin list.
    let read_only = archived.is_some();
    let fixed = |mut row: MessageRow| {
        if read_only {
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
                feed: timeline.map(|timeline| fixed_feed(feed::derive(&source(timeline)))),
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
    WorkspaceView {
        id: workspace.workspace.id,
        name: workspace.workspace.name.clone(),
        sections,
        channel,
        members: members(server_id, workspace, model, known),
        call,
        allowed,
        feed,
        draft: place.map(draft_of).unwrap_or_default(),
        roles: workspace
            .roles
            .iter()
            .filter(|role| !role.is_everyone)
            .map(|role| (role.id, role.name.clone()))
            .collect(),
        uploads: Arc::new(known.uploads.clone()),
        directs,
        conversation,
        thread,
        pins,
        conversation_refused: known
            .conversation_refused
            .as_ref()
            .filter(|(server, _, _)| *server == server_id)
            .map(|(_, account_id, reason)| (*account_id, reason.clone())),
        conversation_asked: known
            .conversation_asked
            .filter(|(server, _)| *server == server_id)
            .map(|(_, account_id)| account_id),
        archived: archived
            .filter(|held| workspace.channel(held.channel.id).is_none())
            .map(|held| ArchivedState {
                archived_at: known.zone.local(held.archived_at),
                deletes_at: held.deletes_at.map(|at| known.zone.local(at)),
                may_restore: allowed.manage,
            }),
    }
}

/// The names that the messages of a workspace can mention: its members, the accounts
/// whose names were read, the account itself, and its roles.
fn names(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Names {
    let mut accounts: HashMap<AccountId, String> = known
        .people
        .names(server_id)
        .map(|(account_id, name)| (account_id, name.to_owned()))
        .collect();
    if let Some(members) = workspace.members.loaded() {
        accounts.extend(
            members
                .iter()
                .map(|member| (member.account_id, member.name.clone())),
        );
    }
    accounts.insert(model.account.id, model.account.name.clone());
    Names {
        accounts,
        roles: workspace
            .roles
            .iter()
            .map(|role| (role.id, role.name.clone()))
            .collect(),
    }
}

/// What the channels of a workspace hold that the person has not read.
fn workspace_unread(workspace: &WorkspaceModel, model: &ServerModel) -> Unread {
    let mut unread = Unread::default();
    for channel in &workspace.channels {
        unread.add(Unread::of(model, PlaceId::Channel(channel.channel.id)));
    }
    unread
}

/// What the places of a server hold that the person has not read and that their settings
/// let through.
fn server_unread(model: &ServerModel) -> Unread {
    let mut unread = Unread::default();
    for state in &model.message_states {
        unread.add(Unread::of(model, PlaceId::of(&state.place)));
    }
    unread
}

/// What a server holds unread that no row of a workspace shows: its direct conversations,
/// and every place when the server lists no workspace.
fn head_unread(model: &ServerModel) -> Unread {
    if model.workspaces.is_empty() {
        return server_unread(model);
    }
    let mut unread = Unread::default();
    for state in &model.message_states {
        if let place @ PlaceId::Direct(_) = PlaceId::of(&state.place) {
            unread.add(Unread::of(model, place));
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

/// The shares of a call: the ones of the other people as the media connection shows them,
/// with the sound that the server tells of each, and the one of this device.
fn share_stage(
    server_id: ServerId,
    active: &ActiveCall,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Option<ShareStage> {
    let call = model.call_at(&active.target.place);
    let name_of = |account_id: AccountId| name_of(server_id, account_id, workspace, model, known);
    let mut shares: Vec<ShareCard> = active
        .shares
        .iter()
        .map(|seen| ShareCard {
            id: ShareId::Of(seen.account_id),
            account_id: seen.account_id,
            name: name_of(seen.account_id),
            kind: seen.kind,
            sound: call
                .and_then(|call| call.participant(seen.account_id))
                .and_then(|participant| participant.screen_share.as_ref())
                .map(|share| share.audio),
            sound_held: false,
            size: seen.size,
            frame_rate: seen.frame_rate,
            watched: active.watched == Some(ShareId::Of(seen.account_id)),
        })
        .collect();
    if let ShareState::Running(info) = &active.share {
        shares.push(ShareCard {
            id: ShareId::Own,
            account_id: model.account.id,
            name: name_of(model.account.id),
            kind: Some(info.kind),
            sound: Some(info.sound),
            sound_held: info.sound_held,
            size: Some(info.size),
            frame_rate: Some(info.frame_rate),
            watched: active.watched == Some(ShareId::Own),
        });
    }
    (!shares.is_empty()).then_some(ShareStage {
        shares,
        volume: known.media.voice.share_volume,
    })
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
    let media = &known.media;
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
                    name: member.name.clone(),
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
    CallPanel {
        phase: active.phase,
        interrupted_at: active.interrupted_at,
        outside,
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
        recording: recording_control(
            server_id,
            call.map(|call| call.id),
            call.and_then(|call| call.recording.as_ref()),
            row.allowed.record,
            known,
        ),
        room: room_view(server_id, row, active, workspace, model, known),
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
    call.and_then(|call| call.meeting.as_ref())
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
    let name_of = |account_id: AccountId| name_of(server_id, account_id, workspace, model, known);
    let own = model.account.id;
    let meeting = call.and_then(|call| call.meeting.as_ref()).map(|meeting| {
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
        name: row.name.clone(),
        inside: voice.map_or(0, |voice| voice.occupants.len()),
        meeting_start: if !allowed.run_meeting {
            MeetingStart::NoPermission
        } else if active.microphone_block.is_some() {
            MeetingStart::MicrophoneBlocked
        } else if !matches!(active.share, ShareState::Running(_)) {
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
                    name_of(server.id, invitation.invited_by, workspace, model, known)
                })
                .or_else(|| known.people.name(server.id, invitation.invited_by));
            waiting.push(Invited {
                id: invitation.id,
                target,
                by,
                channel: channel_of(servers, &target),
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
                    name_of(server.id, request.account_id, workspace, model, known)
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
        name_of(server.id, account_id, workspace, model, known)
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
    Controls {
        microphone: if held.is_some() {
            MicrophoneButton::Held
        } else if calls.muted {
            MicrophoneButton::Muted
        } else {
            MicrophoneButton::Open
        },
        sound_off: calls.deafened,
        warning,
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
        name: channel_of(servers, &active.target),
        phase: active.phase,
        elapsed: started_at.zip(known.media.now).map(|(started_at, now)| {
            // The clock of the server and the clock of this machine can differ.
            Duration::try_from(now - started_at).unwrap_or_default()
        }),
        shown: shown.server == Some(active.target.server_id)
            && channel_id.is_some()
            && shown.channel == channel_id,
        share: match active.share {
            ShareState::Off | ShareState::Ended(_) => OwnShare::Off,
            ShareState::Starting => OwnShare::Starting,
            ShareState::Running(_) => OwnShare::Running,
        },
        can_share: known.media.can_share,
        hand_raised: channel_id.zip(model).map(|(_, model)| {
            call.and_then(|call| call.participant(model.account.id))
                .is_some_and(|own| own.hand_raised)
        }),
        may_speak: may_speak(active, call, model, known),
    })
}

fn call_notice(servers: &[ServerSnapshot], known: &Known) -> Option<CallNotice> {
    match known.media.calls.outcome.as_ref()? {
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
            let shown = shown_model(server).and_then(|model| {
                shown_workspace(model, selection.workspaces.get(&server.id).copied())
            });
            ServerRow {
                id: server.id,
                name: server.name.clone(),
                address: server.address.clone(),
                selected,
                has_recovery: matches!(
                    &server.access,
                    AccessState::LoggedIn { recoveries, .. } if !recoveries.is_empty()
                ),
                unread: shown_model(server)
                    .filter(|_| status.shows_content())
                    .map(server_unread)
                    .unwrap_or_default(),
                head_unread: shown_model(server)
                    .filter(|_| status.shows_content())
                    .map(head_unread)
                    .unwrap_or_default(),
                workspaces: shown_model(server)
                    .filter(|_| status.shows_content())
                    .map(|model| {
                        model
                            .workspaces
                            .iter()
                            .map(|workspace| WorkspaceRow {
                                unread: workspace_unread(workspace, model),
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
                match shown_workspace(model, selection.workspaces.get(&server.id).copied()) {
                    Some(workspace) => Content::Workspace(Box::new(workspace_view(
                        server.id, workspace, model, selection, known,
                    ))),
                    None => Content::NoWorkspace,
                }
            }
        };
        SelectedServer {
            id: server.id,
            name: server.name.clone(),
            address: server.address.clone(),
            status,
            keep_connected: server.keep_connected,
            busy: server.in_progress.is_some(),
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
            content,
        }
    });
    let account = server.and_then(shown_model).map(|model| Person {
        account_id: Some(model.account.id),
        name: model.account.name.clone(),
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
            .and_then(|call| match &call.share {
                ShareState::Ended(error) => Some(error.clone()),
                _ => None,
            }),
    };
    view.call = call_strip(servers, known, view.shown());
    view.faces = people::faces(servers, &known.people);
    view.wanted_people = people::wanted(servers, &view);
    view
}

#[cfg(test)]
mod book_tests;
#[cfg(test)]
mod panel_tests;
#[cfg(test)]
mod people_tests;
#[cfg(test)]
mod permission_tests;
#[cfg(test)]
mod tests;
