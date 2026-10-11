use std::sync::Arc;

use junto_protocol::account::{AccountProfile, OwnAccount};
use junto_protocol::call::{
    BlockReason, Call, CallInvitation, CallLock, CallMove, CallParticipant, CallPlace, ChannelCall,
    DirectMute, EntryOutcome, EntryRequest, InvitationOutcome, LeaveReason, Meeting, MeetingEnd,
    MicrophoneBlock, MoveOutcome, OwnCall, OwnCallState,
};
use junto_protocol::events::{Cursor, Event};
use junto_protocol::ids::{
    AccountId, CallId, CallInvitationId, CallMoveId, CategoryId, ChannelId, DeviceId,
    EntryRequestId, StreamEpoch, WorkspaceId, WorkspaceRoleId,
};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::snapshot::{Snapshot, WorkspaceState};
use junto_protocol::structure::{Category, Channel, ChannelKind, Workspace, WorkspaceMember};
use junto_protocol::{ServerPermission, WorkspacePermission};
use time::OffsetDateTime;

use super::*;

pub(crate) fn time() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

/// One workspace with a text channel, a voice channel and a voice channel that hides who
/// is inside, as an account without a call sees it.
pub(crate) struct Scene {
    pub(crate) account_id: AccountId,
    pub(crate) device_id: DeviceId,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) everyone: WorkspaceRoleId,
    pub(crate) helpers: WorkspaceRoleId,
    pub(crate) general: ChannelId,
    pub(crate) daily: ChannelId,
    pub(crate) hidden: ChannelId,
    pub(crate) snapshot: Snapshot,
}

pub(crate) fn channel(
    workspace_id: WorkspaceId,
    kind: ChannelKind,
    name: &str,
    position: i32,
) -> Channel {
    Channel {
        id: ChannelId::generate(),
        workspace_id,
        category_id: None,
        kind,
        name: name.to_owned(),
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    }
}

pub(crate) fn scene() -> Scene {
    let account_id = AccountId::generate();
    let workspace_id = WorkspaceId::generate();
    let role = |name: &str, permissions: &[WorkspacePermission], is_everyone| WorkspaceRole {
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: name.to_owned(),
        permissions: permissions.to_vec(),
        is_everyone,
    };
    let everyone = role("everyone", &[WorkspacePermission::ViewChannel], true);
    let helpers = role("helpers", &[WorkspacePermission::MuteMembers], false);
    let unheld = role("leads", &[WorkspacePermission::ManageChannel], false);
    let general = channel(workspace_id, ChannelKind::Text, "general", 0);
    let daily = channel(workspace_id, ChannelKind::Voice, "Daily", 1);
    let mut hidden = channel(workspace_id, ChannelKind::Voice, "HR", 2);
    hidden.hides_occupants = true;
    let snapshot = Snapshot {
        cursor: Cursor {
            epoch: StreamEpoch::generate(),
            position: 40,
        },
        account: OwnAccount {
            id: account_id,
            name: "Luca".to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: vec![workspace_id],
        },
        workspaces: vec![WorkspaceState {
            workspace: Workspace {
                id: workspace_id,
                name: "Studio".to_owned(),
                is_default: true,
            },
            own_role_ids: vec![helpers.id],
            roles: vec![everyone.clone(), helpers.clone(), unheld],
            categories: Vec::new(),
            channels: vec![general.clone(), daily.clone(), hidden.clone()],
            calls: Vec::new(),
        }],
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        direct_mutes: Vec::new(),
    };
    Scene {
        account_id,
        device_id: DeviceId::generate(),
        workspace_id,
        everyone: everyone.id,
        helpers: helpers.id,
        general: general.id,
        daily: daily.id,
        hidden: hidden.id,
        snapshot,
    }
}

impl Scene {
    pub(crate) fn model(&self) -> ServerModel {
        ServerModel::from_snapshot(self.snapshot.clone())
    }

    pub(crate) fn place(&self, channel_id: ChannelId) -> CallPlace {
        CallPlace::Channel {
            workspace_id: self.workspace_id,
            channel_id,
        }
    }

    pub(crate) fn own_call(&self, call_id: CallId, channel_id: ChannelId) -> OwnCall {
        OwnCall {
            call_id,
            place: self.place(channel_id),
            device_id: self.device_id,
            identity: "own-seat".to_owned(),
        }
    }
}

