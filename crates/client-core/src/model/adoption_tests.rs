//! Role order, the permissions that the server states, and the events of the client
//! requests (ADR 0309, 0310, 0412).

use junto_protocol::account::{AccountStatus, AccountSummary};
use junto_protocol::discovery::{Notification, NotificationKind, NotificationLevel};
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, MessageId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::message::ThreadState;
use junto_protocol::roles::{ServerRole, ServerRolePosition, WorkspaceRole, WorkspaceRolePosition};
use junto_protocol::structure::{ArchivedChannel, ChannelPermissions, Workspace};
use junto_protocol::{ServerPermission, WorkspacePermission};

use super::tests::{Scene, scene, time};
use super::*;

fn role_names(model: &ServerModel, workspace_id: WorkspaceId) -> Vec<String> {
    model
        .workspace(workspace_id)
        .unwrap()
        .roles
        .iter()
        .map(|role| format!("{}:{}", role.name, role.position))
        .collect()
}

fn role_id(model: &ServerModel, workspace_id: WorkspaceId, name: &str) -> WorkspaceRoleId {
    model
        .workspace(workspace_id)
        .unwrap()
        .roles
        .iter()
        .find(|role| role.name == name)
        .unwrap()
        .id
}

fn server_role(name: &str, position: i32, permissions: &[ServerPermission]) -> ServerRole {
    ServerRole {
        position,
        id: ServerRoleId::generate(),
        name: name.to_owned(),
        permissions: permissions.to_vec(),
    }
}

fn load_server_roles(model: &mut ServerModel, roles: Vec<ServerRole>) {
    model.set_admin(AdminPart::ServerRoles, AdminSet::Loading);
    model.set_admin(
        AdminPart::ServerRoles,
        AdminSet::Loaded(AdminPayload::ServerRoles(roles)),
    );
}

fn server_role_names(model: &ServerModel) -> Vec<String> {
    model
        .admin
        .server_roles
        .loaded()
        .unwrap()
        .iter()
        .map(|role| format!("{}:{}", role.name, role.position))
        .collect()
}

#[test]
fn the_roles_of_a_workspace_stand_in_their_order_and_follow_a_shift() {
    let scene = scene();
    let mut model = scene.model();
    // The "everyone" role first, then from the highest position to the lowest, whatever
    // order the snapshot had.
    assert_eq!(
        role_names(&model, scene.workspace_id),
        ["everyone:0", "leads:2", "helpers:1"]
    );
    let leads = role_id(&model, scene.workspace_id, "leads");

    // A new role enters at the bottom: the shift states the absolute positions of the
    // roles that moved up, and the created role follows it (ADR 0309).
    let shift = Event::WorkspaceRolePositionsShifted {
        workspace_id: scene.workspace_id,
        positions: vec![
            WorkspaceRolePosition {
                role_id: scene.helpers,
                position: 2,
            },
            WorkspaceRolePosition {
                role_id: leads,
                position: 3,
            },
        ],
    };
    assert_eq!(model.apply(&shift), Follow::default());
    assert_eq!(
        role_names(&model, scene.workspace_id),
        ["everyone:0", "leads:3", "helpers:2"]
    );
    // The positions are absolute: a second delivery changes nothing.
    model.apply(&shift);
    assert_eq!(
        role_names(&model, scene.workspace_id),
        ["everyone:0", "leads:3", "helpers:2"]
    );
    let guests = WorkspaceRole {
        position: 1,
        id: WorkspaceRoleId::generate(),
        workspace_id: scene.workspace_id,
        name: "guests".to_owned(),
        permissions: Vec::new(),
        is_everyone: false,
    };
    model.apply(&Event::WorkspaceRoleCreated {
        role: guests.clone(),
    });
    assert_eq!(
        role_names(&model, scene.workspace_id),
        ["everyone:0", "leads:3", "helpers:2", "guests:1"]
    );

    // A reorder arrives as the updated roles with their new positions.
    let mut raised = guests;
    raised.position = 3;
    let mut lowered = model
        .workspace_role(scene.workspace_id, leads)
        .unwrap()
        .clone();
    lowered.position = 1;
    model.apply(&Event::WorkspaceRoleUpdated { role: raised });
    model.apply(&Event::WorkspaceRoleUpdated { role: lowered });
    assert_eq!(
        role_names(&model, scene.workspace_id),
        ["everyone:0", "guests:3", "helpers:2", "leads:1"]
    );

    // A shift of another workspace, and of a role that the state does not hold, changes
    // nothing.
    model.apply(&Event::WorkspaceRolePositionsShifted {
        workspace_id: WorkspaceId::generate(),
        positions: vec![WorkspaceRolePosition {
            role_id: scene.helpers,
            position: 9,
        }],
    });
    model.apply(&Event::WorkspaceRolePositionsShifted {
        workspace_id: scene.workspace_id,
        positions: vec![WorkspaceRolePosition {
            role_id: WorkspaceRoleId::generate(),
            position: 9,
        }],
    });
    assert_eq!(
        role_names(&model, scene.workspace_id),
        ["everyone:0", "guests:3", "helpers:2", "leads:1"]
    );
}

