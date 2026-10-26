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
    EntryRequestId, RecordingId, StreamEpoch, WorkspaceId, WorkspaceRoleId,
};
use junto_protocol::recording::{Recording, RecordingState, RecordingStop, RecordingStopReason};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::snapshot::{Snapshot, WorkspaceState};
use junto_protocol::structure::{Category, Channel, ChannelKind, Workspace, WorkspaceMember};
use junto_protocol::transcript::TranscriptState;
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
    let role = |name: &str, permissions: &[WorkspacePermission], position| WorkspaceRole {
        position,
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: name.to_owned(),
        permissions: permissions.to_vec(),
        is_everyone: position == 0,
    };
    let everyone = role("everyone", &[WorkspacePermission::ViewChannel], 0);
    let helpers = role("helpers", &[WorkspacePermission::MuteMembers], 1);
    let unheld = role("leads", &[WorkspacePermission::ManageChannel], 2);
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
            permissions: vec![
                WorkspacePermission::ViewChannel,
                WorkspacePermission::MuteMembers,
            ],
            channel_permissions: Vec::new(),
            notification_default: None,
        }],
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        sent_call_invitations: Vec::new(),
        call_moves: Vec::new(),
        entry_requests: Vec::new(),
        direct_mutes: Vec::new(),
        messages: Vec::new(),
        threads: Vec::new(),
        threads_more: false,
        threads_next: None,
        notifications: Vec::new(),
        notification_settings: Vec::new(),
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
            hides_occupants: None,
            call_id,
            place: self.place(channel_id),
            device_id: self.device_id,
            identity: "own-seat".to_owned(),
        }
    }
}

pub(crate) fn participant(account_id: AccountId) -> CallParticipant {
    CallParticipant {
        avatar: None,
        account_id,
        identity: format!("seat-{account_id}"),
        joined_at: time(),
        muted: false,
        deafened: false,
        microphone_block: None,
        sound_block: None,
        hand_raised: false,
        screen_share: None,
        screen_shares: Vec::new(),
    }
}

pub(crate) fn member(name: &str) -> WorkspaceMember {
    WorkspaceMember {
        avatar: None,
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
        hides_occupants: None,
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
                hides_occupants: false,
                participants: vec![participant(other)],
                recording: None,
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
        workspace.permissions,
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
        position: 1,
        id: scene.helpers,
        workspace_id: scene.workspace_id,
        name: "helpers".to_owned(),
        permissions: vec![WorkspacePermission::MoveMembers],
        is_everyone: false,
    };
    model.apply(&Event::WorkspaceRoleUpdated { role: role.clone() });
    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert_eq!(workspace.roles.len(), 3);
    assert_eq!(workspace.roles[0].id, scene.everyone);
    assert!(workspace.roles.contains(&role));
    // What the account holds is what the server stated (ADR 0310): the event of a role
    // changes the role, and the snapshot after `permissions_changed` changes the list.
    assert_eq!(
        workspace.permissions,
        [
            WorkspacePermission::ViewChannel,
            WorkspacePermission::MuteMembers
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
        avatar: None,
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
        avatar: None,
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
fn a_call_keeps_the_hide_setting_of_its_start_and_a_change_applies_to_the_next_call() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let first = participant(AccountId::generate());
    model.apply(&started(call_id, place));
    model.apply(&joined(call_id, place, &first));
    let mut daily = model.channel(scene.daily).unwrap().channel.clone();

    // The channel hides who is inside from now on. The call that runs does not: the
    // server still tells who is in it (ADR 0517).
    daily.hides_occupants = true;
    let follow = model.apply(&Event::ChannelUpdated {
        channel: daily.clone(),
    });
    assert_eq!(follow, Follow::default());
    let second = participant(AccountId::generate());
    model.apply(&joined(call_id, place, &second));
    let held = model.channel(scene.daily).unwrap();
    assert!(held.channel.hides_occupants && held.in_use);
    let call = held.call.as_ref().unwrap();
    assert!(!call.hides_occupants);
    assert_eq!(call.participants.len(), 2);

    // The account enters and leaves that call: it keeps what an account outside is told.
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(call_id, scene.daily)),
        left: None,
    });
    model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::Left),
    });
    assert_eq!(model.call_at(&place).unwrap().participants.len(), 2);

    // The next call of the channel hides who is inside.
    model.apply(&Event::CallEnded { call_id, place });
    let next = CallId::generate();
    model.apply(&started(next, place));
    model.apply(&joined(next, place, &first));
    let held = model.channel(scene.daily).unwrap();
    assert!(held.in_use && held.call.is_none());

    // The setting is turned off during that call: nothing of the call is shown or asked
    // for, and the call after it is shown.
    daily.hides_occupants = false;
    let follow = model.apply(&Event::ChannelUpdated { channel: daily });
    assert_eq!(follow, Follow::default());
    let held = model.channel(scene.daily).unwrap();
    assert!(held.in_use && held.call.is_none());
    model.apply(&Event::CallEnded {
        call_id: next,
        place,
    });
    let last = CallId::generate();
    model.apply(&started(last, place));
    model.apply(&joined(last, place, &first));
    let call = model.call_at(&place).unwrap();
    assert!(!call.hides_occupants);
    assert_eq!(call.participants.len(), 1);
}