pub(crate) fn participant(account_id: AccountId) -> CallParticipant {
    CallParticipant {
        account_id,
        identity: format!("seat-{account_id}"),
        joined_at: time(),
        muted: false,
        deafened: false,
        microphone_block: None,
        sound_block: None,
        hand_raised: false,
        screen_share: None,
    }
}

pub(crate) fn member(name: &str) -> WorkspaceMember {
    WorkspaceMember {
        account_id: AccountId::generate(),
        name: name.to_owned(),
        role_ids: Vec::new(),
    }
}

fn joined(call_id: CallId, place: CallPlace, participant: &CallParticipant) -> Event {
    Event::CallParticipantJoined {
        call_id,
        place,
        participant: participant.clone(),
    }
}

fn started(call_id: CallId, place: CallPlace) -> Event {
    Event::CallStarted {
        call_id,
        place,
        started_at: time(),
    }
}

fn names(model: &ServerModel, workspace_id: WorkspaceId) -> Vec<String> {
    model
        .workspace(workspace_id)
        .unwrap()
        .channels
        .iter()
        .map(|held| held.channel.name.clone())
        .collect()
}

#[test]
fn a_snapshot_gives_the_structure_the_calls_and_the_permissions_of_the_roles() {
    let mut scene = scene();
    let call_id = CallId::generate();
    let other = AccountId::generate();
    scene.snapshot.workspaces[0].calls = vec![
        ChannelCall {
            workspace_id: scene.workspace_id,
            channel_id: scene.daily,
            in_use: true,
            call: Some(Call {
                id: call_id,
                place: scene.place(scene.daily),
                started_at: Some(time()),
                lock: None,
                meeting: None,
                participants: vec![participant(other)],
            }),
        },
        ChannelCall {
            workspace_id: scene.workspace_id,
            channel_id: scene.hidden,
            in_use: true,
            call: None,
        },
    ];
    let model = scene.model();

    assert_eq!(model.cursor, scene.snapshot.cursor);
    assert_eq!(
        names(&model, scene.workspace_id),
        ["general", "Daily", "HR"]
    );
    let daily = model.channel(scene.daily).unwrap();
    assert!(daily.in_use);
    let call = daily.call.as_ref().unwrap();
    assert_eq!(call.id, call_id);
    assert!(call.participant(other).is_some());
    assert!(call.speaking.is_empty());
    assert_eq!(
        model.call_at(&scene.place(scene.daily)).unwrap().id,
        call_id
    );
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use && hidden.call.is_none());
    assert!(!model.channel(scene.general).unwrap().in_use);

    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert_eq!(workspace.members, Loadable::NotLoaded);
    assert_eq!(
        workspace.role_permissions(),
        [
            WorkspacePermission::ViewChannel,
            WorkspacePermission::MuteMembers
        ]
    );
    assert!(!model.is_administrator());
    let mut owner = scene.model();
    owner.account.is_owner = true;
    assert!(owner.is_administrator());
    let mut administrator = scene.model();
    administrator.account.server_permissions = vec![ServerPermission::Administrator];
    assert!(administrator.is_administrator());
}

