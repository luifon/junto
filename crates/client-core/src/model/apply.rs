//! How each event changes the state (ADR 0208, ADR 0510).
//!
//! An event carries the whole changed object, so applying one twice changes nothing. An
//! event about an object whose parent the state does not hold is not applied: the parent
//! became visible by a permission change, and the reload after that change brings both.

use std::sync::Arc;

use junto_protocol::call::{BlockReason, CallParticipant, CallPlace, MicrophoneBlock};
use junto_protocol::events::Event;
use junto_protocol::ids::{CallId, ChannelId, MessageId, RecordingId, WorkspaceId};
use junto_protocol::message::MessagePlace;
use junto_protocol::recording::Recording;
use junto_protocol::structure::Channel;

use super::rank::sort_workspace_roles;
use super::{
    AdminPart, CallModel, ChannelModel, LogoState, PlaceId, Reach, ServerModel, ThreadMoved,
    WorkspaceModel,
};

/// A part of the state that is read from a route of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Members(WorkspaceId),
    /// The call of the account with its requests to enter.
    OwnCall,
    /// The unread summary of one place.
    MessageState(PlaceId),
    /// One message of a place that a reader has open.
    Message(PlaceId, MessageId),
    /// A page of the messages of a place that a reader has open.
    Messages(PlaceId, Reach),
    /// The limits of the server for the files of a message.
    AttachmentLimits,
    /// The first page of the threads that hold something unread.
    UnreadThreads,
    /// The page of unread threads after the loaded ones.
    MoreThreads,
    /// The root and the fork channels of a thread that a reader has open.
    ThreadHead(PlaceId),
    /// The pinned messages of a channel that a reader has open.
    Pins(ChannelId),
    /// The first page of the notices.
    Notices,
    /// The page of notices after this notice.
    MoreNotices(uuid::Uuid),
    /// A message that a list of the state names by its identifier.
    Named(MessageId),
    /// A recording that a reader has open, with its segments and its transcript.
    Recording(RecordingId),
    /// Lines of the transcript of a recording that a reader has open.
    TranscriptLines(RecordingId),
    /// A collection of the administration that a reader has open (ADR 0760).
    Admin(AdminPart),
}

impl Part {
    /// True for a part whose content no event carries (ADR 0408, ADR 0409): an event that
    /// arrives while the part is read is not applied to the answer; it starts the read
    /// again.
    pub(crate) const fn is_read_again(&self) -> bool {
        !matches!(self, Self::Members(_) | Self::OwnCall | Self::Admin(_))
    }
}

/// What the event stream client does after an event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Follow {
    /// The state must be replaced by a snapshot (ADR 0207, ADR 0208, ADR 0510).
    pub(crate) reload: bool,
    /// What the account may see changed, and the state withdrew what it may have lost
    /// (see [`ServerModel::withdraw`]): an answer to a read that was sent before is not
    /// taken.
    pub(crate) withdrawn: bool,
    /// The parts to read now. A read of the same part that is on its way is dropped.
    pub(crate) load: Vec<Part>,
}

impl Follow {
    fn reload() -> Self {
        Self {
            reload: true,
            withdrawn: false,
            load: Vec::new(),
        }
    }

    fn withdrawn() -> Self {
        Self {
            reload: true,
            withdrawn: true,
            load: Vec::new(),
        }
    }

    fn load(part: Part) -> Self {
        Self {
            reload: false,
            withdrawn: false,
            load: vec![part],
        }
    }

    fn loads(load: Vec<Part>) -> Self {
        Self {
            reload: false,
            withdrawn: false,
            load,
        }
    }
}

fn by_position<T>(items: &mut [T], key: impl Fn(&T) -> (i32, u128)) {
    items.sort_by_key(key);
}

fn upsert<T>(items: &mut Vec<T>, item: &T, same: impl Fn(&T) -> bool)
where
    T: Clone,
{
    match items.iter_mut().find(|held| same(held)) {
        Some(held) => *held = item.clone(),
        None => items.push(item.clone()),
    }
}

