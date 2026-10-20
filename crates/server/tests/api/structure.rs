use junto_protocol::account::{AccountSummary, OwnAccount};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure::{
    Category, Channel, ChannelKind, ChannelList, ChannelOverride, Workspace, WorkspaceMember,
};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{Member, TestServer};

use WorkspacePermission as W;

async fn roles(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
) -> Vec<WorkspaceRole> {
    server
        .get(caller, &format!("/api/v1/workspaces/{workspace_id}/roles"))
        .await
        .ok(StatusCode::OK)
}

async fn everyone_role(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
) -> WorkspaceRole {
    roles(server, caller, workspace_id)
        .await
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
}

async fn create_role(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
    permissions: &[W],
) -> WorkspaceRole {
    server
        .post(
            caller,
            &format!("/api/v1/workspaces/{workspace_id}/roles"),
            &json!({ "name": "role", "permissions": permissions }),
        )
        .await
        .ok(StatusCode::CREATED)
}

async fn create_channel(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
    name: &str,
) -> Channel {
    server
        .post(
            caller,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({ "name": name, "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED)
}

async fn visible_channels(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
) -> Vec<ChannelId> {
    let list: ChannelList = server
        .get(
            caller,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
        )
        .await
        .ok(StatusCode::OK);
    list.channels
        .into_iter()
        .map(|channel| channel.id)
        .collect()
}

pub async fn set_override(
    server: &TestServer,
    caller: &Member,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
    allow: &[W],
    deny: &[W],
) -> crate::support::Reply {
    server
        .put(
            caller,
            &format!("/api/v1/channels/{channel_id}/overrides/{role_id}"),
            &json!({ "allow": allow, "deny": deny }),
        )
        .await
}

#[tokio::test]
async fn server_roles_are_created_edited_and_assigned() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;

    let role: ServerRole = server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({ "name": " Moderators ", "permissions": ["ban_members", "ban_members"] }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(role.name, "Moderators");
    assert_eq!(role.permissions, [ServerPermission::BanMembers]);

    let assignment = format!(
        "/api/v1/accounts/{}/server-roles/{}",
        guest.account_id, role.id
    );
    server
        .put_empty(&owner, &assignment)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &assignment)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&guest, "/api/v1/accounts")
        .await
        .ok::<Vec<AccountSummary>>(StatusCode::OK);
    server
        .get(&guest, "/api/v1/invites")
        .await
        .error(ErrorCode::Forbidden);

    let updated: ServerRole = server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{}", role.id),
            &json!({ "permissions": ["create_invites", "view_server_audit_log"] }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(updated.name, "Moderators");
    assert_eq!(
        updated.permissions,
        [
            ServerPermission::CreateInvites,
            ServerPermission::ViewServerAuditLog
        ]
    );
    server
        .get(&guest, "/api/v1/accounts")
        .await
        .error(ErrorCode::Forbidden);
    server
        .get(&guest, "/api/v1/invites")
        .await
        .ok::<Value>(StatusCode::OK);
    let account: OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.server_role_ids, [role.id]);

    let listed: Vec<ServerRole> = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed, [updated]);

    server
        .delete(&owner, &assignment)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&guest, "/api/v1/invites")
        .await
        .error(ErrorCode::Forbidden);

    server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({ "name": "x", "permissions": ["manage_roles"] }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{}",
                guest.account_id,
                uuid::Uuid::now_v7()
            ),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{}",
                uuid::Uuid::now_v7(),
                role.id
            ),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn an_administrator_role_gives_the_administrator_pass() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let side: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);

    let seen: Vec<Workspace> = server
        .get(&administrator, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(seen.len(), 2);
    create_role(&server, &administrator, side.id, W::ALL).await;
    let channel = create_channel(&server, &administrator, side.id, "hidden").await;
    let everyone = everyone_role(&server, &administrator, side.id).await;
    set_override(
        &server,
        &administrator,
        channel.id,
        everyone.id,
        &[],
        &[W::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    assert_eq!(
        visible_channels(&server, &administrator, side.id).await,
        [channel.id]
    );
    let renamed: Workspace = server
        .patch(
            &administrator,
            &format!("/api/v1/workspaces/{}", default_workspace.id),
            &json!({ "name": "Renamed" }),
        )
        .await
        .ok(StatusCode::OK);
    assert!(renamed.is_default);
    assert_eq!(renamed.name, "Renamed");
}

#[tokio::test]
async fn a_new_workspace_has_an_everyone_role_and_its_creator_as_member() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let creator = server.join(&owner, "Creator").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateWorkspace])
        .await;
    let outsider = server.join(&owner, "Outsider").await;

    let workspace: Workspace = server
        .post(&creator, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);
    assert!(!workspace.is_default);

    let listed = roles(&server, &creator, workspace.id).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].is_everyone);
    assert_eq!(
        listed[0].permissions,
        [W::ViewChannel, W::LockRoom, W::RunMeetingMode]
    );
    let members: Vec<WorkspaceMember> = server
        .get(
            &creator,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].account_id, creator.account_id);
    assert!(
        visible_channels(&server, &creator, workspace.id)
            .await
            .is_empty()
    );

    server
        .post(
            &creator,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({ "name": "mine" }),
        )
        .await
        .error(ErrorCode::Forbidden);

    for path in ["members", "roles", "channels", "audit-log"] {
        server
            .get(
                &outsider,
                &format!("/api/v1/workspaces/{}/{path}", workspace.id),
            )
            .await
            .error(ErrorCode::NotFound);
    }
    let seen: Vec<Workspace> = server
        .get(&outsider, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(seen.len(), 1);
    assert!(seen[0].is_default);
    server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/members", uuid::Uuid::now_v7()),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn a_member_cannot_grant_a_workspace_permission_they_lack() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    let guest = server.join(&owner, "Guest").await;
    server
        .grant_workspace(
            &owner,
            &manager,
            workspace.id,
            &[W::ManageRoles, W::RecordCalls],
        )
        .await;
    let strong = create_role(&server, &owner, workspace.id, &[W::ManageChannel]).await;
    let roles_path = format!("/api/v1/workspaces/{}/roles", workspace.id);
    let assignment = |role_id: WorkspaceRoleId| {
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{role_id}",
            workspace.id, guest.account_id
        )
    };

    server
        .post(
            &manager,
            &roles_path,
            &json!({ "name": "too much", "permissions": ["manage_channel"] }),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    server
        .post(
            &manager,
            &roles_path,
            &json!({ "name": "mixed", "permissions": ["record_calls", "export_channel"] }),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    server
        .put_empty(&manager, &assignment(strong.id))
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    server
        .patch(
            &manager,
            &format!("{roles_path}/{}", strong.id),
            &json!({ "permissions": ["manage_channel", "manage_messages"] }),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);

    let higher = server
        .grant_workspace(&owner, &guest, workspace.id, &[])
        .await;
    let within = create_role(
        &server,
        &manager,
        workspace.id,
        &[W::RecordCalls, W::ViewChannel, W::LockRoom],
    )
    .await;
    server
        .put_empty(&manager, &assignment(within.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let narrowed: WorkspaceRole = server
        .patch(
            &manager,
            &format!("{roles_path}/{}", strong.id),
            &json!({ "name": "narrowed", "permissions": [] }),
        )
        .await
        .ok(StatusCode::OK);
    assert!(narrowed.permissions.is_empty());
    server
        .patch(
            &manager,
            &format!("{roles_path}/{}", within.id),
            &json!({ "permissions": ["record_calls", "manage_roles"] }),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);

    for (method_is_put, role_id) in [(true, within.id), (false, within.id)] {
        let reply = if method_is_put {
            server.put_empty(&guest, &assignment(role_id)).await
        } else {
            server.delete(&guest, &assignment(role_id)).await
        };
        reply.empty(StatusCode::NO_CONTENT);
    }
    let members: Vec<WorkspaceMember> = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    let guest_roles = &members
        .iter()
        .find(|member| member.account_id == guest.account_id)
        .unwrap()
        .role_ids;
    assert_eq!(guest_roles, &[higher]);
}

#[tokio::test]
async fn a_role_is_assigned_only_by_a_caller_who_holds_what_its_overrides_allow() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    let friend = server.join(&owner, "Friend").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageRoles])
        .await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let private = create_channel(&server, &owner, workspace.id, "boardroom").await;
    let open = create_channel(&server, &owner, workspace.id, "lobby").await;
    set_override(
        &server,
        &owner,
        private.id,
        everyone.id,
        &[],
        &[W::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);

    let key_role = create_role(&server, &owner, workspace.id, &[]).await;
    set_override(
        &server,
        &owner,
        private.id,
        key_role.id,
        &[W::ViewChannel, W::ManageChannel, W::ViewAuditEntries],
        &[],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    let recorder_role = create_role(&server, &owner, workspace.id, &[]).await;
    set_override(
        &server,
        &owner,
        open.id,
        recorder_role.id,
        &[W::RecordCalls],
        &[],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    let harmless_role = create_role(&server, &owner, workspace.id, &[]).await;
    set_override(
        &server,
        &owner,
        open.id,
        harmless_role.id,
        &[W::LockRoom],
        &[W::RecordCalls],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);

    let assignment = |account: &Member, role_id: WorkspaceRoleId| {
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{role_id}",
            workspace.id, account.account_id
        )
    };
    for target in [&manager, &friend] {
        for role in [&key_role, &recorder_role] {
            server
                .put_empty(&manager, &assignment(target, role.id))
                .await
                .error(ErrorCode::ExceedsOwnPermissions);
        }
    }
    assert!(
        !visible_channels(&server, &manager, workspace.id)
            .await
            .contains(&private.id)
    );
    assert!(
        !visible_channels(&server, &friend, workspace.id)
            .await
            .contains(&private.id)
    );
    server
        .patch(
            &manager,
            &format!("/api/v1/channels/{}", private.id),
            &json!({ "name": "mine" }),
        )
        .await
        .error(ErrorCode::NotFound);

    server
        .put_empty(&manager, &assignment(&friend, harmless_role.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &assignment(&manager, key_role.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&manager, &assignment(&friend, key_role.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        visible_channels(&server, &friend, workspace.id)
            .await
            .contains(&private.id)
    );
}

#[tokio::test]
async fn an_administrator_adds_an_existing_account_to_a_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    let creator = server.join(&owner, "Creator").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateWorkspace])
        .await;
    let side: Workspace = server
        .post(&creator, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);
    let membership = format!(
        "/api/v1/workspaces/{}/members/{}",
        side.id, guest.account_id
    );

    server
        .put_empty(&creator, &membership)
        .await
        .error(ErrorCode::Forbidden);
    server
        .get(&guest, &format!("/api/v1/workspaces/{}/members", side.id))
        .await
        .error(ErrorCode::NotFound);

    server
        .put_empty(&owner, &membership)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &membership)
        .await
        .empty(StatusCode::NO_CONTENT);
    let members: Vec<WorkspaceMember> = server
        .get(&guest, &format!("/api/v1/workspaces/{}/members", side.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(members.len(), 2);
    let account: OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    let mut expected = vec![workspace.id, side.id];
    expected.sort();
    assert_eq!(account.workspace_ids, expected);

    let log: junto_protocol::audit::AuditPage = server
        .get(&owner, &format!("/api/v1/workspaces/{}/audit-log", side.id))
        .await
        .ok(StatusCode::OK);
    let additions: Vec<_> = log
        .entries
        .iter()
        .filter(|entry| entry.event.action() == "workspace_member_added")
        .collect();
    assert_eq!(additions.len(), 1);
    assert_eq!(additions[0].actor_id, Some(owner.account_id));
    assert_eq!(
        additions[0].event,
        junto_protocol::audit::AuditEvent::WorkspaceMemberAdded {
            account_id: guest.account_id
        }
    );

    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                side.id,
                uuid::Uuid::now_v7()
            ),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                uuid::Uuid::now_v7(),
                guest.account_id
            ),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let third: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Third" }))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                third.id, guest.account_id
            ),
        )
        .await
        .error(ErrorCode::Conflict);
}