#[test]
fn structure_events_replace_whole_objects_and_keep_the_order_of_positions() {
    let scene = scene();
    let mut model = scene.model();

    let mut first = channel(scene.workspace_id, ChannelKind::Text, "announcements", -1);
    assert_eq!(
        model.apply(&Event::ChannelCreated {
            channel: first.clone()
        }),
        Follow::default()
    );
    assert_eq!(
        names(&model, scene.workspace_id),
        ["announcements", "general", "Daily", "HR"]
    );
    first.name = "news".to_owned();
    first.position = 5;
    model.apply(&Event::ChannelUpdated {
        channel: first.clone(),
    });
    assert_eq!(
        names(&model, scene.workspace_id),
        ["general", "Daily", "HR", "news"]
    );
    // The same event again changes nothing.
    let before = model.clone();
    model.apply(&Event::ChannelUpdated {
        channel: first.clone(),
    });
    assert_eq!(model, before);
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: first.id,
    });
    assert_eq!(
        names(&model, scene.workspace_id),
        ["general", "Daily", "HR"]
    );

    let category = |name: &str, position| Category {
        id: CategoryId::generate(),
        workspace_id: scene.workspace_id,
        name: name.to_owned(),
        position,
    };
    let (projects, archive) = (category("Projects", 2), category("Archive", 1));
    model.apply(&Event::CategoryCreated {
        category: projects.clone(),
    });
    model.apply(&Event::CategoryCreated {
        category: archive.clone(),
    });
    let renamed = Category {
        name: "Old".to_owned(),
        ..archive
    };
    model.apply(&Event::CategoryUpdated {
        category: renamed.clone(),
    });
    assert_eq!(
        model.workspace(scene.workspace_id).unwrap().categories,
        [renamed, projects]
    );

    let role = WorkspaceRole {
        id: scene.helpers,
        workspace_id: scene.workspace_id,
        name: "helpers".to_owned(),
        permissions: vec![WorkspacePermission::MoveMembers],
        is_everyone: false,
    };
    model.apply(&Event::WorkspaceRoleUpdated { role });
    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert_eq!(workspace.roles.len(), 3);
    assert_eq!(workspace.roles[0].id, scene.everyone);
    assert_eq!(
        workspace.role_permissions(),
        [
            WorkspacePermission::ViewChannel,
            WorkspacePermission::MoveMembers
        ]
    );

    let created = Workspace {
        id: WorkspaceId::generate(),
        name: "Lab".to_owned(),
        is_default: false,
    };
    model.apply(&Event::WorkspaceCreated {
        workspace: created.clone(),
    });
    model.apply(&Event::WorkspaceUpdated {
        workspace: Workspace {
            name: "Laboratory".to_owned(),
            ..created.clone()
        },
    });
    assert_eq!(model.workspaces.len(), 2);
    assert_eq!(
        model.workspace(created.id).unwrap().workspace.name,
        "Laboratory"
    );
    assert!(model.workspace(created.id).unwrap().channels.is_empty());
}

#[test]
fn a_copy_shares_the_workspaces_that_an_event_did_not_change() {
    let scene = scene();
    let mut model = scene.model();
    let other = Workspace {
        id: WorkspaceId::generate(),
        name: "Lab".to_owned(),
        is_default: false,
    };
    model.apply(&Event::WorkspaceCreated {
        workspace: other.clone(),
    });
    let copy = model.clone();
    let position = |model: &ServerModel, id| {
        model
            .workspaces
            .iter()
            .position(|held| held.workspace.id == id)
            .unwrap()
    };
    let (studio, lab) = (
        position(&model, scene.workspace_id),
        position(&model, other.id),
    );
    model.apply(&Event::ChannelCreated {
        channel: channel(other.id, ChannelKind::Text, "bench", 0),
    });
    assert!(Arc::ptr_eq(
        &model.workspaces[studio],
        &copy.workspaces[studio]
    ));
    assert!(!Arc::ptr_eq(&model.workspaces[lab], &copy.workspaces[lab]));
    assert!(copy.workspace(other.id).unwrap().channels.is_empty());

    // An event that changes nothing copies nothing.
    let copy = model.clone();
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: ChannelId::generate(),
    });
    model.apply(&Event::CallSpeaking {
        call_id: CallId::generate(),
        place: scene.place(scene.daily),
        account_ids: vec![scene.account_id],
    });
    assert!(Arc::ptr_eq(
        &model.workspaces[studio],
        &copy.workspaces[studio]
    ));
}

#[test]
fn an_event_about_a_parent_that_the_state_does_not_hold_changes_nothing() {
    let scene = scene();
    let mut model = scene.model();
    let before = model.clone();
    let elsewhere = WorkspaceId::generate();
    let unknown_place = CallPlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: ChannelId::generate(),
    };
    let events = [
        Event::WorkspaceUpdated {
            workspace: Workspace {
                id: elsewhere,
                name: "Hidden".to_owned(),
                is_default: false,
            },
        },
        Event::ChannelCreated {
            channel: channel(elsewhere, ChannelKind::Text, "secret", 0),
        },
        Event::CategoryCreated {
            category: Category {
                id: CategoryId::generate(),
                workspace_id: elsewhere,
                name: "Plans".to_owned(),
                position: 0,
            },
        },
        started(CallId::generate(), unknown_place),
        joined(
            CallId::generate(),
            unknown_place,
            &participant(AccountId::generate()),
        ),
        Event::WorkspaceMemberAdded {
            workspace_id: scene.workspace_id,
            member: member("Not loaded"),
        },
    ];
    for event in events {
        assert_eq!(model.apply(&event), Follow::default(), "{event:?}");
    }
    assert_eq!(model, before);
}