#[test]
fn the_server_roles_stand_in_their_order_and_follow_a_shift() {
    let scene = scene();
    let mut model = scene.model();
    let hosts = server_role("hosts", 1, &[ServerPermission::CreateInvites]);
    let wardens = server_role("wardens", 2, &[ServerPermission::BanMembers]);
    // A shift for a collection that no reader has open changes nothing.
    let shift = Event::ServerRolePositionsShifted {
        positions: vec![
            ServerRolePosition {
                role_id: hosts.id,
                position: 2,
            },
            ServerRolePosition {
                role_id: wardens.id,
                position: 3,
            },
        ],
    };
    assert_eq!(model.apply(&shift), Follow::default());
    assert!(model.admin.server_roles.loaded().is_none());

    load_server_roles(&mut model, vec![hosts.clone(), wardens.clone()]);
    assert_eq!(server_role_names(&model), ["wardens:2", "hosts:1"]);
    model.apply(&shift);
    model.apply(&shift);
    assert_eq!(server_role_names(&model), ["wardens:3", "hosts:2"]);
    let clerks = server_role("clerks", 1, &[]);
    model.apply(&Event::ServerRoleCreated { role: clerks });
    assert_eq!(
        server_role_names(&model),
        ["wardens:3", "hosts:2", "clerks:1"]
    );
    // The events of the time of a read are applied to the answer of that read.
    assert!(AdminPart::ServerRoles.concerns(&shift));
}