/// ADR 0517: the server gives an account with a place nothing of a call that hides who is
/// inside, and no request to enter of any call, until it has seen the device of the
/// account in the room. The event of the own entry is the first one that the account
/// receives, and the account reads its call then.
#[test]
fn the_account_reads_its_call_when_the_server_has_seen_its_device_in_the_room() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.hidden));
    let own = scene.own_call(call_id, scene.hidden);
    let follow = model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    assert_eq!(follow.load, [Part::OwnCall]);
    let answer = |participants: Vec<CallParticipant>| OwnCallState {
        inside: None,
        meeting_grant: None,
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: true,
            participants,
            recording: None,
        },
        entry_requests: Vec::new(),
    };
    model.set_own_call(Some(answer(Vec::new())), None);
    let call = model.call_at(&place).unwrap();
    assert!(call.hides_occupants && call.participants.is_empty());
    assert!(model.channel(scene.hidden).unwrap().in_use);

    let me = participant(scene.account_id);
    let other = participant(AccountId::generate());
    let follow = model.apply(&joined(call_id, place, &me));
    assert_eq!(follow.load, [Part::OwnCall]);
    assert!(!follow.reload);
    // The entry of another person and a change of the own state ask for nothing.
    assert_eq!(
        model.apply(&joined(call_id, place, &other)),
        Follow::default()
    );
    let updated = Event::CallParticipantUpdated {
        call_id,
        place,
        participant: CallParticipant {
            avatar: None,
            muted: true,
            ..me.clone()
        },
    };
    assert_eq!(model.apply(&updated), Follow::default());
    model.set_own_call(Some(answer(vec![me.clone(), other])), None);
    assert_eq!(model.call_at(&place).unwrap().participants.len(), 2);

    // The account leaves: it keeps nothing of the call, also after the channel stopped
    // hiding who is inside, because the call keeps the setting of its start.
    let mut shown = model.channel(scene.hidden).unwrap().channel.clone();
    shown.hides_occupants = false;
    model.apply(&Event::ChannelUpdated { channel: shown });
    model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::Left),
    });
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use && hidden.call.is_none());

    // The same event in a call that shows who is inside: the requests to enter are given
    // to an account inside only.
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(call_id, scene.daily)),
        left: None,
    });
    assert_eq!(
        model.apply(&joined(call_id, place, &me)).load,
        [Part::OwnCall]
    );
    // An event that names the account in a call that is not its own asks for nothing.
    let elsewhere = joined(CallId::generate(), scene.place(scene.daily), &me);
    assert_eq!(model.apply(&elsewhere), Follow::default());
}