#[test]
fn a_call_in_a_voice_channel_follows_its_events() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let (ana, bruno) = (
        participant(AccountId::generate()),
        participant(AccountId::generate()),
    );

    model.apply(&started(call_id, place));
    let daily = model.channel(scene.daily).unwrap();
    assert!(daily.in_use);
    assert_eq!(daily.call.as_ref().unwrap().started_at, Some(time()));
    model.apply(&joined(call_id, place, &ana));
    model.apply(&joined(call_id, place, &bruno));
    model.apply(&joined(call_id, place, &ana));
    let muted = CallParticipant {
        muted: true,
        hand_raised: true,
        ..bruno.clone()
    };
    model.apply(&Event::CallParticipantUpdated {
        call_id,
        place,
        participant: muted.clone(),
    });
    model.apply(&Event::CallSpeaking {
        call_id,
        place,
        account_ids: vec![ana.account_id, bruno.account_id],
    });
    let lock = CallLock {
        by: ana.account_id,
        at: time(),
    };
    model.apply(&Event::CallLockChanged {
        call_id,
        place,
        lock: Some(lock.clone()),
    });
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.participants, [ana.clone(), muted]);
    assert_eq!(call.speaking, [ana.account_id, bruno.account_id]);
    assert_eq!(call.lock, Some(lock));

    model.apply(&Event::CallParticipantLeft {
        call_id,
        place,
        account_id: ana.account_id,
        reason: LeaveReason::Left,
    });
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.participants.len(), 1);
    assert_eq!(call.speaking, [bruno.account_id]);

    // An event of a call that the place no longer has changes nothing.
    let before = model.clone();
    model.apply(&Event::CallParticipantLeft {
        call_id: CallId::generate(),
        place,
        account_id: bruno.account_id,
        reason: LeaveReason::Left,
    });
    model.apply(&Event::CallEnded {
        call_id: CallId::generate(),
        place,
    });
    assert_eq!(model, before);

    model.apply(&Event::CallEnded { call_id, place });
    let daily = model.channel(scene.daily).unwrap();
    assert!(!daily.in_use && daily.call.is_none());

    // The next call of the channel has another identifier and starts empty.
    let next = CallId::generate();
    model.apply(&Event::CallLockChanged {
        call_id: next,
        place,
        lock: None,
    });
    assert!(!model.channel(scene.daily).unwrap().in_use);
    model.apply(&started(next, place));
    let daily = model.channel(scene.daily).unwrap();
    assert!(daily.in_use);
    assert_eq!(daily.call.as_ref().unwrap().id, next);
    assert!(daily.call.as_ref().unwrap().participants.is_empty());
}

#[test]
fn meeting_mode_blocks_every_microphone_except_the_presenters_and_its_end_removes_every_block() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let presenter = participant(AccountId::generate());
    let listener = participant(AccountId::generate());
    let moderator = AccountId::generate();
    let blocked = CallParticipant {
        microphone_block: Some(MicrophoneBlock {
            by: moderator,
            reason: BlockReason::Moderator,
        }),
        ..participant(AccountId::generate())
    };
    model.apply(&started(call_id, place));
    for person in [&presenter, &listener, &blocked] {
        model.apply(&joined(call_id, place, person));
    }
    let meeting = Meeting {
        presenter: presenter.account_id,
        started_at: time(),
    };
    model.apply(&Event::CallMeetingStarted {
        call_id,
        place,
        meeting: meeting.clone(),
    });
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.meeting, Some(meeting));
    assert_eq!(
        call.participant(presenter.account_id)
            .unwrap()
            .microphone_block,
        None
    );
    assert_eq!(
        call.participant(listener.account_id)
            .unwrap()
            .microphone_block,
        Some(MicrophoneBlock {
            by: presenter.account_id,
            reason: BlockReason::Meeting
        })
    );
    assert_eq!(
        call.participant(blocked.account_id)
            .unwrap()
            .microphone_block
            .as_ref()
            .unwrap()
            .by,
        moderator
    );

    model.apply(&Event::CallMeetingEnded {
        call_id,
        place,
        reason: MeetingEnd::ShareStopped,
    });
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.meeting, None);
    assert!(
        call.participants
            .iter()
            .all(|person| person.microphone_block.is_none())
    );
}