#[test]
fn the_permissions_of_the_account_are_the_lists_that_the_server_stated() {
    let mut scene = scene();
    let state = &mut scene.snapshot.workspaces[0];
    state.channel_permissions = vec![
        ChannelPermissions {
            channel_id: scene.general,
            permissions: vec![
                WorkspacePermission::ViewChannel,
                WorkspacePermission::ManageMessages,
            ],
        },
        // An entry for a channel that the snapshot does not carry belongs to nothing.
        ChannelPermissions {
            channel_id: junto_protocol::ids::ChannelId::generate(),
            permissions: vec![WorkspacePermission::ManageChannel],
        },
    ];
    let mut model = scene.model();
    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert_eq!(workspace.channel_permissions.len(), 1);
    assert!(workspace.holds(WorkspacePermission::MuteMembers));
    assert!(!workspace.holds(WorkspacePermission::ManageMessages));
    // A channel with an entry has the list of the entry, and a channel without one has
    // the list of the workspace (ADR 0310).
    assert!(model.holds_in(scene.general, WorkspacePermission::ManageMessages));
    assert!(!model.holds_in(scene.general, WorkspacePermission::MuteMembers));
    assert!(model.holds_in(scene.daily, WorkspacePermission::MuteMembers));
    assert!(!model.holds_in(scene.daily, WorkspacePermission::ManageMessages));
    // A channel that the state does not hold has no list.
    let unknown = junto_protocol::ids::ChannelId::generate();
    assert_eq!(model.permissions_in(unknown), None);

    // The role of the account changes: the lists stay as the server stated them, and the
    // notice of the server replaces the state.
    let mut helpers = model
        .workspace_role(scene.workspace_id, scene.helpers)
        .unwrap()
        .clone();
    helpers.permissions = vec![WorkspacePermission::ManageChannel];
    assert_eq!(
        model.apply(&Event::WorkspaceRoleUpdated { role: helpers }),
        Follow::default()
    );
    assert!(!model.holds_in(scene.daily, WorkspacePermission::ManageChannel));
    assert!(
        model
            .apply(&Event::PermissionsChanged {
                workspace_id: Some(scene.workspace_id)
            })
            .reload
    );

    // A channel that leaves the lists takes its entry with it.
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(
        model
            .workspace(scene.workspace_id)
            .unwrap()
            .channel_permissions
            .is_empty()
    );

    // A workspace that an event brings holds nothing for the account until its snapshot.
    let lab = Workspace {
        id: WorkspaceId::generate(),
        name: "Lab".to_owned(),
        is_default: false,
    };
    model.apply(&Event::WorkspaceCreated {
        workspace: lab.clone(),
    });
    let held = model.workspace(lab.id).unwrap();
    assert!(held.permissions.is_empty() && held.notification_default.is_none());
}

fn archive(scene: &Scene, model: &mut ServerModel) -> ArchivedChannel {
    let archived = ArchivedChannel {
        channel: model.channel(scene.general).unwrap().channel.clone(),
        archived_at: time(),
        deletes_at: None,
    };
    let part = AdminPart::Archived(scene.workspace_id);
    model.set_admin(part, AdminSet::Loading);
    model.set_admin(part, AdminSet::Loaded(AdminPayload::Archived(Vec::new())));
    model.apply(&Event::ChannelArchived {
        channel: archived.clone(),
    });
    archived
}

#[test]
fn the_permissions_in_an_archived_channel_are_the_collection_that_a_reader_opened() {
    let mut scene = scene();
    scene.snapshot.workspaces[0].channel_permissions = vec![ChannelPermissions {
        channel_id: scene.general,
        permissions: vec![
            WorkspacePermission::ViewChannel,
            WorkspacePermission::ManageChannel,
        ],
    }];
    let mut model = scene.model();
    assert!(model.holds_in(scene.general, WorkspacePermission::ManageChannel));
    archive(&scene, &mut model);
    // The snapshot states no list for an archived channel (ADR 0310, 0410): the entry
    // left with the channel, and the list is not known until its route answered.
    assert!(model.channel(scene.general).is_none());
    assert!(model.readable_channel(scene.general).is_some());
    assert_eq!(model.permissions_in(scene.general), None);
    assert!(!model.holds_in(scene.general, WorkspacePermission::ManageChannel));

    let part = AdminPart::ChannelPermissions(scene.general);
    model.set_admin(part, AdminSet::Loading);
    assert_eq!(model.admin.state(part), CollectionState::Loading);
    assert_eq!(model.permissions_in(scene.general), None);
    model.set_admin(
        part,
        AdminSet::Loaded(AdminPayload::ChannelPermissions(vec![
            WorkspacePermission::ViewChannel,
            WorkspacePermission::ManageChannel,
        ])),
    );
    assert!(model.holds_in(scene.general, WorkspacePermission::ManageChannel));
    assert!(!model.holds_in(scene.general, WorkspacePermission::ManageMessages));
    // No event carries the list: a change of it replaces the state.
    assert!(!part.concerns(&Event::PermissionsChanged { workspace_id: None }));

    // A deleted channel leaves nothing.
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
    assert_eq!(model.permissions_in(scene.general), None);
}