#[tokio::test]
async fn workspace_roles_need_the_manage_roles_permission() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    let other = server.join(&owner, "Other").await;
    let role = create_role(&server, &owner, workspace.id, &[]).await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let roles_path = format!("/api/v1/workspaces/{}/roles", workspace.id);
    let assignment = format!(
        "/api/v1/workspaces/{}/members/{}/roles/{}",
        workspace.id, other.account_id, role.id
    );

    assert_eq!(roles(&server, &guest, workspace.id).await.len(), 2);
    server
        .post(&guest, &roles_path, &json!({ "name": "mine" }))
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &guest,
            &format!("{roles_path}/{}", role.id),
            &json!({ "name": "mine" }),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &guest,
            &format!("{roles_path}/{}", everyone.id),
            &json!({ "permissions": ["manage_roles"] }),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .put_empty(&guest, &assignment)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&guest, &assignment)
        .await
        .error(ErrorCode::Forbidden);

    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                workspace.id, other.account_id, everyone.id
            ),
        )
        .await
        .error(ErrorCode::Conflict);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                workspace.id,
                uuid::Uuid::now_v7(),
                role.id
            ),
        )
        .await
        .error(ErrorCode::NotFound);

    let side: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);
    let foreign = create_role(&server, &owner, side.id, &[]).await;
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                workspace.id, other.account_id, foreign.id
            ),
        )
        .await
        .error(ErrorCode::NotFound);

    let widened: WorkspaceRole = server
        .patch(
            &owner,
            &format!("{roles_path}/{}", everyone.id),
            &json!({ "permissions": ["view_channel", "manage_roles"] }),
        )
        .await
        .ok(StatusCode::OK);
    assert!(widened.is_everyone);
    server
        .grant_workspace(&owner, &guest, workspace.id, &[])
        .await;
    server
        .post(
            &guest,
            &roles_path,
            &json!({ "name": "mine", "permissions": ["manage_roles"] }),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::CREATED);
}