#[test]
fn a_voice_channel_that_hides_who_is_inside_shows_only_that_it_is_in_use() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.hidden));
    model.apply(&started(call_id, place));
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use && hidden.call.is_none());

    // The account enters: it is told who is inside from then on.
    let follow = model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(call_id, scene.hidden)),
        left: None,
    });
    assert_eq!(follow.load, [Part::OwnCall]);
    let inside = participant(AccountId::generate());
    model.apply(&joined(call_id, place, &inside));
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use);
    assert_eq!(hidden.call.as_ref().unwrap().participants, [inside]);

    // It leaves: it keeps nothing of the people inside.
    let follow = model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::Left),
    });
    assert_eq!(follow, Follow::default());
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use && hidden.call.is_none());
    model.apply(&Event::CallEnded { call_id, place });
    assert!(!model.channel(scene.hidden).unwrap().in_use);
}

#[test]
fn a_change_of_the_hide_setting_drops_or_asks_for_the_people_inside() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&started(call_id, place));
    model.apply(&joined(call_id, place, &participant(AccountId::generate())));
    let mut daily = model.channel(scene.daily).unwrap().channel.clone();

    daily.hides_occupants = true;
    let follow = model.apply(&Event::ChannelUpdated {
        channel: daily.clone(),
    });
    assert_eq!(follow, Follow::default());
    let held = model.channel(scene.daily).unwrap();
    assert!(held.in_use && held.call.is_none());

    daily.hides_occupants = false;
    let follow = model.apply(&Event::ChannelUpdated {
        channel: daily.clone(),
    });
    assert_eq!(follow.load, [Part::ChannelCall(scene.daily)]);
    assert!(!follow.reload);

    // A voice channel that is not in use has nobody to ask for.
    let mut hidden = model.channel(scene.hidden).unwrap().channel.clone();
    hidden.hides_occupants = false;
    assert_eq!(
        model.apply(&Event::ChannelUpdated { channel: hidden }),
        Follow::default()
    );
}

#[test]
fn the_account_that_leaves_a_call_that_never_started_keeps_nothing_of_it() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let own = scene.own_call(call_id, scene.daily);
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    model.set_own_call(Some(OwnCallState {
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: None,
            lock: None,
            meeting: None,
            participants: Vec::new(),
        },
        entry_requests: Vec::new(),
    }));
    let daily = model.channel(scene.daily).unwrap();
    assert!(!daily.in_use);
    assert_eq!(daily.call.as_ref().unwrap().id, call_id);
    assert!(model.in_call_with(scene.device_id));
    assert!(!model.in_call_with(DeviceId::generate()));

    model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::NeverConnected),
    });
    assert!(model.channel(scene.daily).unwrap().call.is_none());
    assert!(!model.in_call_with(scene.device_id));
}

#[test]
fn the_answer_for_the_own_call_gives_the_people_and_the_requests_of_that_call_only() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let own = scene.own_call(call_id, scene.daily);
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    model.apply(&started(call_id, place));
    model.apply(&Event::CallSpeaking {
        call_id,
        place,
        account_ids: vec![scene.account_id],
    });
    let request = EntryRequest {
        id: EntryRequestId::generate(),
        call_id,
        place,
        account_id: AccountId::generate(),
        created_at: time(),
        expires_at: time(),
    };
    let loaded = |call_id| OwnCallState {
        own: OwnCall {
            call_id,
            ..own.clone()
        },
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            participants: vec![participant(scene.account_id)],
        },
        entry_requests: vec![request.clone()],
    };
    // The answer of a read for a call that the account has left is not used.
    let before = model.clone();
    model.set_own_call(Some(loaded(CallId::generate())));
    model.set_own_call(None);
    assert_eq!(model, before);

    model.set_own_call(Some(loaded(call_id)));
    assert_eq!(model.entry_requests, std::slice::from_ref(&request));
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.participants.len(), 1);
    assert_eq!(call.speaking, [scene.account_id]);
    assert!(model.channel(scene.daily).unwrap().in_use);

    // The call moves to another device of the account: the requests stay.
    let follow = model.apply(&Event::OwnCallChanged {
        call: Some(OwnCall {
            device_id: DeviceId::generate(),
            ..own
        }),
        left: None,
    });
    assert_eq!(follow.load, [Part::OwnCall]);
    assert_eq!(model.entry_requests, std::slice::from_ref(&request));

    model.apply(&Event::CallEntryResolved {
        request_id: request.id,
        call_id,
        place,
        account_id: request.account_id,
        outcome: EntryOutcome::Accepted,
        by: Some(scene.account_id),
    });
    assert!(model.entry_requests.is_empty());
    model.apply(&Event::CallEntryRequested {
        request: request.clone(),
    });
    assert_eq!(model.entry_requests.len(), 1);
    model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::Left),
    });
    assert!(model.entry_requests.is_empty());
    // A request for a call that the account is not in is not held.
    model.apply(&Event::CallEntryRequested { request });
    assert!(model.entry_requests.is_empty());
}