impl Part {
    /// True when `event` changes this part. The stream client keeps such events while the
    /// part is read and applies them after the answer (ADR 0207).
    pub(crate) fn concerns(&self, event: &Event, model: &ServerModel) -> bool {
        match self {
            Self::Members(workspace_id) => match event {
                Event::WorkspaceMemberAdded {
                    workspace_id: of, ..
                }
                | Event::WorkspaceMemberRemoved {
                    workspace_id: of, ..
                }
                | Event::WorkspaceMemberRolesChanged {
                    workspace_id: of, ..
                }
                | Event::WorkspaceRoleDeleted {
                    workspace_id: of, ..
                } => of == workspace_id,
                Event::AccountProfileUpdated { .. } => true,
                _ => false,
            },
            Self::OwnCall => {
                let Some(own) = &model.own_call else {
                    return false;
                };
                match event {
                    Event::CallEntryRequested { request } => request.call_id == own.call_id,
                    Event::CallEntryResolved { call_id, .. } => *call_id == own.call_id,
                    Event::RecordingRemoved { channel_id, .. } => {
                        own.place.channel_id() == Some(*channel_id)
                    }
                    _ => call_of(event).is_some_and(|(call_id, _)| call_id == own.call_id),
                }
            }
            Self::MessageState(place) => match event {
                Event::MessageChanged { place: of, .. } | Event::MessageRead { place: of, .. } => {
                    PlaceId::of(of) == *place
                }
                Event::MessageMentioned { channel_id, .. } => {
                    PlaceId::Channel(*channel_id) == *place
                }
                _ => false,
            },
            Self::Message(_, message_id) => {
                matches!(event, Event::MessageChanged { message_id: of, .. } if of == message_id)
            }
            Self::Messages(place, _) => {
                matches!(event, Event::MessageChanged { place: of, .. }
                    if PlaceId::of(of) == place.container())
            }
            Self::AttachmentLimits => matches!(event, Event::AttachmentLimitsChanged { .. }),
            Self::UnreadThreads => matches!(
                event,
                Event::MessageChanged {
                    place: MessagePlace::Channel { .. },
                    ..
                } | Event::ThreadRead { .. }
                    | Event::NotificationChanged { .. }
                    | Event::MessageMentioned { .. }
                    | Event::ThreadFollowChanged { .. }
            ),
            Self::ThreadHead(_) => matches!(
                event,
                Event::ChannelCreated { .. } | Event::ChannelDeleted { .. }
            ),
            Self::Pins(channel_id) => matches!(
                event,
                Event::MessageChanged { place: of, .. }
                    if PlaceId::of(of) == PlaceId::Channel(*channel_id)
            ),
            Self::Notices => matches!(
                event,
                Event::NotificationChanged { .. }
                    | Event::MessageRead { .. }
                    | Event::ThreadRead { .. }
                    | Event::NotificationsRead { .. }
            ),
            Self::Named(message_id) => {
                matches!(event, Event::MessageChanged { message_id: of, .. } if of == message_id)
            }
            Self::Recording(recording_id) => match event {
                Event::RecordingStarted { recording } | Event::RecordingUpdated { recording } => {
                    recording.id == *recording_id
                }
                Event::RecordingRemoved {
                    recording_id: of, ..
                }
                | Event::TranscriptSpeakerRenamed {
                    recording_id: of, ..
                } => of == recording_id,
                // The answer tells the channel of the recording. A change of a channel
                // can change what the account may read of its recordings (ADR 0604).
                Event::ChannelDeleted { .. }
                | Event::ChannelUpdated { .. }
                | Event::WorkspaceDeleted { .. } => true,
                _ => false,
            },
            Self::TranscriptLines(recording_id) => match event {
                Event::TranscriptLineCorrected {
                    recording_id: of, ..
                }
                | Event::RecordingRemoved {
                    recording_id: of, ..
                } => of == recording_id,
                Event::ChannelDeleted { .. }
                | Event::ChannelUpdated { .. }
                | Event::WorkspaceDeleted { .. } => true,
                _ => false,
            },
            Self::Admin(part) => part.concerns(event),
            // The answer to a page that was asked for before every notice was read states
            // its notices as unread: the event is applied to them after the answer.
            Self::MoreNotices(_) => matches!(event, Event::NotificationsRead { .. }),
            // A page after the loaded ones is added only while the list before it is the
            // one that the request continued; no event changes its answer.
            Self::MoreThreads => false,
        }
    }
}

/// The call and the place of an event that changes what a call shows.
fn call_of(event: &Event) -> Option<(CallId, CallPlace)> {
    match event {
        Event::CallStarted { call_id, place, .. }
        | Event::CallEnded { call_id, place }
        | Event::CallParticipantJoined { call_id, place, .. }
        | Event::CallParticipantUpdated { call_id, place, .. }
        | Event::CallParticipantLeft { call_id, place, .. }
        | Event::CallLockChanged { call_id, place, .. }
        | Event::CallMeetingStarted { call_id, place, .. }
        | Event::CallMeetingEnded { call_id, place, .. }
        | Event::CallSpeaking { call_id, place, .. } => Some((*call_id, *place)),
        Event::RecordingStarted { recording } | Event::RecordingUpdated { recording } => {
            Some((recording.call_id, place_of(recording)))
        }
        _ => None,
    }
}

fn place_of(recording: &Recording) -> CallPlace {
    CallPlace::Channel {
        workspace_id: recording.workspace_id,
        channel_id: recording.channel_id,
    }
}

impl WorkspaceModel {
    /// Puts a channel into the workspace. The call of a voice channel keeps the setting
    /// "hides who is inside" of its start (ADR 0517), so a change of the channel changes
    /// nothing of what the state holds of a running call.
    fn put_channel(&mut self, channel: &Channel) {
        match self
            .channels
            .iter_mut()
            .find(|held| held.channel.id == channel.id)
        {
            Some(held) => held.channel = channel.clone(),
            None => self.channels.push(ChannelModel::new(channel.clone())),
        }
        by_position(&mut self.channels, |held| {
            (held.channel.position, held.channel.id.as_uuid().as_u128())
        });
    }
}

impl ServerModel {
    /// Applies one event. The caller moves the cursor.
    pub(crate) fn apply(&mut self, event: &Event) -> Follow {
        let stale = self.apply_admin(event);
        let mut follow = self.apply_event(event);
        follow.load.extend(stale.into_iter().map(Part::Admin));
        follow
    }

