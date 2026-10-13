//! The state of one server as the account of this device sees it (ADR 0719).
//!
//! A state is built from a snapshot and changed by the events after the cursor of the
//! snapshot (ADR 0207). This module has no input and no output: the event stream client
//! owns one state per server, applies each event to it and gives readers copies.
//!
//! A copy shares every workspace that did not change since the copy before, so a reader
//! that keeps an old copy costs the memory of the changed workspaces only.

mod admin;
mod apply;
mod discovery;
mod messages;
mod recordings;

#[cfg(test)]
mod admin_tests;
#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
pub(crate) mod message_tests;
#[cfg(test)]
pub(crate) mod recording_tests;
#[cfg(test)]
pub(crate) mod tests;

use std::sync::Arc;

use junto_protocol::account::OwnAccount;
use junto_protocol::call::{
    Call, CallInvitation, CallLock, CallMove, CallParticipant, CallPlace, ChannelCall, DirectMute,
    EntryRequest, Meeting, OwnCall, OwnCallState,
};
use junto_protocol::discovery::{Notification, ScopedNotificationSettings};
use junto_protocol::events::Cursor;
use junto_protocol::ids::{
    AccountId, CallId, ChannelId, DeviceId, MessageId, WorkspaceId, WorkspaceRoleId,
};
use junto_protocol::message::{
    AttachmentLimits, Conversation, MessagePlace, MessageState, ThreadState,
};
use junto_protocol::recording::Recording;
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::snapshot::{Snapshot, WorkspaceState};
use junto_protocol::structure::{Category, Channel, Workspace, WorkspaceMember};
use junto_protocol::{ServerPermission, WorkspacePermission};
use time::OffsetDateTime;

pub use admin::{
    AdminModel, AdminPart, Collection, CollectionState, LoadFailure, MOST_TOKENS, WorkspaceSettings,
};
pub(crate) use admin::{AdminPayload, AdminSet};
pub(crate) use apply::{Follow, Part};
pub(crate) use discovery::ThreadMoved;
pub use discovery::{MOST_NOTICES, MOST_PINS, MOST_THREADS, Named, PinList, in_daily_quiet};
pub(crate) use messages::Reach;
pub use messages::{
    MOST_LOADED, PendingFile, PendingMessage, PlaceId, ReadFailure, Reading, SendFailure,
    SendState, Timeline,
};
pub use recordings::{
    LINES_PAGE, MOST_LINES, MOST_SEGMENTS, MOST_SPEAKERS, OpenRecording, TranscriptLines,
};
pub(crate) use recordings::{LinesAsk, LinesRead};

/// How many notices a snapshot carries at most (ADR 0409).
const SNAPSHOT_NOTICES: usize = 50;

/// A part of the state that the snapshot does not carry. The client loads it from its
/// route when a reader asks for it and keeps it current from then on (ADR 0207).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Loadable<T> {
    /// Nobody asked for the part, or a reload of the state discarded it.
    #[default]
    NotLoaded,
    /// The request is on its way.
    Loading,
    Loaded(T),
}

impl<T> Loadable<T> {
    pub const fn loaded(&self) -> Option<&T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::NotLoaded | Self::Loading => None,
        }
    }

    fn loaded_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::NotLoaded | Self::Loading => None,
        }
    }
}

/// A call as the account may see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallModel {
    pub id: CallId,
    pub place: CallPlace,
    /// When the first person was seen in the room. Absent until then: a call exists from
    /// the first request to enter (ADR 0510).
    pub started_at: Option<OffsetDateTime>,
    pub lock: Option<CallLock>,
    pub meeting: Option<Meeting>,
    /// The call shows who is inside only to the people inside. A call keeps the setting
    /// that its voice channel had when the call began (ADR 0517). For a call that the
    /// state learned of from an event, this is the setting of the channel at that event,
    /// until an answer of the server states it.
    pub hides_occupants: bool,
    /// The people in the call, in the order in which they joined. Empty for the account
    /// with a place in a call that hides who is inside, until the server has seen its
    /// device in the room.
    pub participants: Vec<CallParticipant>,
    /// The people who speak now, as the last `call_speaking` told it (ADR 0513). A
    /// snapshot does not carry the marks, so the list is empty until the next change.
    pub speaking: Vec<AccountId>,
    /// The recording of the call that is starting or runs (ADR 0601). A reader shows the
    /// recording indicator while it is present. A recording that has ended leaves the
    /// state; its later changes are notices.
    pub recording: Option<Recording>,
}