#[test]
fn invitations_and_moves_wait_until_their_outcome_or_the_end_of_their_place() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let invitation = |account_id, invited_by| CallInvitation {
        id: CallInvitationId::generate(),
        call_id,
        place,
        account_id,
        invited_by,
        created_at: time(),
        expires_at: time(),
    };
    let other = AccountId::generate();
    let got = invitation(scene.account_id, other);
    let made = invitation(other, scene.account_id);
    for invitation in [&got, &made, &got] {
        model.apply(&Event::CallInvited {
            invitation: invitation.clone(),
        });
    }
    assert_eq!(model.invitations, [got.clone(), made.clone()]);
    assert_eq!(model.incoming_invitations().collect::<Vec<_>>(), [&got]);
    model.apply(&Event::CallInvitationResolved {
        invitation_id: made.id,
        outcome: InvitationOutcome::Declined,
    });
    assert_eq!(model.invitations, std::slice::from_ref(&got));
    model.apply(&Event::CallEnded { call_id, place });
    assert!(model.invitations.is_empty());

    let call_move = CallMove {
        id: CallMoveId::generate(),
        account_id: scene.account_id,
        from: place,
        to: scene.place(scene.hidden),
        moved_by: other,
        expires_at: time(),
    };
    model.apply(&Event::CallMoveRequested {
        call_move: call_move.clone(),
    });
    assert_eq!(model.moves, std::slice::from_ref(&call_move));
    model.apply(&Event::CallMoveResolved {
        move_id: call_move.id,
        outcome: MoveOutcome::TimedOut,
    });
    assert!(model.moves.is_empty());

    // The deletion of a channel ends what waited there.
    model.apply(&Event::CallMoveRequested { call_move });
    model.apply(&Event::CallInvited { invitation: got });
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    });
    assert!(model.moves.is_empty() && model.invitations.is_empty());
    assert!(model.channel(scene.daily).is_none());
}

#[test]
fn a_permission_change_and_an_ended_access_ask_for_a_reload_and_change_nothing() {
    let scene = scene();
    let mut model = scene.model();
    let before = model.clone();
    for event in [
        Event::PermissionsChanged {
            workspace_id: Some(scene.workspace_id),
        },
        Event::PermissionsChanged { workspace_id: None },
        Event::CallAccessEnded {},
    ] {
        let follow = model.apply(&event);
        assert!(follow.reload && follow.load.is_empty(), "{event:?}");
    }
    assert_eq!(model, before);
}

#[test]
fn members_follow_the_events_once_they_are_loaded() {
    let scene = scene();
    let mut model = scene.model();
    let (ana, bruno) = (member("Ana"), member("Bruno"));
    let own = WorkspaceMember {
        account_id: scene.account_id,
        name: "Luca".to_owned(),
        role_ids: vec![scene.helpers],
    };
    model.set_members(scene.workspace_id, Loadable::Loading);
    model.apply(&Event::WorkspaceMemberAdded {
        workspace_id: scene.workspace_id,
        member: bruno.clone(),
    });
    assert_eq!(
        model.workspace(scene.workspace_id).unwrap().members,
        Loadable::Loading
    );
    model.set_members(
        scene.workspace_id,
        Loadable::Loaded(vec![ana.clone(), own.clone()]),
    );
    let members = |model: &ServerModel| {
        let mut names: Vec<String> = model
            .workspace(scene.workspace_id)
            .unwrap()
            .members
            .loaded()
            .unwrap()
            .iter()
            .map(|member| member.name.clone())
            .collect();
        names.sort();
        names
    };

    model.apply(&Event::WorkspaceMemberAdded {
        workspace_id: scene.workspace_id,
        member: bruno.clone(),
    });
    model.apply(&Event::WorkspaceMemberAdded {
        workspace_id: scene.workspace_id,
        member: bruno.clone(),
    });
    assert_eq!(members(&model), ["Ana", "Bruno", "Luca"]);
    model.apply(&Event::AccountProfileUpdated {
        profile: AccountProfile {
            id: ana.account_id,
            name: "Ana Maria".to_owned(),
            avatar: None,
        },
    });
    model.apply(&Event::AccountProfileUpdated {
        profile: AccountProfile {
            id: scene.account_id,
            name: "Luca B.".to_owned(),
            avatar: None,
        },
    });
    assert_eq!(members(&model), ["Ana Maria", "Bruno", "Luca B."]);
    assert_eq!(model.account.name, "Luca B.");
    model.apply(&Event::WorkspaceMemberRolesChanged {
        workspace_id: scene.workspace_id,
        account_id: scene.account_id,
        role_ids: Vec::new(),
    });
    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert!(workspace.own_role_ids.is_empty());
    assert_eq!(
        workspace.role_permissions(),
        [WorkspacePermission::ViewChannel]
    );
    let held = workspace.members.loaded().unwrap();
    assert!(
        held.iter()
            .all(|member| member.account_id != scene.account_id || member.role_ids.is_empty())
    );
    model.apply(&Event::WorkspaceMemberRemoved {
        workspace_id: scene.workspace_id,
        account_id: bruno.account_id,
    });
    assert_eq!(members(&model), ["Ana Maria", "Luca B."]);
    let _ = own;
}

