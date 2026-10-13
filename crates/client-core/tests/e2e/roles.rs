//! Role order and the permissions that the server states, in the client core against the
//! real server (ADR 0309, ADR 0310): a member follows a new role, a reorder and an
//! override without a request of its own, and reads its permissions in an archived
//! channel from the route of that channel.

use junto_client_core::api::ErrorCategory;
use junto_client_core::model::{AdminPart, RoleRank, ServerModel, outranks};
use junto_client_core::protocol::ids::{WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::roles::{
    CreateServerRole, CreateWorkspaceRole, ServerRolePosition, WorkspaceRolePosition,
};
use junto_client_core::protocol::structure::SetChannelOverride;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use crate::stream::{Pair, agrees_with_a_snapshot, live, model_when, pair, text_channel};
use crate::support::{TestServer, logged_in};

/// An owner and a member on one server, each with a running event stream.
async fn scene(name: &str) -> Pair {
    let server = TestServer::start(name).await;
    let address = server.address();
    let scene = pair(server, &address).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    scene
}

/// The names of the roles of a workspace with their positions, in the order of the state.
fn roles_of(model: &ServerModel, workspace_id: WorkspaceId) -> Vec<(String, i32)> {
    model
        .workspace(workspace_id)
        .unwrap()
        .roles
        .iter()
        .map(|role| (role.name.clone(), role.position))
        .collect()
}

fn named(names: &[(&str, i32)]) -> Vec<(String, i32)> {
    names
        .iter()
        .map(|(name, position)| ((*name).to_owned(), *position))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_follows_new_roles_their_order_and_a_reorder_without_a_request() {
    let scene = scene("Role order").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let create = |name: &str, permissions: Vec<WorkspacePermission>| {
        let role = CreateWorkspaceRole {
            name: name.to_owned(),
            permissions,
        };
        async move {
            owner
                .api()
                .create_workspace_role(workspace_id, &role)
                .await
                .unwrap()
        }
    };
    // Each new role enters at the bottom, and the roles above it move up one place
    // (ADR 0309).
    let leads = create("leads", vec![WorkspacePermission::ManageRoles]).await;
    let helpers = create("helpers", Vec::new()).await;
    let guests = create("guests", Vec::new()).await;
    assert_eq!(
        (leads.position, helpers.position, guests.position),
        (1, 1, 1)
    );
    let order = named(&[("everyone", 0), ("leads", 3), ("helpers", 2), ("guests", 1)]);
    for handle in [owner, member] {
        let model = model_when(handle, "the three roles in their order", |model| {
            roles_of(model, workspace_id) == order
        })
        .await;
        assert!(model.workspace_role(workspace_id, guests.id).is_some());
    }

    // One request exchanges the positions of two roles; the member follows the events
    // of the two roles.
    owner
        .api()
        .reorder_workspace_roles(
            workspace_id,
            vec![
                WorkspaceRolePosition {
                    role_id: guests.id,
                    position: 2,
                },
                WorkspaceRolePosition {
                    role_id: helpers.id,
                    position: 1,
                },
            ],
        )
        .await
        .unwrap();
    let order = named(&[("everyone", 0), ("leads", 3), ("guests", 2), ("helpers", 1)]);
    for handle in [owner, member] {
        model_when(handle, "the new order", |model| {
            roles_of(model, workspace_id) == order
        })
        .await;
    }
    agrees_with_a_snapshot(member).await;

    // Where the member stands follows its roles, and decides what the server allows.
    let account_id = logged_in(member);
    assert_eq!(
        member
            .live()
            .view()
            .model
            .unwrap()
            .workspace_rank(workspace_id),
        Some(RoleRank::Workspace(0))
    );
    owner
        .api()
        .assign_workspace_role(workspace_id, account_id, leads.id)
        .await
        .unwrap();
    let model = model_when(member, "the role and its permission", |model| {
        model
            .workspace(workspace_id)
            .is_some_and(|workspace| workspace.holds(WorkspacePermission::ManageRoles))
    })
    .await;
    let rank = model.workspace_rank(workspace_id).unwrap();
    assert_eq!(rank, RoleRank::Workspace(3));
    let rank_of = |role_id: WorkspaceRoleId| {
        RoleRank::Workspace(
            model
                .workspace_role(workspace_id, role_id)
                .unwrap()
                .position,
        )
    };
    assert!(outranks(rank, rank_of(guests.id)));
    assert!(!outranks(rank, rank_of(leads.id)));
    // The server decides the same: a role below is moved, and the own highest role is
    // not.
    member
        .api()
        .reorder_workspace_roles(
            workspace_id,
            vec![
                WorkspaceRolePosition {
                    role_id: helpers.id,
                    position: 2,
                },
                WorkspaceRolePosition {
                    role_id: guests.id,
                    position: 1,
                },
            ],
        )
        .await
        .unwrap();
    let refused = member
        .api()
        .reorder_workspace_roles(
            workspace_id,
            vec![
                WorkspaceRolePosition {
                    role_id: leads.id,
                    position: 2,
                },
                WorkspaceRolePosition {
                    role_id: helpers.id,
                    position: 3,
                },
            ],
        )
        .await
        .expect_err("a person moved the own highest role");
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    let order = named(&[("everyone", 0), ("leads", 3), ("helpers", 2), ("guests", 1)]);
    model_when(owner, "the order after the move of the member", |model| {
        roles_of(model, workspace_id) == order
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_roles_are_read_in_their_order_by_a_holder_of_ban_members() {
    let scene = scene("Server role order").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let create = |name: &str, permissions: Vec<ServerPermission>| {
        let role = CreateServerRole {
            name: name.to_owned(),
            permissions,
        };
        async move { owner.api().create_server_role(&role).await.unwrap() }
    };
    let stewards = create("stewards", vec![ServerPermission::BanMembers]).await;
    let wardens = create("wardens", vec![ServerPermission::BanMembers]).await;
    let account_id = logged_in(member);
    owner
        .api()
        .assign_server_role(account_id, wardens.id)
        .await
        .unwrap();
    model_when(member, "the permission of the member", |model| {
        model
            .account
            .server_permissions
            .contains(&ServerPermission::BanMembers)
    })
    .await;
    // The place of the member is not known until the positions are read.
    assert_eq!(member.live().view().model.unwrap().server_rank(), None);
    member
        .live()
        .open_admin(AdminPart::ServerRoles)
        .await
        .unwrap();
    let names = |model: &ServerModel| -> Option<Vec<(String, i32)>> {
        Some(
            model
                .admin
                .server_roles
                .loaded()?
                .iter()
                .map(|role| (role.name.clone(), role.position))
                .collect(),
        )
    };
    let model = model_when(member, "the server roles in their order", |model| {
        names(model) == Some(named(&[("stewards", 2), ("wardens", 1)]))
    })
    .await;
    assert_eq!(model.server_rank(), Some(RoleRank::Server(1)));

    // A reorder and a new role reach the holder as events of the collection.
    owner
        .api()
        .reorder_server_roles(vec![
            ServerRolePosition {
                role_id: wardens.id,
                position: 2,
            },
            ServerRolePosition {
                role_id: stewards.id,
                position: 1,
            },
        ])
        .await
        .unwrap();
    let model = model_when(member, "the reordered server roles", |model| {
        names(model) == Some(named(&[("wardens", 2), ("stewards", 1)]))
    })
    .await;
    assert_eq!(model.server_rank(), Some(RoleRank::Server(2)));
    create("clerks", Vec::new()).await;
    let model = model_when(member, "the shifted server roles", |model| {
        names(model) == Some(named(&[("wardens", 3), ("stewards", 2), ("clerks", 1)]))
    })
    .await;
    assert_eq!(model.server_rank(), Some(RoleRank::Server(3)));
    member.live().release_admin(AdminPart::ServerRoles);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lists_of_the_snapshot_follow_a_role_an_override_and_an_archived_channel() {
    let scene = scene("Stated permissions").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let plans = text_channel(owner, workspace_id, "plans").await;
    let ideas = text_channel(owner, workspace_id, "ideas").await;
    let account_id = logged_in(member);
    let holds =
        |model: &ServerModel, channel_id, permission| model.holds_in(channel_id, permission);
    let model = model_when(member, "the two channels", |model| {
        model.channel(plans.id).is_some() && model.channel(ideas.id).is_some()
    })
    .await;
    assert!(holds(&model, plans.id, WorkspacePermission::ViewChannel));
    assert!(!holds(&model, plans.id, WorkspacePermission::ManageChannel));
    // The owner holds every permission, and no channel has a list of its own.
    let owned = owner.live().view().model.unwrap();
    let workspace = owned.workspace(workspace_id).unwrap();
    assert_eq!(workspace.permissions, WorkspacePermission::ALL);
    assert!(workspace.channel_permissions.is_empty());

    // A role gives the permission in the workspace; an override takes it in one channel.
    let role = CreateWorkspaceRole {
        name: "stewards".to_owned(),
        permissions: vec![WorkspacePermission::ManageChannel],
    };
    let role = owner
        .api()
        .create_workspace_role(workspace_id, &role)
        .await
        .unwrap();
    owner
        .api()
        .assign_workspace_role(workspace_id, account_id, role.id)
        .await
        .unwrap();
    model_when(member, "the permission of the role", |model| {
        holds(model, plans.id, WorkspacePermission::ManageChannel)
            && holds(model, ideas.id, WorkspacePermission::ManageChannel)
    })
    .await;
    let deny = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ManageChannel],
    };
    owner
        .api()
        .set_channel_override(ideas.id, role.id, &deny)
        .await
        .unwrap();
    let model = model_when(
        member,
        "the list of the channel with the override",
        |model| !holds(model, ideas.id, WorkspacePermission::ManageChannel),
    )
    .await;
    assert!(holds(&model, plans.id, WorkspacePermission::ManageChannel));
    assert!(holds(&model, ideas.id, WorkspacePermission::ViewChannel));
    let workspace = model.workspace(workspace_id).unwrap();
    assert_eq!(workspace.channel_permissions.len(), 1);
    assert_eq!(workspace.channel_permissions[0].channel_id, ideas.id);
    agrees_with_a_snapshot(member).await;

    // An archived channel is in no snapshot: its list is the collection that a reader
    // opens, and it is read again after a change of the permissions.
    member
        .live()
        .open_admin(AdminPart::Archived(workspace_id))
        .await
        .unwrap();
    owner.api().archive_channel(plans.id).await.unwrap();
    let model = model_when(member, "the archived channel", |model| {
        model.channel(plans.id).is_none() && model.admin.archived_channel(plans.id).is_some()
    })
    .await;
    assert_eq!(model.permissions_in(plans.id), None);
    let part = AdminPart::ChannelPermissions(plans.id);
    member.live().open_admin(part).await.unwrap();
    let model = model_when(member, "the list of the archived channel", |model| {
        model.permissions_in(plans.id).is_some()
    })
    .await;
    assert!(holds(&model, plans.id, WorkspacePermission::ManageChannel));
    owner
        .api()
        .unassign_workspace_role(workspace_id, account_id, role.id)
        .await
        .unwrap();
    model_when(
        member,
        "the list of the archived channel after the role",
        |model| {
            model
                .permissions_in(plans.id)
                .is_some_and(|held| !held.contains(&WorkspacePermission::ManageChannel))
        },
    )
    .await;
    member.live().release_admin(part);
    member
        .live()
        .release_admin(AdminPart::Archived(workspace_id));
}