impl CallModel {
    fn new(id: CallId, place: CallPlace, hides_occupants: bool) -> Self {
        Self {
            id,
            place,
            started_at: None,
            lock: None,
            meeting: None,
            hides_occupants,
            participants: Vec::new(),
            speaking: Vec::new(),
            recording: None,
        }
    }

    fn of(call: Call) -> Self {
        Self {
            id: call.id,
            place: call.place,
            started_at: call.started_at,
            lock: call.lock,
            meeting: call.meeting,
            hides_occupants: call.hides_occupants,
            participants: call.participants,
            speaking: Vec::new(),
            recording: call
                .recording
                .filter(|recording| recording.state.is_active()),
        }
    }

    pub fn participant(&self, account_id: AccountId) -> Option<&CallParticipant> {
        self.participants
            .iter()
            .find(|participant| participant.account_id == account_id)
    }
}

/// One channel with the call of a voice channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelModel {
    pub channel: Channel,
    /// True while at least one person is in the room of the voice channel.
    pub in_use: bool,
    /// What the account knows of the call. Absent for a call that hides who is inside
    /// while the account has no place in it (ADR 0514, ADR 0517). It can be present
    /// before the call is in use: then nobody is shown in it yet.
    pub call: Option<CallModel>,
}

impl ChannelModel {
    fn new(channel: Channel) -> Self {
        Self {
            channel,
            in_use: false,
            call: None,
        }
    }
}

/// One workspace that the account can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceModel {
    pub workspace: Workspace,
    /// The roles of the workspace that the account holds, without the "everyone" role.
    pub own_role_ids: Vec<WorkspaceRoleId>,
    pub roles: Vec<WorkspaceRole>,
    /// In the order of their positions.
    pub categories: Vec<Category>,
    /// The channels that the account can see, in the order of their positions.
    pub channels: Vec<ChannelModel>,
    /// The members, once a reader asked for them.
    pub members: Loadable<Vec<WorkspaceMember>>,
}

impl WorkspaceModel {
    fn of(state: WorkspaceState) -> Self {
        let mut channels: Vec<ChannelModel> =
            state.channels.into_iter().map(ChannelModel::new).collect();
        for shown in state.calls {
            if let Some(channel) = channels
                .iter_mut()
                .find(|channel| channel.channel.id == shown.channel_id)
            {
                channel.set_call(shown);
            }
        }
        Self {
            workspace: state.workspace,
            own_role_ids: state.own_role_ids,
            roles: state.roles,
            categories: state.categories,
            channels,
            members: Loadable::NotLoaded,
        }
    }

    pub fn channel(&self, channel_id: ChannelId) -> Option<&ChannelModel> {
        self.channels
            .iter()
            .find(|channel| channel.channel.id == channel_id)
    }

    /// The permissions that the "everyone" role and the roles of the account give in the
    /// workspace. The overrides of a channel are not in the snapshot (ADR 0207), so this
    /// is not the answer for one channel, and the server decides every request.
    pub fn role_permissions(&self) -> Vec<WorkspacePermission> {
        let mut held: Vec<WorkspacePermission> = self
            .roles
            .iter()
            .filter(|role| role.is_everyone || self.own_role_ids.contains(&role.id))
            .flat_map(|role| role.permissions.iter().copied())
            .collect();
        held.sort_unstable();
        held.dedup();
        held
    }
}

impl ChannelModel {
    fn set_call(&mut self, shown: ChannelCall) {
        self.in_use = shown.in_use;
        self.call = shown.call.map(CallModel::of);
    }
}