fn summary(roles: &[&ServerRole], is_owner: bool) -> AccountSummary {
    AccountSummary {
        id: AccountId::generate(),
        name: "Ana".to_owned(),
        status: AccountStatus::Active,
        is_owner,
        server_role_ids: roles.iter().map(|role| role.id).collect(),
        created_at: time(),
    }
}

#[test]
fn the_account_stands_on_the_server_at_its_highest_server_role() {
    let scene = scene();
    let mut model = scene.model();
    // No server role: the lowest place, known without the list of the roles.
    assert_eq!(model.server_rank(), Some(RoleRank::Server(0)));
    let hosts = server_role("hosts", 1, &[ServerPermission::CreateInvites]);
    let wardens = server_role("wardens", 2, &[ServerPermission::BanMembers]);
    let stewards = server_role("stewards", 3, &[ServerPermission::Administrator]);
    model.account.server_role_ids = vec![hosts.id, wardens.id];
    model.account.server_permissions = vec![
        ServerPermission::CreateInvites,
        ServerPermission::BanMembers,
    ];
    // The account holds roles whose positions the state does not know yet.
    assert_eq!(model.server_rank(), None);
    load_server_roles(
        &mut model,
        vec![hosts.clone(), wardens.clone(), stewards.clone()],
    );
    assert_eq!(model.server_rank(), Some(RoleRank::Server(2)));

    let own = model.server_rank().unwrap();
    let rank_of =
        |roles: &[&ServerRole], is_owner| model.server_rank_of(&summary(roles, is_owner)).unwrap();
    // A lower account can be removed; an equal one, a higher one and the owner cannot
    // (ADR 0309).
    assert!(outranks(own, rank_of(&[], false)));
    assert!(outranks(own, rank_of(&[&hosts], false)));
    assert!(!outranks(own, rank_of(&[&wardens], false)));
    assert!(!outranks(own, rank_of(&[&hosts, &stewards], false)));
    assert!(!outranks(own, rank_of(&[], true)));

    let mut owner = model.clone();
    owner.account.is_owner = true;
    assert_eq!(owner.server_rank(), Some(RoleRank::Owner));
    assert!(outranks(
        RoleRank::Owner,
        owner.server_rank_of(&summary(&[&stewards], false)).unwrap()
    ));
}

#[test]
fn the_account_stands_in_a_workspace_at_its_highest_role_or_above_them_as_an_administrator() {
    let scene = scene();
    let mut model = scene.model();
    // The account holds "helpers" at position 1 of the roles "leads" (2) and "helpers".
    assert_eq!(
        model.workspace_rank(scene.workspace_id),
        Some(RoleRank::Workspace(1))
    );
    assert_eq!(model.workspace_rank(WorkspaceId::generate()), None);

    // A server role without the administrator permission gives no place in a workspace,
    // so its position is not needed.
    let wardens = server_role("wardens", 7, &[ServerPermission::BanMembers]);
    model.account.server_role_ids = vec![wardens.id];
    model.account.server_permissions = vec![ServerPermission::BanMembers];
    assert_eq!(
        model.workspace_rank(scene.workspace_id),
        Some(RoleRank::Workspace(1))
    );

    // An administrator stands above every workspace role, at the position of its highest
    // role that gives the administrator permission.
    let stewards = server_role("stewards", 3, &[ServerPermission::Administrator]);
    model.account.server_role_ids = vec![wardens.id, stewards.id];
    model.account.server_permissions = vec![
        ServerPermission::BanMembers,
        ServerPermission::Administrator,
    ];
    assert_eq!(model.workspace_rank(scene.workspace_id), None);
    load_server_roles(&mut model, vec![wardens, stewards]);
    let rank = model.workspace_rank(scene.workspace_id).unwrap();
    assert_eq!(rank, RoleRank::Server(3));
    assert!(outranks(rank, RoleRank::Workspace(i32::MAX)));

    // Without a role the account stands at the "everyone" role, and no role is below it.
    let mut plain = scene.model();
    plain.apply(&Event::WorkspaceMemberRolesChanged {
        workspace_id: scene.workspace_id,
        account_id: scene.account_id,
        role_ids: Vec::new(),
    });
    let rank = plain.workspace_rank(scene.workspace_id).unwrap();
    assert_eq!(rank, RoleRank::Workspace(0));
    assert!(!outranks(rank, RoleRank::Workspace(0)));

    let mut owner = scene.model();
    owner.account.is_owner = true;
    assert_eq!(
        owner.workspace_rank(scene.workspace_id),
        Some(RoleRank::Owner)
    );
}

