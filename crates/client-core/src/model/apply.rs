//! How each event changes the state (ADR 0208, ADR 0510).
//!
//! An event carries the whole changed object, so applying one twice changes nothing. An
//! event about an object whose parent the state does not hold is not applied: the parent
//! became visible by a permission change, and the reload after that change brings both.

use std::sync::Arc;

use junto_protocol::call::{BlockReason, CallParticipant, CallPlace, MicrophoneBlock};
use junto_protocol::events::Event;
use junto_protocol::ids::{CallId, ChannelId, WorkspaceId};
use junto_protocol::structure::Channel;

use super::{CallModel, ChannelModel, Loadable, ServerModel, WorkspaceModel};

/// A part of the state that is read from a route of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Members(WorkspaceId),
    /// The call of the account with its requests to enter.
    OwnCall,
    /// The call of one voice channel.
    ChannelCall(ChannelId),
}

/// What the event stream client does after an event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Follow {
    /// The state must be replaced by a snapshot (ADR 0207, ADR 0208, ADR 0510).
    pub(crate) reload: bool,
    /// The parts to read now. A read of the same part that is on its way is dropped.
    pub(crate) load: Vec<Part>,
}

impl Follow {
    fn reload() -> Self {
        Self {
            reload: true,
            load: Vec::new(),
        }
    }

    fn load(part: Part) -> Self {
        Self {
            reload: false,
            load: vec![part],
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
                    _ => call_of(event).is_some_and(|(call_id, _)| call_id == own.call_id),
                }
            }
            Self::ChannelCall(channel_id) => {
                call_of(event).is_some_and(|(_, place)| place.channel_id() == Some(*channel_id))
            }
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
        _ => None,
    }
}

impl WorkspaceModel {
    fn put_channel(&mut self, channel: &Channel, inside: bool) -> Option<Part> {
        let mut load = None;
        match self
            .channels
            .iter_mut()
            .find(|held| held.channel.id == channel.id)
        {
            Some(held) => {
                let was_hidden = held.channel.hides_occupants;
                held.channel = channel.clone();
                if channel.hides_occupants && !inside {
                    held.call = None;
                } else if was_hidden && !channel.hides_occupants && held.in_use && !inside {
                    // The people inside are shown from now on, and no event tells who
                    // they are.
                    load = Some(Part::ChannelCall(channel.id));
                }
            }
            None => self.channels.push(ChannelModel::new(channel.clone())),
        }
        by_position(&mut self.channels, |held| {
            (held.channel.position, held.channel.id.as_uuid().as_u128())
        });
        load
    }
}

impl ServerModel {
    /// Applies one event. The caller moves the cursor.
    pub(crate) fn apply(&mut self, event: &Event) -> Follow {
        match event {
            Event::PermissionsChanged { .. } | Event::CallAccessEnded {} => {
                return Follow::reload();
            }
            Event::AccountProfileUpdated { profile } => {
                if profile.id == self.account.id {
                    self.account.name.clone_from(&profile.name);
                    self.account.avatar.clone_from(&profile.avatar);
                }
                for workspace in &mut self.workspaces {
                    let renamed = workspace.members.loaded().is_some_and(|members| {
                        members.iter().any(|member| {
                            member.account_id == profile.id && member.name != profile.name
                        })
                    });
                    if !renamed {
                        continue;
                    }
                    if let Some(members) = Arc::make_mut(workspace).members.loaded_mut() {
                        for member in members {
                            if member.account_id == profile.id {
                                member.name.clone_from(&profile.name);
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
                        self.workspaces.push(Arc::new(WorkspaceModel {
                            workspace: workspace.clone(),
                            own_role_ids: Vec::new(),
                            roles: Vec::new(),
                            categories: Vec::new(),
                            channels: Vec::new(),
                            members: Loadable::NotLoaded,
                        }));
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
                    workspace
                        .roles
                        .sort_by_key(|held| (!held.is_everyone, held.id));
                }
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
            Event::ChannelCreated { channel } | Event::ChannelUpdated { channel } => {
                let inside = self.is_inside(channel.id);
                if let Some(workspace) = self.workspace_mut(channel.workspace_id)
                    && let Some(part) = workspace.put_channel(channel, inside)
                {
                    return Follow::load(part);
                }
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
                }
                let gone = |place: &CallPlace| place.channel_id() == Some(*channel_id);
                self.invitations.retain(|held| !gone(&held.place));
                self.entry_requests.retain(|held| !gone(&held.place));
                self.moves
                    .retain(|held| !gone(&held.from) && !gone(&held.to));
            }
            Event::CallStarted {
                call_id,
                place,
                started_at,
            } => {
                let shown = self.shows_inside(place);
                if shown && let Some(call) = self.call_mut(*call_id, place, true) {
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
            Event::CallEntryRequested { request } => {
                let own = self.own_call.as_ref().map(|own| own.call_id);
                if own == Some(request.call_id) {
                    upsert(&mut self.entry_requests, request, |held| {
                        held.id == request.id
                    });
                }
            }
            Event::CallEntryResolved { request_id, .. } => {
                self.entry_requests.retain(|held| held.id != *request_id);
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
            // The collections of these events are read from their routes when a reader
            // needs them (ADR 0207). The state holds none of them.
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
            | Event::InviteCreated { .. }
            | Event::InviteRevoked { .. }
            | Event::InviteUsed { .. }
            | Event::InvitesOfCreatorRevoked { .. }
            | Event::JoinRequestCreated { .. }
            | Event::JoinRequestDecided { .. }
            | Event::ChannelOverrideSet { .. }
            | Event::CallEntryAnswered { .. } => {}
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

    fn members_mut(
        &mut self,
        workspace_id: WorkspaceId,
    ) -> Option<&mut Vec<junto_protocol::structure::WorkspaceMember>> {
        self.workspace(workspace_id)?.members.loaded()?;
        self.workspace_mut(workspace_id)?.members.loaded_mut()
    }

    fn is_inside(&self, channel_id: ChannelId) -> bool {
        self.own_call
            .as_ref()
            .is_some_and(|own| own.place.channel_id() == Some(channel_id))
    }

    /// True when the account may see who is in the call at `place` (ADR 0514).
    pub(super) fn shows_inside(&self, place: &CallPlace) -> bool {
        match place {
            CallPlace::Channel { channel_id, .. } => self
                .channel(*channel_id)
                .is_some_and(|held| !held.channel.hides_occupants || self.is_inside(*channel_id)),
            CallPlace::Direct { .. } => true,
        }
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
        // An account outside a voice channel that hides who is inside holds no details of
        // its call. The server sends it none; an event that was kept during a read is
        // applied by the setting of now.
        if !self.shows_inside(place) {
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
                    channel.call = Some(CallModel::new(call_id, *place));
                }
                channel.call.as_mut()
            }
            CallPlace::Direct { .. } => {
                if !held {
                    self.direct_calls.retain(|call| call.place != *place);
                    self.direct_calls.push(CallModel::new(call_id, *place));
                }
                self.direct_calls.iter_mut().find(|call| call.id == call_id)
            }
        }
    }

    /// The account is no longer in the call at `place`. It keeps of that call only what
    /// an account outside is told: nothing of a call that is not in use, and nothing of a
    /// voice channel that hides who is inside.
    fn left(&mut self, place: &CallPlace) {
        match place {
            CallPlace::Channel { channel_id, .. } => {
                let drop = self.channel(*channel_id).is_some_and(|held| {
                    held.channel.hides_occupants
                        || held
                            .call
                            .as_ref()
                            .is_some_and(|call| call.started_at.is_none())
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