/// The state of one server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerModel {
    /// The state holds the effect of every event up to this cursor that the account may
    /// receive at the level of the connection.
    pub cursor: Cursor,
    pub account: OwnAccount,
    /// The workspaces that the account can see, in the order of the server.
    pub workspaces: Vec<Arc<WorkspaceModel>>,
    /// The call that the account is in, and the device that it is in the call with.
    pub own_call: Option<OwnCall>,
    /// The call of the last answer of `GET /call` that was put into the state, and the
    /// position that the answer named (ADR 0519).
    pub(crate) own_call_cursor: Option<(CallId, Cursor)>,
    /// The direct-message calls of the account. One without `started_at` is not in
    /// progress yet.
    pub direct_calls: Vec<CallModel>,
    /// The invitations that wait: the ones that the account got and the ones that it made.
    pub invitations: Vec<CallInvitation>,
    /// The requests of people outside to enter the call that the account is in.
    pub entry_requests: Vec<EntryRequest>,
    /// The requests of the account to enter a voice channel that wait for an answer,
    /// from whichever device of the account they were made.
    pub own_entry_requests: Vec<EntryRequest>,
    /// The moves that wait: of the account, and the ones that the account asked for.
    pub moves: Vec<CallMove>,
    /// The direct-message conversations that the account has muted.
    pub direct_mutes: Vec<DirectMute>,
    /// The unread summary of each place that the account can read and that holds a
    /// message (ADR 0408).
    pub message_states: Vec<MessageState>,
    /// The direct conversations of the account that the contact rule allows now.
    pub conversations: Vec<Conversation>,
    /// The limits of the server for the files of a message. They are read when a reader
    /// opens the messages of a place.
    pub attachment_limits: Option<AttachmentLimits>,
    /// The messages of the places that a reader has open.
    pub timelines: Vec<Arc<Timeline>>,
    /// The messages that the person sent and that the server has not confirmed.
    pub outbox: Vec<Arc<PendingMessage>>,
    /// The newest messages that the server returned as answers to sends of this client.
    pub(crate) sent_ids: Vec<MessageId>,
    /// The threads that hold something unread for the account, the newest first, as far
    /// as their pages were read (ADR 0409).
    pub threads: Vec<ThreadState>,
    /// The server lists more unread threads after the loaded ones.
    pub threads_more: bool,
    pub(crate) threads_next: Option<String>,
    /// The notices of the account, the newest first, as far as their pages were read.
    pub notices: Vec<Notification>,
    /// The server may hold more notices after the loaded ones.
    pub notices_more: bool,
    /// The notification settings that the account stored, by their scope.
    pub notification_settings: Vec<ScopedNotificationSettings>,
    /// The messages that the unread threads and the notices name, as far as they were
    /// read.
    pub named: Vec<(MessageId, Named)>,
    /// The pin lists of the channels that a reader has open.
    pub pins: Vec<PinList>,
    /// The recordings that a reader has open, each with its segments, its transcript and
    /// the transcript lines that were read.
    pub recordings: Vec<Arc<OpenRecording>>,
    /// The highest position of each place that an event named since the snapshot.
    pub(crate) seen_positions: std::collections::BTreeMap<PlaceId, u64>,
    /// The collections of the administration that a reader has open (ADR 0760).
    pub admin: Arc<AdminModel>,
}