/// ADR 0517: the answer of the own call says whether the admission check has seen the seat.
/// An answer that does not say it states nothing; only an explicit "not inside" tells a
/// reader that the person is entering. The statement holds at the position of its answer:
/// a later event that lists the account, or a change of the own call, ends it.
#[test]
fn what_the_server_says_of_the_own_seat_is_held_until_an_event_lists_the_account() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let own = scene.own_call(call_id, scene.daily);
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    let answer = |inside: Option<bool>| OwnCallState {
        meeting_grant: None,
        inside,
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: Vec::new(),
            recording: None,
        },
        entry_requests: Vec::new(),
    };

    // A server that does not state it: not stated is not "outside".
    model.set_own_call(Some(answer(None)), Some(7));
    assert_eq!(model.own_seat, None);
    assert!(!model.seat_stated_outside(&own.identity));

    model.set_own_call(Some(answer(Some(false))), Some(7));
    assert!(model.seat_stated_outside(&own.identity));
    // The statement names one seat.
    assert!(!model.seat_stated_outside("another-seat"));

    // The entry of another person changes nothing of it.
    model.apply(&joined(call_id, place, &participant(AccountId::generate())));
    assert!(model.seat_stated_outside(&own.identity));

    // The server has seen the account in the room.
    let mut me = participant(scene.account_id);
    me.identity.clone_from(&own.identity);
    model.apply(&joined(call_id, place, &me));
    assert_eq!(model.own_seat, None);
    assert!(!model.seat_stated_outside(&own.identity));

    model.set_own_call(Some(answer(Some(true))), Some(7));
    assert_eq!(
        model.own_seat,
        Some(SeatStated {
            identity: own.identity.clone(),
            inside: true
        })
    );
    assert!(!model.seat_stated_outside(&own.identity));

    // A change of the state of the person in the call is an event of a person inside.
    model.set_own_call(Some(answer(Some(false))), Some(7));
    model.apply(&Event::CallParticipantUpdated {
        call_id,
        place,
        participant: me,
    });
    assert_eq!(model.own_seat, None);

    // Another own call: the statement was about the call of before.
    model.set_own_call(Some(answer(Some(false))), Some(7));
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(CallId::generate(), scene.daily)),
        left: None,
    });
    assert_eq!(model.own_seat, None);
}

/// ADR 0521: a read of the own call states a grant of meeting mode that the person has
/// not answered, for one seat; a change of the own call ends the statement.
#[test]
fn a_read_of_the_own_call_states_a_grant_of_its_seat() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let own = scene.own_call(call_id, scene.daily);
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    let answer = |meeting_grant: Option<bool>| OwnCallState {
        meeting_grant,
        inside: Some(true),
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: Vec::new(),
            recording: None,
        },
        entry_requests: Vec::new(),
    };
    model.set_own_call(Some(answer(None)), Some(7));
    assert_eq!(model.grant_stated(&own.identity), None);
    model.set_own_call(Some(answer(Some(true))), Some(7));
    assert_eq!(model.grant_stated(&own.identity), Some(7));
    assert_eq!(model.grant_stated("another-seat"), None);
    model.set_own_call(Some(answer(Some(false))), Some(8));
    assert_eq!(model.grant_stated(&own.identity), None);
    // An answer without a position states the grant at the position of the state.
    model.set_own_call(Some(answer(Some(true))), None);
    assert_eq!(
        model.grant_stated(&own.identity),
        Some(model.cursor.position)
    );
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(CallId::generate(), scene.daily)),
        left: None,
    });
    assert_eq!(model.grant_stated(&own.identity), None);
}