fn notice_at(seconds: i64, read: bool) -> Notification {
    let message_id = MessageId::generate();
    Notification {
        id: message_id.as_uuid(),
        message_id,
        kind: NotificationKind::Mention,
        created_at: time() + time::Duration::seconds(seconds),
        read_at: read.then(time),
    }
}

#[test]
fn one_event_marks_the_notices_up_to_its_time_and_the_list_is_read_again() {
    let mut scene = scene();
    let (old, seen, later) = (
        notice_at(-60, false),
        notice_at(0, false),
        notice_at(30, false),
    );
    let read_before = notice_at(-120, true);
    scene.snapshot.notifications = vec![
        later.clone(),
        seen.clone(),
        old.clone(),
        read_before.clone(),
    ];
    let mut model = scene.model();
    assert_eq!(model.unread_notices().count(), 3);
    let read_at = time() + time::Duration::seconds(40);
    let follow = model.apply(&Event::NotificationsRead {
        until: seen.created_at,
        read_at,
    });
    // A notice that arrived after the stated time stays unread (ADR 0412).
    let unread: Vec<_> = model.unread_notices().map(|held| held.id).collect();
    assert_eq!(unread, [later.id]);
    let held = |id| model.notices.iter().find(|held| held.id == id).unwrap();
    assert_eq!(held(seen.id).read_at, Some(read_at));
    assert_eq!(held(old.id).read_at, Some(read_at));
    // A notice that was read before keeps its time.
    assert_eq!(held(read_before.id).read_at, read_before.read_at);
    // The server can hold a notice of that time that was written after the request: the
    // list of the server decides.
    assert_eq!(follow.load, [Part::Notices]);
    assert!(!follow.reload);
    assert!(Part::Notices.concerns(
        &Event::NotificationsRead {
            until: seen.created_at,
            read_at
        },
        &model
    ));
}

#[test]
fn a_change_of_the_follow_choice_changes_the_thread_and_reads_the_lists_again() {
    let mut scene = scene();
    let (root_id, other) = (MessageId::generate(), MessageId::generate());
    let state = |root_id| ThreadState {
        root_id,
        last_read: 2,
        latest: 9,
        unread: 3,
        mentions: 0,
        following: true,
        follow_setting: None,
    };
    scene.snapshot.threads = vec![state(root_id), state(other)];
    let mut model = scene.model();
    let stopped = Event::ThreadFollowChanged {
        root_id,
        following: false,
        follow_setting: Some(false),
    };
    let follow = model.apply(&stopped);
    let held = model.thread_state(root_id).unwrap();
    assert!(!held.following);
    assert_eq!(held.follow_setting, Some(false));
    assert_eq!(model.thread_state(other).unwrap().follow_setting, None);
    // The replies of a thread that is not followed are not unread any more, and only the
    // server knows what a mention of it still counts (ADR 0412).
    assert_eq!(follow.load, [Part::UnreadThreads]);
    assert!(!follow.reload);
    assert!(Part::UnreadThreads.concerns(&stopped, &model));

    // A thread that the list does not hold: the list is read, since a thread that is
    // followed from now on can hold something unread later.
    let follow = model.apply(&Event::ThreadFollowChanged {
        root_id: MessageId::generate(),
        following: true,
        follow_setting: Some(true),
    });
    assert_eq!(follow.load, [Part::UnreadThreads]);
}