impl ServerModel {
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        let mut conversations: Vec<Conversation> = snapshot
            .messages
            .iter()
            .filter_map(|state| match state.place {
                MessagePlace::Direct {
                    conversation_id,
                    low_account_id,
                    high_account_id,
                } => Some(Conversation {
                    id: conversation_id,
                    low_account_id,
                    high_account_id,
                }),
                MessagePlace::Channel { .. } => None,
            })
            .collect();
        conversations.sort_by_key(|held| held.id);
        let mut threads = snapshot.threads;
        threads.truncate(MOST_THREADS);
        let mut notices = snapshot.notifications;
        // The snapshot carries the newest 50 notices and does not tell whether there are
        // more (ADR 0409). A full list is taken as one that goes on.
        let notices_more = notices.len() >= SNAPSHOT_NOTICES;
        notices.truncate(MOST_NOTICES);
        let mut invitations = snapshot.call_invitations;
        for sent in snapshot.sent_call_invitations {
            if !invitations.iter().any(|held| held.id == sent.id) {
                invitations.push(sent);
            }
        }
        Self {
            cursor: snapshot.cursor,
            account: snapshot.account,
            workspaces: snapshot
                .workspaces
                .into_iter()
                .map(|state| Arc::new(WorkspaceModel::of(state)))
                .collect(),
            own_call: snapshot.own_call,
            own_call_cursor: None,
            direct_calls: snapshot
                .direct_calls
                .into_iter()
                .map(CallModel::of)
                .collect(),
            invitations,
            entry_requests: Vec::new(),
            own_entry_requests: snapshot.entry_requests,
            moves: snapshot.call_moves,
            direct_mutes: snapshot.direct_mutes,
            message_states: snapshot.messages,
            conversations,
            attachment_limits: None,
            timelines: Vec::new(),
            outbox: Vec::new(),
            sent_ids: Vec::new(),
            threads,
            threads_more: snapshot.threads_more,
            threads_next: snapshot.threads_next,
            notices,
            notices_more,
            notification_settings: snapshot.notification_settings,
            named: Vec::new(),
            pins: Vec::new(),
            recordings: Vec::new(),
            seen_positions: std::collections::BTreeMap::new(),
            admin: Arc::default(),
        }
    }

    pub fn workspace(&self, workspace_id: WorkspaceId) -> Option<&WorkspaceModel> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.workspace.id == workspace_id)
            .map(Arc::as_ref)
    }

    /// A channel that the account can see, in any workspace.
    pub fn channel(&self, channel_id: ChannelId) -> Option<&ChannelModel> {
        self.workspaces
            .iter()
            .find_map(|workspace| workspace.channel(channel_id))
    }

    /// What the account knows of the call at a place.
    pub fn call_at(&self, place: &CallPlace) -> Option<&CallModel> {
        match place {
            CallPlace::Channel { channel_id, .. } => self.channel(*channel_id)?.call.as_ref(),
            CallPlace::Direct { .. } => self.direct_calls.iter().find(|call| call.place == *place),
        }
    }

    /// The position up to which the state holds the events of the call at `place`. It is
    /// the cursor of the state, or the position of a read of the own call that is ahead of
    /// the events (ADR 0785). The own call of the account and the moves follow the cursor
    /// of the state only.
    pub fn call_cursor(&self, place: &CallPlace) -> Cursor {
        self.own_call_cursor
            .filter(|(id, cursor)| {
                cursor.epoch == self.cursor.epoch
                    && cursor.position > self.cursor.position
                    && self.call_at(place).is_some_and(|call| call.id == *id)
            })
            .map_or(self.cursor, |(_, cursor)| cursor)
    }

    /// True when the account is in a call with the device `device_id`.
    pub fn in_call_with(&self, device_id: DeviceId) -> bool {
        self.own_call
            .as_ref()
            .is_some_and(|own| own.device_id == device_id)
    }

    /// The invitations that wait for an answer of the account.
    pub fn incoming_invitations(&self) -> impl Iterator<Item = &CallInvitation> {
        let own = self.account.id;
        self.invitations
            .iter()
            .filter(move |invitation| invitation.account_id == own)
    }

    /// True for the server owner and for an account with the permission `administrator`,
    /// which the permission function allows everything that a role can allow (ADR 0301).
    pub fn is_administrator(&self) -> bool {
        self.account.is_owner
            || self
                .account
                .server_permissions
                .contains(&ServerPermission::Administrator)
    }

    fn workspace_mut(&mut self, workspace_id: WorkspaceId) -> Option<&mut WorkspaceModel> {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace.id == workspace_id)
            .map(Arc::make_mut)
    }

    fn channel_mut(&mut self, place: &CallPlace) -> Option<&mut ChannelModel> {
        let CallPlace::Channel {
            workspace_id,
            channel_id,
        } = place
        else {
            return None;
        };
        // A workspace is copied only when the channel is in it.
        self.workspace(*workspace_id)?.channel(*channel_id)?;
        self.workspace_mut(*workspace_id)?
            .channels
            .iter_mut()
            .find(|channel| channel.channel.id == *channel_id)
    }

    /// Puts the members of a workspace into the state, or the mark that they are on
    /// their way or not held.
    pub(crate) fn set_members(
        &mut self,
        workspace_id: WorkspaceId,
        members: Loadable<Vec<WorkspaceMember>>,
    ) {
        if let Some(workspace) = self.workspace(workspace_id)
            && workspace.members != members
            && let Some(workspace) = self.workspace_mut(workspace_id)
        {
            workspace.members = members;
        }
    }

    /// Puts the answer of `GET /call` into the state: the call of the account with the
    /// people in it, and the requests to enter that wait there. An answer for another
    /// call than the one that the state holds is not used.
    pub(crate) fn set_own_call(&mut self, loaded: Option<OwnCallState>) {
        let Some(loaded) = loaded else {
            return;
        };
        if self.own_call.as_ref().map(|own| own.call_id) != Some(loaded.own.call_id) {
            return;
        }
        self.entry_requests = loaded.entry_requests;
        let speaking = self
            .call_at(&loaded.call.place)
            .filter(|held| held.id == loaded.call.id)
            .map(|held| held.speaking.clone())
            .unwrap_or_default();
        let mut call = CallModel::of(loaded.call);
        call.speaking = speaking;
        self.put_call(call);
    }

    pub(crate) fn put_call(&mut self, call: CallModel) {
        match call.place {
            CallPlace::Channel { .. } => {
                if let Some(channel) = self.channel_mut(&call.place) {
                    channel.in_use = call.started_at.is_some();
                    channel.call = Some(call);
                }
            }
            CallPlace::Direct { .. } => {
                self.direct_calls.retain(|held| held.place != call.place);
                self.direct_calls.push(call);
            }
        }
    }
}