#[test]
fn the_events_that_arrived_during_a_read_are_applied_to_its_part_after_the_answer() {
    let scene = scene();
    let mut model = scene.model();
    let (ana, bruno, carla) = (member("Ana"), member("Bruno"), member("Carla"));
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let speaker = participant(AccountId::generate());

    // While the members are read: Bruno is added and Carla, whom the answer still lists,
    // is removed. Other events of the same time are in the state already.
    model.set_members(scene.workspace_id, Loadable::Loading);
    let during = [
        Event::WorkspaceMemberAdded {
            workspace_id: scene.workspace_id,
            member: bruno.clone(),
        },
        started(call_id, place),
        joined(call_id, place, &speaker),
        Event::WorkspaceMemberRemoved {
            workspace_id: scene.workspace_id,
            account_id: carla.account_id,
        },
        Event::WorkspaceMemberAdded {
            workspace_id: WorkspaceId::generate(),
            member: member("Elsewhere"),
        },
    ];
    for event in &during {
        model.apply(event);
    }
    let before_answer = model.clone();
    model.set_members(
        scene.workspace_id,
        Loadable::Loaded(vec![ana.clone(), carla.clone()]),
    );
    model.replay(Part::Members(scene.workspace_id), &during);
    let mut held = model
        .workspace(scene.workspace_id)
        .unwrap()
        .members
        .loaded()
        .unwrap()
        .clone();
    held.sort_by(|one, other| one.name.cmp(&other.name));
    assert_eq!(held, [ana, bruno]);
    // The replay changed the members and nothing else.
    let mut without_members = model.clone();
    without_members.set_members(scene.workspace_id, Loadable::Loading);
    assert_eq!(without_members, before_answer);

    // While the call of the channel is read: the answer is older than the leave.
    let answer = ChannelCall {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
        in_use: true,
        call: Some(Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            participants: vec![speaker.clone()],
        }),
    };
    let late = participant(AccountId::generate());
    let during = [
        Event::CallParticipantLeft {
            call_id,
            place,
            account_id: speaker.account_id,
            reason: LeaveReason::Left,
        },
        joined(call_id, place, &late),
        joined(
            CallId::generate(),
            scene.place(scene.hidden),
            &participant(AccountId::generate()),
        ),
    ];
    model.set_channel_call(answer);
    model.replay(Part::ChannelCall(scene.daily), &during);
    assert_eq!(model.call_at(&place).unwrap().participants, [late]);
    assert!(model.channel(scene.hidden).unwrap().call.is_none());
}