#[test]
fn a_change_of_the_default_of_a_workspace_sets_the_level_and_reloads_the_state() {
    let mut scene = scene();
    scene.snapshot.workspaces[0].notification_default = Some(NotificationLevel::All);
    let mut model = scene.model();
    let level = |model: &ServerModel| {
        model
            .workspace(scene.workspace_id)
            .unwrap()
            .notification_default
    };
    assert_eq!(level(&model), Some(NotificationLevel::All));
    let follow = model.apply(&Event::NotificationDefaultChanged {
        workspace_id: scene.workspace_id,
        level: Some(NotificationLevel::Nothing),
    });
    assert_eq!(level(&model), Some(NotificationLevel::Nothing));
    // The level decides the unread signs of every channel of the workspace that has no
    // level of its own, and only the snapshot states them all.
    assert!(follow.reload);
    // A removed default is stated as none (ADR 0412).
    model.apply(&Event::NotificationDefaultChanged {
        workspace_id: scene.workspace_id,
        level: None,
    });
    assert_eq!(level(&model), None);
    // A workspace that the state does not hold: nothing to reload.
    assert_eq!(
        model.apply(&Event::NotificationDefaultChanged {
            workspace_id: WorkspaceId::generate(),
            level: Some(NotificationLevel::All),
        }),
        Follow::default()
    );
}

#[test]
fn a_held_role_that_the_state_does_not_hold_makes_the_place_unknown() {
    let scene = scene();
    let mut model = scene.model();
    let hosts = server_role("hosts", 1, &[ServerPermission::BanMembers]);
    load_server_roles(&mut model, vec![hosts.clone()]);
    // The list of the accounts can name a role that the list of the roles does not hold
    // yet: where that account stands is not known, and it is not taken as the lowest
    // place.
    let known = summary(&[&hosts], false);
    assert_eq!(model.server_rank_of(&known), Some(RoleRank::Server(1)));
    let mut unknown = summary(&[&hosts], false);
    unknown.server_role_ids.push(ServerRoleId::generate());
    assert_eq!(model.server_rank_of(&unknown), None);
    // The same holds for the account itself.
    model.account.server_role_ids = vec![hosts.id];
    assert_eq!(model.server_rank(), Some(RoleRank::Server(1)));
    model.account.server_role_ids.push(ServerRoleId::generate());
    assert_eq!(model.server_rank(), None);

    // A role of the workspace that the account holds and the workspace does not list.
    let mut snapshot = scene.snapshot.clone();
    snapshot.workspaces[0]
        .own_role_ids
        .push(WorkspaceRoleId::generate());
    let model = ServerModel::from_snapshot(snapshot);
    assert_eq!(model.workspace_rank(scene.workspace_id), None);
}

#[test]
fn a_page_of_notices_that_was_asked_for_before_all_were_read_is_marked_too() {
    let mut scene = scene();
    let first = notice_at(0, false);
    scene.snapshot.notifications = vec![first.clone()];
    let mut model = scene.model();
    let event = Event::NotificationsRead {
        until: time() + time::Duration::seconds(10),
        read_at: time() + time::Duration::seconds(11),
    };
    // The event arrives while the page after the loaded notices is on its way: the
    // answer was made before the request that marked every notice.
    let part = Part::MoreNotices(first.id);
    assert!(part.concerns(&event, &model));
    model.apply(&event);
    let (older, newer) = (notice_at(-30, false), notice_at(40, false));
    let page = junto_protocol::discovery::NotificationPage {
        notifications: vec![older.clone(), newer.clone()],
        has_more: false,
    };
    assert!(model.add_notices(first.id, page));
    model.replay_reads(part, [&event]);
    let unread: Vec<_> = model.unread_notices().map(|held| held.id).collect();
    assert_eq!(unread, [newer.id]);
}