#[tokio::test]
async fn categories_and_channels_are_created_listed_and_edited() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    let base = format!("/api/v1/workspaces/{}", workspace.id);

    let category: Category = server
        .post(
            &owner,
            &format!("{base}/categories"),
            &json!({ "name": "Projects" }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(category.position, 0);
    let second: Category = server
        .post(
            &owner,
            &format!("{base}/categories"),
            &json!({ "name": "Archive" }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(second.position, 1);
    let moved: Category = server
        .patch(
            &owner,
            &format!("{base}/categories/{}", second.id),
            &json!({ "position": -1 }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!((moved.name.as_str(), moved.position), ("Archive", -1));

    let channel: Channel = server
        .post(
            &owner,
            &format!("{base}/channels"),
            &json!({ "name": "plans", "kind": "voice", "category_id": category.id }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(channel.kind, ChannelKind::Voice);
    assert_eq!(channel.position, 2);
    assert_eq!(channel.category_id, Some(category.id));
    assert!(!channel.permanent_messages);

    let path = format!("/api/v1/channels/{}", channel.id);
    let edited: Channel = server
        .patch(
            &owner,
            &path,
            &json!({ "name": "roadmap", "permanent_messages": true, "position": 7 }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(edited.name, "roadmap");
    assert_eq!(edited.category_id, Some(category.id));
    assert!(edited.permanent_messages);
    let uncategorised: Channel = server
        .patch(&owner, &path, &json!({ "category_id": null }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(uncategorised.category_id, None);
    assert_eq!(uncategorised.position, 7);

    let list: ChannelList = server
        .get(&guest, &format!("{base}/channels"))
        .await
        .ok(StatusCode::OK);
    let categories: Vec<&str> = list.categories.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(categories, ["Archive", "Projects"]);
    let names: Vec<&str> = list.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["general", "voice", "roadmap"]);

    server
        .post(
            &guest,
            &format!("{base}/categories"),
            &json!({ "name": "mine" }),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &guest,
            &format!("{base}/categories/{}", category.id),
            &json!({ "name": "mine" }),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(
            &guest,
            &format!("{base}/channels"),
            &json!({ "name": "mine", "kind": "text" }),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(&guest, &path, &json!({ "name": "mine" }))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&guest, &path)
        .await
        .error(ErrorCode::Forbidden);
    server
        .get(&guest, &format!("{path}/overrides"))
        .await
        .error(ErrorCode::Forbidden);

    let side: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);
    let foreign: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/categories", side.id),
            &json!({ "name": "Elsewhere" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .patch(&owner, &path, &json!({ "category_id": foreign.id }))
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &owner,
            &format!("{base}/channels"),
            &json!({ "name": "x", "kind": "text", "category_id": foreign.id }),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &owner,
            &format!("{base}/channels"),
            &json!({ "name": "x", "kind": "video" }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn a_private_channel_is_seen_only_through_a_role_override() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let insider = server.join(&owner, "Insider").await;
    let outsider = server.join(&owner, "Outsider").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let insider_role = server
        .grant_workspace(&owner, &insider, workspace.id, &[W::ManageChannel])
        .await;
    server
        .grant_workspace(&owner, &outsider, workspace.id, &[W::ManageChannel])
        .await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let channel = create_channel(&server, &owner, workspace.id, "board").await;

    assert!(
        visible_channels(&server, &outsider, workspace.id)
            .await
            .contains(&channel.id)
    );
    set_override(
        &server,
        &owner,
        channel.id,
        everyone.id,
        &[],
        &[W::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    set_override(
        &server,
        &owner,
        channel.id,
        insider_role,
        &[W::ViewChannel],
        &[],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);

    assert!(
        visible_channels(&server, &insider, workspace.id)
            .await
            .contains(&channel.id)
    );
    assert!(
        !visible_channels(&server, &outsider, workspace.id)
            .await
            .contains(&channel.id)
    );
    assert!(
        visible_channels(&server, &administrator, workspace.id)
            .await
            .contains(&channel.id)
    );
    assert!(
        visible_channels(&server, &owner, workspace.id)
            .await
            .contains(&channel.id)
    );

    let path = format!("/api/v1/channels/{}", channel.id);
    server
        .patch(&outsider, &path, &json!({ "name": "found" }))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&outsider, &path)
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&outsider, &format!("{path}/overrides"))
        .await
        .error(ErrorCode::NotFound);
    set_override(
        &server,
        &outsider,
        channel.id,
        everyone.id,
        &[W::ViewChannel],
        &[],
    )
    .await
    .error(ErrorCode::NotFound);

    server
        .patch(&insider, &path, &json!({ "name": "board room" }))
        .await
        .ok::<Channel>(StatusCode::OK);
    let overrides: Vec<ChannelOverride> = server
        .get(&insider, &format!("{path}/overrides"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(overrides.len(), 2);

    set_override(&server, &owner, channel.id, everyone.id, &[], &[])
        .await
        .ok::<ChannelOverride>(StatusCode::OK);
    assert!(
        visible_channels(&server, &outsider, workspace.id)
            .await
            .contains(&channel.id)
    );
    let overrides: Vec<ChannelOverride> = server
        .get(&owner, &format!("{path}/overrides"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(overrides.len(), 1);
}

#[tokio::test]
async fn an_allow_of_one_role_wins_over_a_deny_of_another_role() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let member = server.join(&owner, "Member").await;
    let denied = server
        .grant_workspace(&owner, &member, workspace.id, &[W::ManageChannel])
        .await;
    let channel = create_channel(&server, &owner, workspace.id, "plans").await;
    let path = format!("/api/v1/channels/{}", channel.id);

    set_override(
        &server,
        &owner,
        channel.id,
        denied,
        &[],
        &[W::ManageChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    server
        .patch(&member, &path, &json!({ "name": "mine" }))
        .await
        .error(ErrorCode::Forbidden);

    let allowed = server
        .grant_workspace(&owner, &member, workspace.id, &[])
        .await;
    set_override(
        &server,
        &owner,
        channel.id,
        allowed,
        &[W::ManageChannel],
        &[],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    server
        .patch(&member, &path, &json!({ "name": "mine" }))
        .await
        .ok::<Channel>(StatusCode::OK);
}

#[tokio::test]
async fn a_member_cannot_grant_a_channel_permission_they_lack() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageChannel])
        .await;
    let target = create_role(&server, &owner, workspace.id, &[]).await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let channel = create_channel(&server, &owner, workspace.id, "plans").await;

    set_override(
        &server,
        &manager,
        channel.id,
        target.id,
        &[W::RecordCalls],
        &[],
    )
    .await
    .error(ErrorCode::ExceedsOwnPermissions);
    set_override(
        &server,
        &manager,
        channel.id,
        target.id,
        &[W::ManageChannel],
        &[W::RecordCalls],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);

    set_override(
        &server,
        &owner,
        channel.id,
        everyone.id,
        &[],
        &[W::ExportChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    set_override(&server, &manager, channel.id, everyone.id, &[], &[])
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    set_override(
        &server,
        &manager,
        channel.id,
        everyone.id,
        &[],
        &[W::ExportChannel, W::LockRoom],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    set_override(
        &server,
        &manager,
        channel.id,
        everyone.id,
        &[],
        &[W::ExportChannel],
    )
    .await
    .error(ErrorCode::ExceedsOwnPermissions);
    set_override(
        &server,
        &owner,
        channel.id,
        everyone.id,
        &[],
        &[W::ExportChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);

    set_override(
        &server,
        &owner,
        channel.id,
        target.id,
        &[W::LockRoom],
        &[W::LockRoom],
    )
    .await
    .error(ErrorCode::InvalidRequest);
    set_override(
        &server,
        &owner,
        channel.id,
        WorkspaceRoleId::from_uuid(uuid::Uuid::now_v7()),
        &[W::LockRoom],
        &[],
    )
    .await
    .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn the_database_refuses_states_that_break_the_invariants() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let side: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);
    let guest = server.join(&owner, "Guest").await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let foreign = create_role(&server, &owner, side.id, &[]).await;
    let local = create_role(&server, &owner, workspace.id, &[]).await;
    let channel = create_channel(&server, &owner, workspace.id, "plans").await;
    server.invite(&owner, json!({})).await;

    let workspace_id = workspace.id.as_uuid();
    let guest_id = guest.account_id.as_uuid();
    let statements: Vec<(&str, String)> = vec![
        (
            "the everyone role is assigned",
            format!(
                "insert into member_roles (workspace_id, account_id, role_id)
                 values ('{workspace_id}', '{guest_id}', '{}')",
                everyone.id
            ),
        ),
        (
            "the everyone role is assigned with a forged flag",
            format!(
                "insert into member_roles (workspace_id, account_id, role_id, is_everyone)
                 values ('{workspace_id}', '{guest_id}', '{}', true)",
                everyone.id
            ),
        ),
        (
            "a role of another workspace is assigned",
            format!(
                "insert into member_roles (workspace_id, account_id, role_id)
                 values ('{workspace_id}', '{guest_id}', '{}')",
                foreign.id
            ),
        ),
        (
            "a role is assigned to an account that is not a member",
            format!(
                "insert into member_roles (workspace_id, account_id, role_id)
                 values ('{}', '{guest_id}', '{}')",
                side.id, foreign.id
            ),
        ),
        (
            "a workspace gets a second everyone role",
            format!(
                "insert into workspace_roles (id, workspace_id, name, permissions, is_everyone, created_at)
                 values (gen_random_uuid(), '{workspace_id}', 'all', 0, true, now())"
            ),
        ),
        (
            "a role holds an unknown permission bit",
            format!("update workspace_roles set permissions = 65536 where id = '{}'", local.id),
        ),
        (
            "an override allows and denies one permission",
            format!(
                "insert into channel_overrides (channel_id, role_id, workspace_id, allow, deny)
                 values ('{}', '{}', '{workspace_id}', 3, 1)",
                channel.id, local.id
            ),
        ),
        (
            "an override names a role of another workspace",
            format!(
                "insert into channel_overrides (channel_id, role_id, workspace_id, allow, deny)
                 values ('{}', '{}', '{workspace_id}', 1, 0)",
                channel.id, foreign.id
            ),
        ),
        (
            "an invite is used more often than its maximum",
            "update invites set uses = max_uses + 1".to_owned(),
        ),
        (
            "a device key belongs to two accounts",
            format!(
                "insert into devices (id, account_id, public_key, name, origin, created_at)
                 select gen_random_uuid(), '{guest_id}', public_key, '', 'link', now()
                 from devices limit 1"
            ),
        ),
        (
            "an account has an unknown status",
            "update accounts set status = 'suspended'".to_owned(),
        ),
        (
            "a second server state row exists",
            "insert into server_state (singleton, server_id, name, default_workspace_id, created_at)
             select false, server_id, name, default_workspace_id, created_at from server_state"
                .to_owned(),
        ),
        (
            "an audit entry is changed",
            "update audit_entries set occurred_at = now()".to_owned(),
        ),
        (
            "an audit entry is deleted",
            "delete from audit_entries".to_owned(),
        ),
        (
            "the audit log is emptied",
            "truncate audit_entries".to_owned(),
        ),
    ];
    for (what, statement) in statements {
        let result = sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&server.db)
            .await;
        assert!(result.is_err(), "the database accepted: {what}");
    }
}

#[tokio::test]
async fn channels_are_moved_together_or_not_at_all() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageChannel])
        .await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let category: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/categories", workspace.id),
            &json!({ "name": "Projects" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let plans = create_channel(&server, &owner, workspace.id, "plans").await;
    let notes = create_channel(&server, &owner, workspace.id, "notes").await;
    let closed = create_channel(&server, &owner, workspace.id, "closed").await;
    // The manager may not change "closed".
    set_override(
        &server,
        &owner,
        closed.id,
        everyone.id,
        &[],
        &[W::ManageChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    let path = format!("/api/v1/workspaces/{}/channels/positions", workspace.id);
    let places = || async {
        let list: ChannelList = server
            .get(
                &owner,
                &format!("/api/v1/workspaces/{}/channels", workspace.id),
            )
            .await
            .ok(StatusCode::OK);
        list.channels
            .into_iter()
            .map(|channel| (channel.id, (channel.category_id, channel.position)))
            .collect::<std::collections::HashMap<_, _>>()
    };
    let before = places().await;
    let place = |channel: &Channel| json!({ "category_id": channel.category_id, "position": channel.position });
    let to = |category_id: Option<_>, position: i32| json!({ "category_id": category_id, "position": position });

    // A move of a channel that the manager may not change refuses the whole request: the
    // first channel stays where it was.
    server
        .patch(
            &manager,
            &path,
            &json!({ "moves": [
                { "channel_id": plans.id, "from": place(&plans), "to": to(None, closed.position) },
                { "channel_id": closed.id, "from": place(&closed), "to": to(None, plans.position) },
            ]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    assert_eq!(places().await, before);

    // A channel that is no longer where the request read it: nothing changes.
    server
        .patch(
            &manager,
            &path,
            &json!({ "moves": [
                { "channel_id": plans.id, "from": place(&plans), "to": to(Some(category.id), 0) },
                { "channel_id": notes.id, "from": to(None, notes.position + 1), "to": to(None, 0) },
            ]}),
        )
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(places().await, before);

    // No move, and a channel named twice, are refused.
    server
        .patch(&manager, &path, &json!({ "moves": [] }))
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .patch(
            &manager,
            &path,
            &json!({ "moves": [
                { "channel_id": plans.id, "from": place(&plans), "to": to(None, 5) },
                { "channel_id": plans.id, "from": place(&plans), "to": to(None, 6) },
            ]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);

    // Every move applies: "plans" goes into the category, and "notes" takes its position.
    server
        .patch(
            &manager,
            &path,
            &json!({ "moves": [
                { "channel_id": plans.id, "from": place(&plans), "to": to(Some(category.id), 0) },
                { "channel_id": notes.id, "from": place(&notes), "to": to(None, plans.position) },
            ]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let after = places().await;
    assert_eq!(after[&plans.id], (Some(category.id), 0));
    assert_eq!(after[&notes.id], (None, plans.position));
    assert_eq!(after[&closed.id], before[&closed.id]);
}

/// A page drops "x" after "b", between "b" at 10 and "c" at 20, and sends the two neighbours
/// as checks. Another administrator moved "b" first: the move of "x" alone would still apply
/// and put "x" before both channels, so the check refuses it. A check needs "view channel"
/// only and writes nothing.
#[tokio::test]
async fn a_move_is_refused_when_a_neighbour_that_it_checks_has_moved() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageChannel])
        .await;
    let everyone = everyone_role(&server, &owner, workspace.id).await;
    let mut placed = Vec::new();
    for (name, position) in [("b", 10), ("c", 20), ("x", 30), ("hidden", 40)] {
        let channel = create_channel(&server, &owner, workspace.id, name).await;
        let channel: Channel = server
            .patch(
                &owner,
                &format!("/api/v1/channels/{}", channel.id),
                &json!({ "position": position }),
            )
            .await
            .ok(StatusCode::OK);
        placed.push(channel);
    }
    let [b, c, x, hidden] = <[Channel; 4]>::try_from(placed).unwrap();
    // The manager sees "c" but may not change it, and does not see "hidden".
    set_override(&server, &owner, c.id, everyone.id, &[], &[W::ManageChannel])
        .await
        .ok::<ChannelOverride>(StatusCode::OK);
    set_override(
        &server,
        &owner,
        hidden.id,
        everyone.id,
        &[],
        &[W::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    let path = format!("/api/v1/workspaces/{}/channels/positions", workspace.id);
    let at = |position: i32| json!({ "category_id": null, "position": position });
    let drop = |neighbour: &Channel, read: i32| {
        json!({ "moves": [
            { "channel_id": x.id, "from": at(30), "to": at(11) },
            { "channel_id": neighbour.id, "from": at(read), "to": at(read) },
            { "channel_id": c.id, "from": at(20), "to": at(20) },
        ]})
    };
    let place_of = |id: ChannelId| {
        let server = &server;
        let owner = &owner;
        async move {
            let list: ChannelList = server
                .get(
                    owner,
                    &format!("/api/v1/workspaces/{}/channels", workspace.id),
                )
                .await
                .ok(StatusCode::OK);
            list.channels
                .into_iter()
                .find(|channel| channel.id == id)
                .map(|channel| (channel.category_id, channel.position))
                .unwrap()
        }
    };
    let audit_entries = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from audit_entries")
            .fetch_one(&server.db)
            .await
            .unwrap()
    };

    // A check of a channel that the manager does not see refuses the request.
    server
        .patch(&manager, &path, &drop(&hidden, 40))
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(place_of(x.id).await, (None, 30));

    // The other administrator moves "b"; the page still read it at 10.
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", b.id),
            &json!({ "position": 40 }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    server
        .patch(&manager, &path, &drop(&b, 10))
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(place_of(x.id).await, (None, 30));

    // From the current page the drop applies. The checks change nothing, so "x" has the only
    // audit entry, and "c", which the manager may not change, is checked.
    let entries = audit_entries().await;
    server
        .patch(&manager, &path, &drop(&b, 40))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(place_of(x.id).await, (None, 11));
    assert_eq!(place_of(b.id).await, (None, 40));
    assert_eq!(place_of(c.id).await, (None, 20));
    assert_eq!(audit_entries().await, entries + 1);
}