#[test]
fn direct_calls_and_muted_conversations_follow_their_events() {
    let scene = scene();
    let mut model = scene.model();
    let other = AccountId::generate();
    let (call_id, place) = (
        CallId::generate(),
        CallPlace::direct(scene.account_id, other),
    );
    let ring = CallInvitation {
        id: CallInvitationId::generate(),
        call_id,
        place,
        account_id: scene.account_id,
        invited_by: other,
        created_at: time(),
        expires_at: time(),
    };
    model.apply(&Event::CallInvited {
        invitation: ring.clone(),
    });
    assert_eq!(model.incoming_invitations().count(), 1);
    model.apply(&started(call_id, place));
    model.apply(&joined(call_id, place, &participant(other)));
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.started_at, Some(time()));
    assert_eq!(call.participants.len(), 1);
    model.apply(&Event::CallEnded { call_id, place });
    assert!(model.direct_calls.is_empty() && model.invitations.is_empty());

    // The account calls, nobody answers, and it leaves before the call started.
    let own = OwnCall {
        call_id,
        place,
        device_id: scene.device_id,
        identity: "own-seat".to_owned(),
    };
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    model.set_own_call(Some(OwnCallState {
        own,
        call: Call {
            id: call_id,
            place,
            started_at: None,
            lock: None,
            meeting: None,
            participants: Vec::new(),
        },
        entry_requests: Vec::new(),
    }));
    assert_eq!(model.direct_calls.len(), 1);
    model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::Left),
    });
    assert!(model.direct_calls.is_empty());

    let mute = DirectMute {
        account_id: other,
        until: None,
    };
    model.apply(&Event::DirectMuteSet { mute: mute.clone() });
    let until = DirectMute {
        until: Some(time()),
        ..mute
    };
    model.apply(&Event::DirectMuteSet {
        mute: until.clone(),
    });
    assert_eq!(model.direct_mutes, [until]);
    model.apply(&Event::DirectMuteRemoved { account_id: other });
    assert!(model.direct_mutes.is_empty());
}

#[test]
fn the_answer_for_the_call_of_a_channel_shows_nobody_when_the_channel_hides_again() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.hidden));
    let answer = ChannelCall {
        workspace_id: scene.workspace_id,
        channel_id: scene.hidden,
        in_use: true,
        call: Some(Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            participants: vec![participant(AccountId::generate())],
        }),
    };
    // The server made the answer while the channel showed who is inside. Before the
    // answer arrived, the channel hid them again.
    model.apply(&started(call_id, place));
    model.set_channel_call(answer.clone());
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use && hidden.call.is_none());

    // The account inside is shown the people.
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(call_id, scene.hidden)),
        left: None,
    });
    model.set_channel_call(answer);
    assert_eq!(model.call_at(&place).unwrap().participants.len(), 1);
}

#[test]
fn the_people_of_a_call_are_in_the_order_of_the_server() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let mut people: Vec<CallParticipant> =
        (0..3).map(|_| participant(AccountId::generate())).collect();
    people[0].joined_at = time() + time::Duration::seconds(5);
    model.apply(&started(call_id, place));
    // The events of one admission check arrive in another order than a snapshot lists
    // the people: by the time of the join, then by the account.
    for person in [&people[0], &people[2], &people[1]] {
        model.apply(&joined(call_id, place, person));
    }
    let order: Vec<AccountId> = model
        .call_at(&place)
        .unwrap()
        .participants
        .iter()
        .map(|person| person.account_id)
        .collect();
    assert_eq!(
        order,
        [
            people[1].account_id,
            people[2].account_id,
            people[0].account_id
        ]
    );
}

#[test]
fn the_events_of_a_read_do_not_show_the_people_of_a_channel_that_hides_again() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.hidden));
    let person = participant(AccountId::generate());
    let mut shown = model.channel(scene.hidden).unwrap().channel.clone();
    shown.hides_occupants = false;
    let mut hidden = shown.clone();
    hidden.hides_occupants = true;

    // The channel stops hiding and its call is read. During the read a person joins, and
    // then the channel hides again.
    model.apply(&started(call_id, place));
    let follow = model.apply(&Event::ChannelUpdated { channel: shown });
    assert_eq!(follow.load, [Part::ChannelCall(scene.hidden)]);
    let during = [
        joined(call_id, place, &person),
        Event::ChannelUpdated { channel: hidden },
    ];
    for event in &during {
        model.apply(event);
    }
    assert!(model.channel(scene.hidden).unwrap().call.is_none());
    model.set_channel_call(ChannelCall {
        workspace_id: scene.workspace_id,
        channel_id: scene.hidden,
        in_use: true,
        call: Some(Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            participants: vec![person],
        }),
    });
    model.replay(Part::ChannelCall(scene.hidden), &during);
    let held = model.channel(scene.hidden).unwrap();
    assert!(held.in_use && held.call.is_none());
}