/// ADR 0517: the event of the start of a call and the own call of the account state the
/// setting that the call keeps. The state uses them before the setting of the channel,
/// which can have changed since the call was made.
#[test]
fn a_call_takes_the_hide_setting_that_its_start_or_the_own_call_states() {
    let scene = scene();
    let start = |hides_occupants, call_id, place| Event::CallStarted {
        hides_occupants,
        call_id,
        place,
        started_at: time(),
    };
    let person = participant(AccountId::generate());

    // The channel shows who is inside, and the call that starts hides it: the account
    // outside holds nothing of the call.
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&start(Some(true), call_id, place));
    let held = model.channel(scene.daily).unwrap();
    assert!(held.in_use && held.call.is_none());

    // The channel hides who is inside, and the call that starts does not.
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.hidden));
    model.apply(&start(Some(false), call_id, place));
    model.apply(&joined(call_id, place, &person));
    let call = model.call_at(&place).unwrap();
    assert!(!call.hides_occupants && call.started_at.is_some());
    assert_eq!(call.participants.len(), 1);

    // A start that states a setting corrects the call that an earlier event made with the
    // setting of the channel.
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&joined(call_id, place, &person));
    assert!(model.call_at(&place).is_some());
    model.apply(&start(Some(true), call_id, place));
    let held = model.channel(scene.daily).unwrap();
    assert!(held.in_use && held.call.is_none());

    // A server that states no setting: the setting of the channel, as before.
    let mut model = scene.model();
    model.apply(&start(None, call_id, place));
    assert!(!model.call_at(&place).unwrap().hides_occupants);
    let hidden = scene.place(scene.hidden);
    model.apply(&start(None, CallId::generate(), hidden));
    assert!(model.call_at(&hidden).is_none());

    // The own call states the setting of a call that the state does not hold.
    for (channel_id, stated) in [(scene.daily, true), (scene.hidden, false)] {
        let mut model = scene.model();
        let (call_id, place) = (CallId::generate(), scene.place(channel_id));
        let mut own = scene.own_call(call_id, channel_id);
        own.hides_occupants = Some(stated);
        model.apply(&Event::OwnCallChanged {
            call: Some(own),
            left: None,
        });
        model.apply(&joined(call_id, place, &person));
        let call = model.call_at(&place).unwrap();
        assert_eq!(call.hides_occupants, stated);
        assert_eq!(call.participants.len(), 1);
    }
}

/// The events of a read that was on its way when the account left a call that hides who is
/// inside, and the answer of that read, show nothing of the call.
#[test]
fn a_read_of_a_hidden_call_that_the_account_left_shows_nothing_of_it() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.hidden));
    let own = scene.own_call(call_id, scene.hidden);
    let person = participant(AccountId::generate());
    model.apply(&started(call_id, place));
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    let during = [
        joined(call_id, place, &person),
        Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::Left),
        },
    ];
    for event in &during {
        model.apply(event);
    }
    model.set_own_call(
        Some(OwnCallState {
            inside: None,
            meeting_grant: None,
            own,
            call: Call {
                id: call_id,
                place,
                started_at: Some(time()),
                lock: None,
                meeting: None,
                hides_occupants: true,
                participants: vec![person],
                recording: None,
            },
            entry_requests: Vec::new(),
        }),
        None,
    );
    model.replay(Part::OwnCall, &during);
    let held = model.channel(scene.hidden).unwrap();
    assert!(held.in_use && held.call.is_none());
}