    fn apply_event(&mut self, event: &Event) -> Follow {
        match event {
            Event::PermissionsChanged { workspace_id } => {
                self.withdraw(*workspace_id);
                return Follow::withdrawn();
            }
            Event::CallAccessEnded {} => return Follow::reload(),
            Event::AccountProfileUpdated { profile } => {
                if profile.id == self.account.id {
                    self.account.name.clone_from(&profile.name);
                    self.account.avatar.clone_from(&profile.avatar);
                }
                for workspace in &mut self.workspaces {
                    // The description of the avatar of a member is the one of the profile
                    // (ADR 0119).
                    let changed = workspace.members.loaded().is_some_and(|members| {
                        members.iter().any(|member| {
                            member.account_id == profile.id
                                && (member.name != profile.name || member.avatar != profile.avatar)
                        })
                    });
                    if !changed {
                        continue;
                    }
                    if let Some(members) = Arc::make_mut(workspace).members.loaded_mut() {
                        for member in members {
                            if member.account_id == profile.id {
                                member.name.clone_from(&profile.name);
                                member.avatar.clone_from(&profile.avatar);
                            }
                        }
                    }
                }
            }
            Event::WorkspaceCreated { workspace } | Event::WorkspaceUpdated { workspace } => {
                let created = matches!(event, Event::WorkspaceCreated { .. });
                match self
                    .workspaces
                    .iter_mut()
                    .find(|held| held.workspace.id == workspace.id)
                {
                    Some(held) if held.workspace != *workspace => {
                        Arc::make_mut(held).workspace = workspace.clone();
                    }
                    Some(_) => {}
                    None if created => {
                        self.workspaces
                            .push(Arc::new(WorkspaceModel::unlisted(workspace.clone())));
                        self.workspaces.sort_by_key(|held| held.workspace.id);
                    }
                    None => {}
                }
            }
            Event::WorkspaceMemberAdded {
                workspace_id,
                member,
            } => {
                if let Some(members) = self.members_mut(*workspace_id) {
                    upsert(members, member, |held| held.account_id == member.account_id);
                    members.sort_by_key(|held| held.account_id);
                }
            }
            Event::WorkspaceMemberRemoved {
                workspace_id,
                account_id,
            } => {
                if let Some(members) = self.members_mut(*workspace_id) {
                    members.retain(|held| held.account_id != *account_id);
                }
            }
            Event::WorkspaceMemberRolesChanged {
                workspace_id,
                account_id,
                role_ids,
            } => {
                if *account_id == self.account.id
                    && let Some(workspace) = self.workspace_mut(*workspace_id)
                {
                    workspace.own_role_ids.clone_from(role_ids);
                }
                if let Some(members) = self.members_mut(*workspace_id) {
                    for member in members {
                        if member.account_id == *account_id {
                            member.role_ids.clone_from(role_ids);
                        }
                    }
                }
            }
            Event::WorkspaceRoleCreated { role } | Event::WorkspaceRoleUpdated { role } => {
                if let Some(workspace) = self.workspace_mut(role.workspace_id) {
                    upsert(&mut workspace.roles, role, |held| held.id == role.id);
                    sort_workspace_roles(&mut workspace.roles);
                }
            }
            Event::WorkspaceRolePositionsShifted {
                workspace_id,
                positions,
            } => {
                // The event states absolute positions (ADR 0309): a second delivery, or
                // a delivery after a read that held the new positions already, changes
                // nothing.
                let moves = self.workspace(*workspace_id).is_some_and(|workspace| {
                    workspace.roles.iter().any(|held| {
                        positions.iter().any(|moved| {
                            moved.role_id == held.id && moved.position != held.position
                        })
                    })
                });
                if moves && let Some(workspace) = self.workspace_mut(*workspace_id) {
                    for held in &mut workspace.roles {
                        if let Some(moved) = positions.iter().find(|moved| moved.role_id == held.id)
                        {
                            held.position = moved.position;
                        }
                    }
                    sort_workspace_roles(&mut workspace.roles);
                }
            }
            Event::NotificationsRead { until, read_at } => {
                for held in &mut self.notices {
                    if held.read_at.is_none() && held.created_at <= *until {
                        held.read_at = Some(*read_at);
                    }
                }
                // A notice of that time whose transaction ended after the request stays
                // unread on the server (ADR 0412): the list of the server decides.
                return Follow::load(Part::Notices);
            }
            Event::ThreadFollowChanged {
                root_id,
                following,
                follow_setting,
            } => {
                if let Some(held) = self
                    .threads
                    .iter_mut()
                    .find(|held| held.root_id == *root_id)
                {
                    held.following = *following;
                    held.follow_setting = *follow_setting;
                }
                // The replies of a thread that is not followed are not unread, and a
                // thread that is followed from now on enters the lists with its next
                // reply: the server states both (ADR 0412).
                return Follow::load(Part::UnreadThreads);
            }
            Event::NotificationDefaultChanged {
                workspace_id,
                level,
            } => {
                if self.workspace(*workspace_id).is_none() {
                    return Follow::default();
                }
                if let Some(workspace) = self.workspace_mut(*workspace_id) {
                    workspace.notification_default = *level;
                }
                // The level decides the unread signs of every channel of the workspace
                // that sets none of its own, and only the snapshot states them all
                // (ADR 0409).
                return Follow::reload();
            }
            Event::CategoryCreated { category } | Event::CategoryUpdated { category } => {
                if let Some(workspace) = self.workspace_mut(category.workspace_id) {
                    upsert(&mut workspace.categories, category, |held| {
                        held.id == category.id
                    });
                    by_position(&mut workspace.categories, |held| {
                        (held.position, held.id.as_uuid().as_u128())
                    });
                }
            }
            Event::ChannelCreated { channel }
            | Event::ChannelUpdated { channel }
            | Event::ChannelRestored { channel } => {
                // A fork of a thread is a new channel (ADR 0409).
                let mut load = match event {
                    Event::ChannelCreated { .. } => self.thread_heads(),
                    _ => Vec::new(),
                };
                if let Some(workspace) = self.workspace_mut(channel.workspace_id) {
                    workspace.put_channel(channel);
                }
                // A channel that hides who is inside gives a recording of its running
                // call only to the people inside (ADR 0604). The event does not tell
                // what the account may read now: the open recordings of the channel are
                // read again.
                if matches!(event, Event::ChannelUpdated { .. }) {
                    load.extend(
                        self.recordings
                            .iter()
                            .filter(|open| {
                                open.detail
                                    .loaded()
                                    .is_some_and(|detail| detail.recording.channel_id == channel.id)
                            })
                            .map(|open| Part::Recording(open.id)),
                    );
                }
                return Follow::loads(load);
            }
            Event::ChannelDeleted {
                workspace_id,
                channel_id,
            } => {
                let held = self
                    .workspace(*workspace_id)
                    .is_some_and(|workspace| workspace.channel(*channel_id).is_some());
                if held && let Some(workspace) = self.workspace_mut(*workspace_id) {
                    workspace
                        .channels
                        .retain(|channel| channel.channel.id != *channel_id);
                    workspace.forget_channel_permissions(*channel_id);
                }
                self.forget_place(PlaceId::Channel(*channel_id));
                self.lose_recordings_of(*channel_id);
                self.forget_calls_of(*channel_id);
                // The deleted channel can be the fork of an open thread.
                return Follow::loads(self.thread_heads());
            }
            Event::ChannelArchived { channel: archived } => {
                let workspace_id = archived.channel.workspace_id;
                let channel_id = archived.channel.id;
                let held = self
                    .workspace(workspace_id)
                    .is_some_and(|workspace| workspace.channel(channel_id).is_some());
                if held && let Some(workspace) = self.workspace_mut(workspace_id) {
                    workspace
                        .channels
                        .retain(|channel| channel.channel.id != channel_id);
                    // The snapshot states no list for an archived channel (ADR 0310).
                    workspace.forget_channel_permissions(channel_id);
                }
                // The call of an archived voice channel ends (ADR 0410).
                self.forget_calls_of(channel_id);
                // The channel stays readable for a reader that has it open, and for a
                // reader of the archived list. For every other reader only its notices
                // stay.
                if self.readable_channel(channel_id).is_none() {
                    self.close_archived_place(PlaceId::Channel(channel_id));
                }
            }
            Event::CategoryDeleted {
                workspace_id,
                category_id,
            } => {
                if let Some(workspace) = self.workspace_mut(*workspace_id) {
                    workspace.categories.retain(|held| held.id != *category_id);
                    for held in &mut workspace.channels {
                        if held.channel.category_id == Some(*category_id) {
                            held.channel.category_id = None;
                        }
                    }
                }
            }
            Event::WorkspaceRoleDeleted {
                workspace_id,
                role_id,
            } => {
                if let Some(workspace) = self.workspace_mut(*workspace_id) {
                    workspace.roles.retain(|held| held.id != *role_id);
                    workspace.own_role_ids.retain(|held| held != role_id);
                    if let Some(members) = workspace.members.loaded_mut() {
                        for member in members {
                            member.role_ids.retain(|held| held != role_id);
                        }
                    }
                }
            }
            Event::WorkspaceDeleted { workspace_id } => {
                let mut channels: Vec<ChannelId> = self
                    .workspace(*workspace_id)
                    .map(|workspace| {
                        workspace
                            .channels
                            .iter()
                            .map(|held| held.channel.id)
                            .collect()
                    })
                    .unwrap_or_default();
                channels.extend(
                    self.admin
                        .archived(*workspace_id)
                        .loaded()
                        .into_iter()
                        .flatten()
                        .chain(&self.admin.archived_known)
                        .filter(|held| held.channel.workspace_id == *workspace_id)
                        .map(|held| held.channel.id),
                );
                // A late recording answer can outlive the workspace in the model.
                channels.extend(self.recordings.iter().filter_map(|open| {
                    let recording = &open.detail.loaded()?.recording;
                    (recording.workspace_id == *workspace_id).then_some(recording.channel_id)
                }));
                self.workspaces
                    .retain(|held| held.workspace.id != *workspace_id);
                self.account
                    .workspace_ids
                    .retain(|held| held != workspace_id);
                for channel_id in &channels {
                    self.forget_place(PlaceId::Channel(*channel_id));
                    self.forget_calls_of(*channel_id);
                    self.lose_recordings_of(*channel_id);
                }
                self.forget_named_of_workspace(*workspace_id);
                // The settings of the snapshot that name the workspace or one of its
                // channels name nothing any more.
                self.notification_settings.retain(|held| {
                    held.scope_id != workspace_id.as_uuid()
                        && !channels
                            .iter()
                            .any(|channel_id| channel_id.as_uuid() == held.scope_id)
                });
                self.forget_admin_workspace(*workspace_id, &channels);
                // A channel of the workspace can be the fork of an open thread.
                return Follow::loads(self.thread_heads());
            }
            Event::CallStarted {
                hides_occupants,
                call_id,
                place,
                started_at,
            } => {
                // The event states the setting that the call keeps (ADR 0517). An older
                // server does not: the setting is then the one that the state holds.
                let hides = hides_occupants.or_else(|| self.hides(*call_id, place));
                let shown = hides.is_some_and(|hides| !hides || self.has_place_in(*call_id));
                if shown && let Some(call) = self.call_mut_stated(*call_id, place, *hides_occupants)
                {
                    call.started_at = Some(*started_at);
                }
                if let Some(channel) = self.channel_mut(place) {
                    channel.in_use = true;
                    if !shown {
                        channel.call = None;
                    }
                }
            }
            Event::CallEnded { call_id, place } => {
                match place {
                    CallPlace::Channel { .. } => {
                        let other_call =
                            self.call_at(place).is_some_and(|held| held.id != *call_id);
                        if !other_call && let Some(channel) = self.channel_mut(place) {
                            channel.in_use = false;
                            channel.call = None;
                        }
                    }
                    CallPlace::Direct { .. } => {
                        self.direct_calls.retain(|held| held.id != *call_id);
                    }
                }
                self.invitations.retain(|held| held.call_id != *call_id);
                self.entry_requests.retain(|held| held.call_id != *call_id);
                self.own_entry_requests
                    .retain(|held| held.call_id != *call_id);
            }
            Event::CallParticipantJoined {
                call_id,
                place,
                participant,
            }
            | Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            } => {
                if let Some(call) = self.call_mut(*call_id, place, true) {
                    upsert(&mut call.participants, participant, |held| {
                        held.account_id == participant.account_id
                    });
                    // The order in which a snapshot lists the people.
                    call.participants
                        .sort_by_key(|held| (held.joined_at, held.account_id));
                }
                // The server has seen the device of the account in the room. From now on
                // the account reads the call and the requests to enter; before, the
                // server gave it neither, and nothing of a call that hides who is inside
                // (ADR 0517).
                if participant.account_id == self.account.id && self.has_place_in(*call_id) {
                    // An earlier answer that said "not inside" is older than this event.
                    self.own_seat = None;
                    if matches!(event, Event::CallParticipantJoined { .. }) {
                        return Follow::load(Part::OwnCall);
                    }
                }
            }
            Event::CallParticipantLeft {
                call_id,
                place,
                account_id,
                ..
            } => {
                if let Some(call) = self.call_mut(*call_id, place, false) {
                    call.participants
                        .retain(|held| held.account_id != *account_id);
                    call.speaking.retain(|held| held != account_id);
                }
            }
            Event::CallLockChanged {
                call_id,
                place,
                lock,
            } => {
                if let Some(call) = self.call_mut(*call_id, place, true) {
                    call.lock.clone_from(lock);
                }
            }
            Event::CallMeetingStarted {
                call_id,
                place,
                meeting,
            } => {
                if let Some(call) = self.call_mut(*call_id, place, true) {
                    // The event states the rule and carries no participant: everyone
                    // except the presenter who had no block has one now.
                    for participant in &mut call.participants {
                        block_for_meeting(participant, meeting.presenter);
                    }
                    call.meeting = Some(meeting.clone());
                }
            }
            Event::CallMeetingEnded { call_id, place, .. } => {
                if let Some(call) = self.call_mut(*call_id, place, false) {
                    call.meeting = None;
                    for participant in &mut call.participants {
                        participant.microphone_block = None;
                    }
                }
            }
            Event::CallSpeaking {
                call_id,
                place,
                account_ids,
            } => {
                if let Some(call) = self.call_mut(*call_id, place, false) {
                    call.speaking.clone_from(account_ids);
                }
            }
            Event::RecordingStarted { recording } | Event::RecordingUpdated { recording } => {
                self.put_recording(recording);
                if self.update_open_recording(recording) {
                    return Follow::load(Part::Recording(recording.id));
                }
            }
            Event::RecordingRemoved {
                recording_id,
                workspace_id,
                channel_id,
            } => {
                let place = CallPlace::Channel {
                    workspace_id: *workspace_id,
                    channel_id: *channel_id,
                };
                let shown = self.call_at(&place).is_some_and(|call| {
                    call.recording
                        .as_ref()
                        .is_some_and(|held| held.id == *recording_id)
                });
                if shown
                    && let Some(channel) = self.channel_mut(&place)
                    && let Some(call) = channel.call.as_mut()
                {
                    call.recording = None;
                }
                self.lose_recording(*recording_id);
            }
            Event::TranscriptLineCorrected {
                recording_id, line, ..
            } => {
                if self.invalidate_line(*recording_id, *line) {
                    return Follow::load(Part::TranscriptLines(*recording_id));
                }
            }
            Event::TranscriptSpeakerRenamed { recording_id, .. } => {
                if let Some(open) = self.recording_mut(*recording_id).filter(|open| !open.gone) {
                    open.stale |= open.transcript.loaded().is_some();
                    return Follow::load(Part::Recording(*recording_id));
                }
            }
            Event::CallEntryRequested { request } => {
                // The event reaches the people inside and every device of the account
                // that asked.
                let held = if request.account_id == self.account.id {
                    Some(&mut self.own_entry_requests)
                } else if self.has_place_in(request.call_id) {
                    Some(&mut self.entry_requests)
                } else {
                    None
                };
                if let Some(held) = held {
                    upsert(held, request, |held| held.id == request.id);
                }
            }
            Event::CallEntryResolved { request_id, .. } => {
                self.entry_requests.retain(|held| held.id != *request_id);
            }
            Event::CallEntryAnswered { request_id, .. } => {
                self.own_entry_requests
                    .retain(|held| held.id != *request_id);
            }
            Event::CallInvited { invitation } => {
                upsert(&mut self.invitations, invitation, |held| {
                    held.id == invitation.id
                });
            }
            Event::CallInvitationResolved { invitation_id, .. } => {
                self.invitations.retain(|held| held.id != *invitation_id);
            }
            Event::CallMoveRequested { call_move } => {
                upsert(&mut self.moves, call_move, |held| held.id == call_move.id);
            }
            Event::CallMoveResolved { move_id, .. } => {
                self.moves.retain(|held| held.id != *move_id);
            }
            Event::OwnCallChanged { call, .. } => {
                let before = std::mem::replace(&mut self.own_call, call.clone());
                self.own_seat = None;
                self.own_grant = None;
                let same_call = match (&before, call) {
                    (Some(before), Some(now)) => before.call_id == now.call_id,
                    _ => false,
                };
                if !same_call {
                    self.entry_requests.clear();
                    if let Some(before) = before {
                        self.left(&before.place);
                    }
                }
                if call.is_some() {
                    return Follow::load(Part::OwnCall);
                }
            }
            Event::DirectMuteSet { mute } => {
                upsert(&mut self.direct_mutes, mute, |held| {
                    held.account_id == mute.account_id
                });
                self.direct_mutes.sort_by_key(|held| held.account_id);
            }
            Event::DirectMuteRemoved { account_id } => {
                self.direct_mutes
                    .retain(|held| held.account_id != *account_id);
            }
            Event::ConversationOpened { conversation } => {
                self.put_conversation(conversation);
            }
            Event::AttachmentLimitsChanged { limits } => {
                self.attachment_limits = Some(limits.clone());
            }
            Event::ServerLogoChanged { logo } => {
                self.logo = LogoState::Stated(logo.clone());
            }
            Event::MessageMentioned { channel_id, .. } => {
                if self.channel(*channel_id).is_some() {
                    // The mention can be one of a thread (ADR 0409).
                    return Follow::loads(vec![
                        Part::MessageState(PlaceId::Channel(*channel_id)),
                        Part::UnreadThreads,
                    ]);
                }
            }
            Event::MessageChanged {
                place,
                message_id,
                position,
                ..
            } => {
                if !self.holds_place(place) {
                    return Follow::default();
                }
                let id = PlaceId::of(place);
                let known = self.see_position(id, *position);
                let mut load = vec![Part::MessageState(id)];
                // The event carries no content (ADR 0408). The message is read when a
                // holder of messages wants it: a place or a thread that a reader has
                // open, an open pin list, or a list that names the message.
                if self.wants_message(id, *message_id, Some(*position)) {
                    load.push(Part::Message(id, *message_id));
                }
                // The event names no thread. A message of a channel that is not new can
                // be the root of a thread whose replies changed.
                if known && matches!(id, PlaceId::Channel(_)) {
                    load.push(Part::UnreadThreads);
                }
                return Follow::loads(load);
            }
            Event::ThreadRead { root_id, position } => {
                // The event names no channel, and an unread mention of a thread counts
                // in the summary of its channel.
                let mut load: Vec<Part> = self
                    .places_with_mentions()
                    .into_iter()
                    .map(Part::MessageState)
                    .collect();
                let inside = |message: &junto_protocol::message::Message| {
                    message.thread_root == Some(*root_id)
                };
                if self.read_covers_notice(inside, *position) {
                    load.push(Part::Notices);
                }
                if self.move_thread_read(*root_id, *position) == ThreadMoved::Unsettled {
                    load.push(Part::UnreadThreads);
                }
                return Follow::loads(load);
            }
            Event::NotificationChanged { .. } => {
                // A notice of a reply or of a mention in a thread changes the unread
                // threads too.
                return Follow::loads(vec![Part::Notices, Part::UnreadThreads]);
            }
            // The event names nothing. A setting changes the signs of every summary in
            // its scope, and only the snapshot lists every setting (ADR 0409).
            Event::NotificationSettingsChanged {} => return Follow::reload(),
            Event::MessageRead { place, position } => {
                if !self.holds_place(place) {
                    return Follow::default();
                }
                let id = PlaceId::of(place);
                let mut load = Vec::new();
                if !self.move_read_position(*place, *position) {
                    load.push(Part::MessageState(id));
                }
                // Replies of threads have a read position of their own.
                let inside = |message: &junto_protocol::message::Message| {
                    message.thread_root.is_none() && PlaceId::of(&message.place) == id
                };
                if self.read_covers_notice(inside, *position) {
                    load.push(Part::Notices);
                }
                return Follow::loads(load);
            }
            // The collections of these events are read from their routes when a reader
            // needs them (ADR 0207). The collections of the administration that a reader
            // has open follow them (ADR 0760); the state holds none of the others.
            Event::DeviceAdded { .. }
            | Event::DeviceRenamed { .. }
            | Event::DeviceRemoved { .. }
            | Event::DeviceLoggedIn { .. }
            | Event::DeviceLoggedOut { .. }
            | Event::AccountRecovered { .. }
            | Event::AccountAdded { .. }
            | Event::AccountStatusChanged { .. }
            | Event::AccountServerRolesChanged { .. }
            | Event::OwnerChanged { .. }
            | Event::ServerRoleCreated { .. }
            | Event::ServerRoleUpdated { .. }
            | Event::ServerRoleDeleted { .. }
            | Event::ServerRolePositionsShifted { .. }
            | Event::AccessTokenCreated { .. }
            | Event::AccessTokenRevoked { .. }
            | Event::AccessTokensOfCreatorRevoked { .. }
            | Event::ChannelDeletionDue { .. }
            | Event::ExportUpdated { .. }
            | Event::ExportRemoved { .. }
            | Event::InviteCreated { .. }
            | Event::InviteChanged { .. }
            | Event::InviteRevoked { .. }
            | Event::InviteUsed { .. }
            | Event::InvitesOfCreatorRevoked { .. }
            | Event::JoinRequestCreated { .. }
            | Event::JoinRequestDecided { .. }
            | Event::ChannelOverrideSet { .. }
            | Event::GlossaryReplaced { .. }
            | Event::RetentionChanged { .. } => {}
        }
        Follow::default()
    }

    /// Applies again, in their order, the events that arrived while `part` was read.
    pub(crate) fn replay<'a>(&mut self, part: Part, events: impl IntoIterator<Item = &'a Event>) {
        for event in events {
            if part.concerns(event, self) {
                self.apply(event);
            }
        }
    }

    /// For a part whose content no event carries: applies again the events that arrived
    /// while the part was read, and returns the reads that they ask for now. The answer
    /// can be older than those events.
    pub(crate) fn replay_reads<'a>(
        &mut self,
        part: Part,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Vec<Part> {
        let mut reads = Vec::new();
        for event in events {
            if !part.concerns(event, self) {
                continue;
            }
            if let (
                Part::Recording(recording_id) | Part::TranscriptLines(recording_id),
                Event::ChannelUpdated { channel },
            ) = (part, event)
            {
                // Only the recording answer is old; the channel already holds later events.
                if let Some(open) = self.recording_mut(recording_id)
                    && !open.gone
                    && open
                        .detail
                        .loaded()
                        .is_some_and(|held| held.recording.channel_id == channel.id)
                {
                    match part {
                        Part::Recording(_) => open.stale = true,
                        Part::TranscriptLines(_) => open.lines.stale = true,
                        _ => unreachable!(),
                    }
                    if !reads.contains(&part) {
                        reads.push(part);
                    }
                }
                continue;
            }
            for read in self.apply(event).load {
                // A read of a page is followed by the reads of single messages, and each
                // other part by its own read.
                let follows = match part {
                    Part::Messages(..) | Part::Pins(_) => matches!(read, Part::Message(..)),
                    // The event names the place of the message, which the list did not.
                    Part::Named(message_id) => {
                        matches!(read, Part::Message(_, of) if of == message_id)
                    }
                    _ => read == part,
                };
                if follows && !reads.contains(&read) {
                    reads.push(read);
                }
            }
        }
        reads
    }

    /// The threads that a reader has open: their fork channels are read from the route of
    /// the thread.
    fn thread_heads(&self) -> Vec<Part> {
        self.timelines
            .iter()
            .filter(|timeline| timeline.place.thread_root().is_some())
            .map(|timeline| Part::ThreadHead(timeline.place))
            .collect()
    }

    /// True when the state holds the place of a message: a channel that the account can
    /// see, or a direct conversation. An event of a direct conversation that the state
    /// does not hold yet adds the conversation.
    fn holds_place(&mut self, place: &MessagePlace) -> bool {
        match place {
            MessagePlace::Channel { channel_id, .. } => {
                self.readable_channel(*channel_id).is_some()
            }
            MessagePlace::Direct {
                conversation_id,
                low_account_id,
                high_account_id,
            } => {
                let own = self.account.id;
                if *low_account_id != own && *high_account_id != own {
                    return false;
                }
                self.put_conversation(&junto_protocol::message::Conversation {
                    id: *conversation_id,
                    low_account_id: *low_account_id,
                    high_account_id: *high_account_id,
                });
                true
            }
        }
    }

    /// A recording belongs to a call that the state holds: the people in a call start it,
    /// so the events of the call came first. An event of a recording of another call, which
    /// a replay can deliver for an earlier call of the channel (ADR 0604), changes nothing.
    /// The state keeps a recording while it is starting or runs, as the view of a call does.
    fn put_recording(&mut self, recording: &Recording) {
        let place = place_of(recording);
        let Some(held) = self
            .call_at(&place)
            .filter(|call| call.id == recording.call_id && self.shows(call.id, &place))
        else {
            return;
        };
        let next = if recording.state.is_active() {
            Some(recording.clone())
        } else if held
            .recording
            .as_ref()
            .is_some_and(|shown| shown.id == recording.id)
        {
            None
        } else {
            return;
        };
        if held.recording != next
            && let Some(call) = self.call_mut(recording.call_id, &place, false)
        {
            call.recording = next;
        }
    }

    fn members_mut(
        &mut self,
        workspace_id: WorkspaceId,
    ) -> Option<&mut Vec<junto_protocol::structure::WorkspaceMember>> {
        self.workspace(workspace_id)?.members.loaded()?;
        self.workspace_mut(workspace_id)?.members.loaded_mut()
    }

    /// A channel takes no call any more: what waits for a call of it has ended.
    fn forget_calls_of(&mut self, channel_id: ChannelId) {
        let gone = |place: &CallPlace| place.channel_id() == Some(channel_id);
        self.invitations.retain(|held| !gone(&held.place));
        self.entry_requests.retain(|held| !gone(&held.place));
        self.own_entry_requests.retain(|held| !gone(&held.place));
        self.moves
            .retain(|held| !gone(&held.from) && !gone(&held.to));
    }

    /// True when the account has a place in the call `call_id`.
    pub(super) fn has_place_in(&self, call_id: CallId) -> bool {
        self.own_call
            .as_ref()
            .is_some_and(|own| own.call_id == call_id)
    }

    /// Whether the call `call_id` at `place` hides who is inside. A call keeps the setting
    /// of its start (ADR 0517), and the state holds it with the call. For a call that the
    /// state does not hold, the own call of the account states the setting when it is that
    /// call; another call is a new one for the state and takes the setting of its channel.
    fn hides(&self, call_id: CallId, place: &CallPlace) -> Option<bool> {
        match place {
            CallPlace::Channel { channel_id, .. } => {
                let channel = self.channel(*channel_id)?;
                Some(match &channel.call {
                    Some(call) if call.id == call_id => call.hides_occupants,
                    _ => self
                        .own_call
                        .as_ref()
                        .filter(|own| own.call_id == call_id)
                        .and_then(|own| own.hides_occupants)
                        .unwrap_or(channel.channel.hides_occupants),
                })
            }
            CallPlace::Direct { .. } => Some(false),
        }
    }

    /// True when the state holds what the events tell of the call `call_id` at `place`:
    /// the call does not hide who is inside, or the account has a place in it (ADR 0514).
    /// The server gives an account with a place nothing of such a call until it is inside.
    fn shows(&self, call_id: CallId, place: &CallPlace) -> bool {
        self.hides(call_id, place)
            .is_some_and(|hides| !hides || self.has_place_in(call_id))
    }

    /// The call `call_id` at `place`. With `create`, a call that the state does not hold
    /// yet is added, and a call with another identifier at the place is replaced: a
    /// place has one call at a time (ADR 0510).
    fn call_mut(
        &mut self,
        call_id: CallId,
        place: &CallPlace,
        create: bool,
    ) -> Option<&mut CallModel> {
        let hides = self.hides(call_id, place)?;
        self.call_mut_with(call_id, place, create, hides)
    }

    /// The call `call_id` at `place` for the event of its start, which states the setting
    /// that the call keeps when `stated` is present (ADR 0517). The call is added when the
    /// state does not hold it, and a call that the state holds takes the stated setting.
    fn call_mut_stated(
        &mut self,
        call_id: CallId,
        place: &CallPlace,
        stated: Option<bool>,
    ) -> Option<&mut CallModel> {
        let hides = stated.or_else(|| self.hides(call_id, place))?;
        let call = self.call_mut_with(call_id, place, true, hides)?;
        call.hides_occupants = hides;
        Some(call)
    }

    fn call_mut_with(
        &mut self,
        call_id: CallId,
        place: &CallPlace,
        create: bool,
        hides: bool,
    ) -> Option<&mut CallModel> {
        // An account without a place in a call that hides who is inside holds no details
        // of that call. The server sends it none; an event that was kept during a read is
        // applied by what holds now.
        if hides && !self.has_place_in(call_id) {
            return None;
        }
        let held = self.call_at(place).is_some_and(|call| call.id == call_id);
        if !held && !create {
            return None;
        }
        match place {
            CallPlace::Channel { .. } => {
                let channel = self.channel_mut(place)?;
                if !held {
                    if channel.call.is_some() {
                        channel.in_use = false;
                    }
                    channel.call = Some(CallModel::new(call_id, *place, hides));
                }
                channel.call.as_mut()
            }
            CallPlace::Direct { .. } => {
                if !held {
                    self.direct_calls.retain(|call| call.place != *place);
                    self.direct_calls
                        .push(CallModel::new(call_id, *place, hides));
                }
                self.direct_calls.iter_mut().find(|call| call.id == call_id)
            }
        }
    }

    /// The account is no longer in the call at `place`. It keeps of that call only what
    /// an account outside is told: nothing of a call that is not in use, and nothing of a
    /// call that hides who is inside.
    fn left(&mut self, place: &CallPlace) {
        match place {
            CallPlace::Channel { channel_id, .. } => {
                let drop = self.channel(*channel_id).is_some_and(|held| {
                    held.call
                        .as_ref()
                        .is_some_and(|call| call.hides_occupants || call.started_at.is_none())
                });
                if drop && let Some(channel) = self.channel_mut(place) {
                    channel.call = None;
                }
            }
            CallPlace::Direct { .. } => self
                .direct_calls
                .retain(|call| call.place != *place || call.started_at.is_some()),
        }
    }
}

fn block_for_meeting(participant: &mut CallParticipant, presenter: junto_protocol::ids::AccountId) {
    if participant.account_id != presenter && participant.microphone_block.is_none() {
        participant.microphone_block = Some(MicrophoneBlock {
            by: presenter,
            reason: BlockReason::Meeting,
        });
    }
}