/// The snapshot carries what waits for the account and what the account made: the moves,
/// the invitations of both directions and its own requests to enter (ADR 0207).
#[test]
fn a_snapshot_gives_the_moves_the_sent_invitations_and_the_own_requests_to_enter() {
    let mut scene = scene();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let other = AccountId::generate();
    let invitation = |account_id, invited_by| CallInvitation {
        id: CallInvitationId::generate(),
        call_id,
        place,
        account_id,
        invited_by,
        created_at: time(),
        expires_at: time(),
    };
    let (got, made) = (
        invitation(scene.account_id, other),
        invitation(other, scene.account_id),
    );
    let call_move = CallMove {
        id: CallMoveId::generate(),
        account_id: other,
        from: place,
        to: scene.place(scene.hidden),
        moved_by: scene.account_id,
        expires_at: time(),
    };
    let request = EntryRequest {
        id: EntryRequestId::generate(),
        call_id,
        place,
        account_id: scene.account_id,
        created_at: time(),
        expires_at: time(),
    };
    scene.snapshot.call_invitations = vec![got.clone()];
    scene.snapshot.sent_call_invitations = vec![made.clone()];
    scene.snapshot.call_moves = vec![call_move.clone()];
    scene.snapshot.entry_requests = vec![request.clone()];
    let mut model = scene.model();
    assert_eq!(model.invitations, [got.clone(), made]);
    assert_eq!(model.incoming_invitations().collect::<Vec<_>>(), [&got]);
    assert_eq!(model.moves, [call_move]);
    assert_eq!(model.own_entry_requests, std::slice::from_ref(&request));
    assert!(model.entry_requests.is_empty());

    // The answer ends the request on every device of the account.
    model.apply(&Event::CallEntryAnswered {
        request_id: request.id,
        outcome: EntryOutcome::Denied,
    });
    assert!(model.own_entry_requests.is_empty());

    // The event of a request reaches every device of the account that asked. The request
    // is not one of people outside the call of the account.
    let asked = Event::CallEntryRequested {
        request: request.clone(),
    };
    model.apply(&asked);
    model.apply(&asked);
    assert_eq!(model.own_entry_requests, std::slice::from_ref(&request));
    assert!(model.entry_requests.is_empty());
    model.apply(&Event::CallEnded { call_id, place });
    assert!(model.own_entry_requests.is_empty());
    model.apply(&asked);
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    });
    assert!(model.own_entry_requests.is_empty());
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
    model.set_own_call(
        Some(OwnCallState {
            inside: None,
            meeting_grant: None,
            own: own.clone(),
            call: Call {
                id: call_id,
                place,
                started_at: None,
                lock: None,
                meeting: None,
                hides_occupants: false,
                participants: Vec::new(),
                recording: None,
            },
            entry_requests: Vec::new(),
        }),
        None,
    );
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
        inside: None,
        meeting_grant: None,
        own: OwnCall {
            hides_occupants: None,
            call_id,
            ..own.clone()
        },
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: vec![participant(scene.account_id)],
            recording: None,
        },
        entry_requests: vec![request.clone()],
    };
    // The answer of a read for a call that the account has left is not used.
    let before = model.clone();
    model.set_own_call(Some(loaded(CallId::generate())), None);
    model.set_own_call(None, None);
    assert_eq!(model, before);

    model.set_own_call(Some(loaded(call_id)), None);
    assert_eq!(model.entry_requests, std::slice::from_ref(&request));
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.participants.len(), 1);
    assert_eq!(call.speaking, [scene.account_id]);
    assert!(model.channel(scene.daily).unwrap().in_use);

    // The call moves to another device of the account: the requests stay.
    let follow = model.apply(&Event::OwnCallChanged {
        call: Some(OwnCall {
            hides_occupants: None,
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
        avatar: None,
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
            departure: None,
        },
    });
    model.apply(&Event::AccountProfileUpdated {
        profile: AccountProfile {
            id: scene.account_id,
            name: "Luca B.".to_owned(),
            avatar: None,
            departure: None,
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

    // While the call of the account is read: the answer is older than the leave.
    let own = scene.own_call(call_id, scene.daily);
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    let answer = OwnCallState {
        inside: None,
        meeting_grant: None,
        own,
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: vec![speaker.clone()],
            recording: None,
        },
        entry_requests: Vec::new(),
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
    model.set_own_call(Some(answer), None);
    model.replay(Part::OwnCall, &during);
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
        hides_occupants: None,
        call_id,
        place,
        device_id: scene.device_id,
        identity: "own-seat".to_owned(),
    };
    model.apply(&Event::OwnCallChanged {
        call: Some(own.clone()),
        left: None,
    });
    model.set_own_call(
        Some(OwnCallState {
            inside: None,
            meeting_grant: None,
            own,
            call: Call {
                id: call_id,
                place,
                started_at: None,
                lock: None,
                meeting: None,
                hides_occupants: false,
                participants: Vec::new(),
                recording: None,
            },
            entry_requests: Vec::new(),
        }),
        None,
    );
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

fn recording(scene: &Scene, channel_id: ChannelId, call_id: CallId) -> Recording {
    Recording {
        id: RecordingId::generate(),
        workspace_id: scene.workspace_id,
        channel_id,
        call_id,
        state: RecordingState::Starting,
        started_by: AccountId::generate(),
        requested_at: time(),
        recording_since: None,
        stop: None,
        ended_at: None,
        failure: None,
        language: "auto".to_owned(),
        segments: 0,
        failed_segments: 0,
        bytes: None,
        timeline_started_at: None,
        transcript: TranscriptState::None,
        transcript_lines: 0,
    }
}

fn updated(recording: &Recording) -> Event {
    Event::RecordingUpdated {
        recording: Box::new(recording.clone()),
    }
}

#[test]
fn a_recording_is_in_the_state_of_its_call_while_it_is_starting_or_runs() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&started(call_id, place));
    model.apply(&joined(call_id, place, &participant(AccountId::generate())));
    assert_eq!(model.call_at(&place).unwrap().recording, None);

    let mut record = recording(&scene, scene.daily, call_id);
    let asked = Event::RecordingStarted {
        recording: Box::new(record.clone()),
    };
    assert_eq!(model.apply(&asked), Follow::default());
    assert_eq!(
        model.call_at(&place).unwrap().recording,
        Some(record.clone())
    );
    let once = model.clone();
    model.apply(&asked);
    assert_eq!(model, once);

    record.state = RecordingState::Recording;
    record.recording_since = Some(time());
    record.segments = 2;
    model.apply(&updated(&record));
    assert_eq!(
        model.call_at(&place).unwrap().recording,
        Some(record.clone())
    );

    // A stop that was asked for leaves the state `recording` until every job has ended.
    record.stop = Some(RecordingStop {
        reason: RecordingStopReason::Stopped,
        by: Some(scene.account_id),
        at: time(),
    });
    model.apply(&updated(&record));
    assert_eq!(
        model.call_at(&place).unwrap().recording,
        Some(record.clone())
    );

    // A recording of the call that ended earlier does not remove the one that runs.
    let mut earlier = recording(&scene, scene.daily, call_id);
    earlier.state = RecordingState::Ready;
    model.apply(&updated(&earlier));
    assert_eq!(
        model.call_at(&place).unwrap().recording,
        Some(record.clone())
    );

    for state in [
        RecordingState::Processing,
        RecordingState::Ready,
        RecordingState::PartlyFailed,
        RecordingState::Failed,
    ] {
        let mut with_recording = model.clone();
        record.state = state;
        with_recording.apply(&updated(&record));
        let call = with_recording.call_at(&place).unwrap();
        assert_eq!(call.recording, None, "{state:?}");
        assert_eq!(call.participants.len(), 1);
        let once = with_recording.clone();
        with_recording.apply(&updated(&record));
        assert_eq!(with_recording, once);
    }
}

#[test]
fn a_recording_changes_only_a_call_that_the_state_holds() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));

    // No call is held at the place: the event creates none.
    let before = model.clone();
    model.apply(&updated(&recording(&scene, scene.daily, call_id)));
    assert_eq!(model, before);

    // A replay can deliver the recording of an earlier call of the channel.
    model.apply(&started(call_id, place));
    let before = model.clone();
    model.apply(&updated(&recording(
        &scene,
        scene.daily,
        CallId::generate(),
    )));
    assert_eq!(model, before);
    assert_eq!(model.call_at(&place).unwrap().id, call_id);

    // A workspace that the state does not hold.
    let mut elsewhere = recording(&scene, scene.daily, call_id);
    elsewhere.workspace_id = WorkspaceId::generate();
    model.apply(&updated(&elsewhere));
    assert_eq!(model, before);

    // An account outside a channel that hides who is inside holds no details of its call.
    let (hidden_call, hidden_place) = (CallId::generate(), scene.place(scene.hidden));
    model.apply(&started(hidden_call, hidden_place));
    let record = recording(&scene, scene.hidden, hidden_call);
    model.apply(&updated(&record));
    let hidden = model.channel(scene.hidden).unwrap();
    assert!(hidden.in_use && hidden.call.is_none());

    // The account inside is shown the recording, and loses it with the call when it leaves.
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(hidden_call, scene.hidden)),
        left: None,
    });
    model.apply(&joined(
        hidden_call,
        hidden_place,
        &participant(scene.account_id),
    ));
    model.apply(&updated(&record));
    assert_eq!(
        model.call_at(&hidden_place).unwrap().recording,
        Some(record)
    );
    model.apply(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::Left),
    });
    assert!(model.channel(scene.hidden).unwrap().call.is_none());
}

#[test]
fn the_views_of_a_call_carry_its_recording_and_a_read_keeps_the_events_of_it() {
    let mut scene = scene();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    let record = recording(&scene, scene.daily, call_id);
    let call = |recording: Option<Recording>| Call {
        id: call_id,
        place,
        started_at: Some(time()),
        lock: None,
        meeting: None,
        hides_occupants: false,
        participants: vec![participant(scene.account_id)],
        recording,
    };
    let shown = |recording: Option<Recording>| ChannelCall {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
        in_use: true,
        call: Some(call(recording)),
    };

    // The snapshot.
    scene.snapshot.workspaces[0].calls = vec![shown(Some(record.clone()))];
    let mut model = scene.model();
    assert_eq!(
        model.call_at(&place).unwrap().recording,
        Some(record.clone())
    );

    // The view of a call names the recording that is starting or runs. The state keeps no
    // other one, whatever a server sends.
    let mut ended = record.clone();
    ended.state = RecordingState::Processing;
    let mut later = scene.snapshot.clone();
    later.workspaces[0].calls = vec![shown(Some(ended.clone()))];
    let after = ServerModel::from_snapshot(later);
    assert_eq!(after.call_at(&place).unwrap().recording, None);

    // The call of the account.
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(call_id, scene.daily)),
        left: None,
    });
    model.set_own_call(
        Some(OwnCallState {
            inside: None,
            meeting_grant: None,
            own: scene.own_call(call_id, scene.daily),
            call: call(Some(record.clone())),
            entry_requests: Vec::new(),
        }),
        None,
    );
    assert_eq!(
        model.call_at(&place).unwrap().recording,
        Some(record.clone())
    );

    // While the call of the account is read: the answer is older than the end of the
    // recording, and the event that was kept is applied after it.
    let during = [updated(&ended)];
    assert!(Part::OwnCall.concerns(&during[0], &model));
    assert!(!Part::Members(scene.workspace_id).concerns(&during[0], &model));
    model.set_own_call(
        Some(OwnCallState {
            inside: None,
            meeting_grant: None,
            own: scene.own_call(call_id, scene.daily),
            call: call(Some(record)),
            entry_requests: Vec::new(),
        }),
        None,
    );
    model.replay(Part::OwnCall, &during);
    assert_eq!(model.call_at(&place).unwrap().recording, None);
}

#[test]
fn a_removed_recording_leaves_the_call_that_shows_it() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&started(call_id, place));
    let record = recording(&scene, scene.daily, call_id);
    model.apply(&updated(&record));
    let removed = |recording_id: RecordingId, channel_id: ChannelId| Event::RecordingRemoved {
        recording_id,
        workspace_id: scene.workspace_id,
        channel_id,
    };

    // Another recording of the channel, and the same recording in another channel.
    let before = model.clone();
    model.apply(&removed(RecordingId::generate(), scene.daily));
    model.apply(&removed(record.id, scene.hidden));
    assert_eq!(model, before);

    let event = removed(record.id, scene.daily);
    assert!(!Part::OwnCall.concerns(&event, &model));
    assert_eq!(model.apply(&event), Follow::default());
    let call = model.call_at(&place).unwrap();
    assert_eq!((call.id, &call.recording), (call_id, &None));
    let once = model.clone();
    model.apply(&event);
    assert_eq!(model, once);

    // The account in the call keeps the event while its call is read.
    model.apply(&Event::OwnCallChanged {
        call: Some(scene.own_call(call_id, scene.daily)),
        left: None,
    });
    assert!(Part::OwnCall.concerns(&event, &model));
}

#[test]
fn the_events_of_transcripts_and_of_the_glossary_change_nothing() {
    let scene = scene();
    let mut model = scene.model();
    let (call_id, place) = (CallId::generate(), scene.place(scene.daily));
    model.apply(&started(call_id, place));
    let record = recording(&scene, scene.daily, call_id);
    model.apply(&updated(&record));
    let before = model.clone();
    let events = [
        Event::TranscriptLineCorrected {
            recording_id: record.id,
            workspace_id: scene.workspace_id,
            channel_id: scene.daily,
            line: 3,
        },
        Event::TranscriptSpeakerRenamed {
            recording_id: record.id,
            workspace_id: scene.workspace_id,
            channel_id: scene.daily,
            speaker: 1,
        },
        Event::GlossaryReplaced {
            workspace_id: scene.workspace_id,
        },
    ];
    for event in &events {
        assert_eq!(model.apply(event), Follow::default(), "{}", event.name());
        assert!(!Part::OwnCall.concerns(event, &model));
        assert!(!Part::Members(scene.workspace_id).concerns(event, &model));
    }
    assert_eq!(model, before);
}
