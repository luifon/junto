//! The deletion of roles, categories and workspaces (ADR 0308), and the removal of the
//! content of deleted channels and workspaces in bounded parts (ADR 0017).

use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{CategoryId, ChannelId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::invite::{Invite, InviteCreated, JoinRequest};
use junto_protocol::message::Conversation;
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure::{Category, Channel, ChannelList, Workspace, WorkspaceMember};
use junto_protocol::structure_file::StructureExport;
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::messages_support::{Files, send, text};
use crate::recordings_support::Studio;
use crate::support::{DeviceKey, Member, Reply, TestServer};

async fn workspace(server: &TestServer, owner: &Member, name: &str) -> Workspace {
    server
        .post(owner, "/api/v1/workspaces", &json!({ "name": name }))
        .await
        .ok(StatusCode::CREATED)
}

async fn role(
    server: &TestServer,
    creator: &Member,
    workspace_id: WorkspaceId,
    permissions: &[WorkspacePermission],
) -> WorkspaceRole {
    server
        .post(
            creator,
            &format!("/api/v1/workspaces/{workspace_id}/roles"),
            &json!({ "name": "role", "permissions": permissions }),
        )
        .await
        .ok(StatusCode::CREATED)
}

async fn assign(
    server: &TestServer,
    granter: &Member,
    workspace_id: WorkspaceId,
    target: &Member,
    role_id: WorkspaceRoleId,
) {
    server
        .put_empty(
            granter,
            &format!(
                "/api/v1/workspaces/{workspace_id}/members/{}/roles/{role_id}",
                target.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

async fn delete_role(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
    role_id: WorkspaceRoleId,
) -> Reply {
    server
        .delete(
            caller,
            &format!("/api/v1/workspaces/{workspace_id}/roles/{role_id}"),
        )
        .await
}

async fn delete_workspace(server: &TestServer, caller: &Member, id: WorkspaceId) -> Reply {
    server
        .delete(caller, &format!("/api/v1/workspaces/{id}"))
        .await
}

async fn set_override(
    server: &TestServer,
    manager: &Member,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
    allow: &[WorkspacePermission],
    deny: &[WorkspacePermission],
) {
    server
        .put(
            manager,
            &format!("/api/v1/channels/{channel_id}/overrides/{role_id}"),
            &json!({ "allow": allow, "deny": deny }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

async fn count(server: &TestServer, statement: &str, id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(statement.to_owned()))
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn roles_of(server: &TestServer, reader: &Member, id: WorkspaceId) -> Vec<WorkspaceRoleId> {
    let roles: Vec<WorkspaceRole> = server
        .get(reader, &format!("/api/v1/workspaces/{id}/roles"))
        .await
        .ok(StatusCode::OK);
    roles.into_iter().map(|role| role.id).collect()
}

async fn server_audit(server: &TestServer, reader: &Member) -> Vec<AuditEvent> {
    let page: AuditPage = server
        .get(reader, "/api/v1/audit-log?limit=200")
        .await
        .ok(StatusCode::OK);
    page.entries.into_iter().map(|entry| entry.event).collect()
}

async fn workspace_audit(server: &TestServer, reader: &Member, id: WorkspaceId) -> Vec<AuditEvent> {
    let page: AuditPage = server
        .get(
            reader,
            &format!("/api/v1/workspaces/{id}/audit-log?limit=200"),
        )
        .await
        .ok(StatusCode::OK);
    page.entries.into_iter().map(|entry| entry.event).collect()
}

async fn events_of(
    server: &TestServer,
    member: &Member,
    stream: &mut crate::stream_support::Stream,
) -> Vec<Event> {
    server
        .traffic(member, stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}

#[tokio::test]
async fn rule_d01_a_workspace_role_is_deleted_with_manage_roles() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let other = workspace(&server, &owner, "Other").await;
    let plain = server.join(&owner, "Plain").await;
    let outsider = server.join_into(&owner, "Outsider", &[other.id]).await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(
            &owner,
            &manager,
            home.id,
            &[WorkspacePermission::ManageRoles],
        )
        .await;
    let helpers = role(&server, &owner, home.id, &[]).await;
    assign(&server, &owner, home.id, &plain, helpers.id).await;
    let elsewhere = role(&server, &owner, other.id, &[]).await;
    let everyone = server.everyone_role(&owner, home.id).await;

    delete_role(&server, &plain, home.id, helpers.id)
        .await
        .error(ErrorCode::Forbidden);
    delete_role(&server, &outsider, home.id, helpers.id)
        .await
        .error(ErrorCode::NotFound);
    assert!(
        roles_of(&server, &owner, home.id)
            .await
            .contains(&helpers.id)
    );

    delete_role(&server, &manager, home.id, WorkspaceRoleId::generate())
        .await
        .error(ErrorCode::NotFound);
    delete_role(&server, &manager, home.id, elsewhere.id)
        .await
        .error(ErrorCode::NotFound);
    assert!(
        roles_of(&server, &owner, other.id)
            .await
            .contains(&elsewhere.id)
    );
    delete_role(&server, &manager, home.id, everyone.id)
        .await
        .error(ErrorCode::Conflict);
    delete_role(&server, &owner, home.id, everyone.id)
        .await
        .error(ErrorCode::Conflict);
    assert!(
        roles_of(&server, &owner, home.id)
            .await
            .contains(&everyone.id)
    );

    delete_role(&server, &manager, home.id, helpers.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        !roles_of(&server, &owner, home.id)
            .await
            .contains(&helpers.id)
    );
    let members: Vec<WorkspaceMember> = server
        .get(&owner, &format!("/api/v1/workspaces/{}/members", home.id))
        .await
        .ok(StatusCode::OK);
    let of_plain = members
        .iter()
        .find(|member| member.account_id == plain.account_id)
        .unwrap();
    assert!(of_plain.role_ids.is_empty());
    delete_role(&server, &manager, home.id, helpers.id)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn rule_d02_deleting_a_role_cannot_lift_a_deny_that_the_caller_could_not_lift() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    let manage = [WorkspacePermission::ManageRoles];
    server
        .grant_workspace(&owner, &manager, home.id, &manage)
        .await;
    let channel = server.channel(&owner, home.id, "plans").await;
    let hidden = server.channel(&owner, home.id, "hidden").await;
    let insiders = role(&server, &owner, home.id, &[]).await;
    server
        .make_private(&owner, home.id, hidden.id, insiders.id)
        .await;
    let messages = [WorkspacePermission::ManageMessages];

    // A deny of a permission that the caller does not hold in the channel.
    let muted = role(&server, &owner, home.id, &[]).await;
    set_override(&server, &owner, channel.id, muted.id, &[], &messages).await;
    delete_role(&server, &manager, home.id, muted.id)
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    assert!(roles_of(&server, &owner, home.id).await.contains(&muted.id));

    // A deny in a channel that the caller cannot see.
    let unseen = role(&server, &owner, home.id, &[]).await;
    let lock = [WorkspacePermission::LockRoom];
    set_override(&server, &owner, hidden.id, unseen.id, &[], &lock).await;
    delete_role(&server, &manager, home.id, unseen.id)
        .await
        .error(ErrorCode::ExceedsOwnPermissions);

    // A role whose override only allows is deleted with the managing permission alone.
    let allowed = role(&server, &owner, home.id, &[]).await;
    set_override(&server, &owner, channel.id, allowed.id, &messages, &[]).await;
    delete_role(&server, &manager, home.id, allowed.id)
        .await
        .empty(StatusCode::NO_CONTENT);

    // The caller who holds the denied permission in the channel lifts the deny.
    server
        .grant_workspace(&owner, &manager, home.id, &messages)
        .await;
    delete_role(&server, &manager, home.id, muted.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    delete_role(&server, &manager, home.id, unseen.id)
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    delete_role(&server, &owner, home.id, unseen.id)
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rule_d03_a_deleted_role_leaves_no_row() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let holder = server.join(&owner, "Holder").await;
    let channel = server.channel(&owner, home.id, "plans").await;
    let managing = role(
        &server,
        &owner,
        home.id,
        &[WorkspacePermission::ManageChannel],
    )
    .await;
    assign(&server, &owner, home.id, &holder, managing.id).await;
    let lock = [WorkspacePermission::LockRoom];
    set_override(&server, &owner, channel.id, managing.id, &lock, &[]).await;
    sqlx::query(
        "insert into structure_identifiers (kind, identifier, object_id)
         values ('workspace_role', gen_random_uuid(), $1)",
    )
    .bind(managing.id)
    .execute(&server.db)
    .await
    .unwrap();
    let kept = role(&server, &owner, home.id, &[]).await;
    assign(&server, &owner, home.id, &holder, kept.id).await;
    set_override(&server, &owner, channel.id, kept.id, &lock, &[]).await;

    let rename = json!({ "name": "renamed" });
    let path = format!("/api/v1/channels/{}", channel.id);
    server
        .patch(&holder, &path, &rename)
        .await
        .ok::<Channel>(StatusCode::OK);

    delete_role(&server, &owner, home.id, managing.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(&holder, &path, &rename)
        .await
        .error(ErrorCode::Forbidden);
    for statement in [
        "select count(*) from workspace_roles where id = $1",
        "select count(*) from member_roles where role_id = $1",
        "select count(*) from channel_overrides where role_id = $1",
        "select count(*) from structure_identifiers where object_id = $1",
    ] {
        assert_eq!(
            count(&server, statement, managing.id.as_uuid()).await,
            0,
            "{statement}"
        );
    }
    // Nothing of another role is removed.
    for statement in [
        "select count(*) from member_roles where role_id = $1",
        "select count(*) from channel_overrides where role_id = $1",
    ] {
        assert_eq!(
            count(&server, statement, kept.id.as_uuid()).await,
            1,
            "{statement}"
        );
    }
}

#[tokio::test]
async fn rule_d04_role_deletion_events() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let other = workspace(&server, &owner, "Other").await;
    let holder = server.join(&owner, "Holder").await;
    let member = server.join(&owner, "Member").await;
    let outsider = server.join_into(&owner, "Outsider", &[other.id]).await;
    let helpers = role(&server, &owner, home.id, &[]).await;
    assign(&server, &owner, home.id, &holder, helpers.id).await;
    let mut of_holder = server.subscribed(&holder, Level::Full).await;
    let mut of_member = server.subscribed(&member, Level::Full).await;
    let mut of_outsider = server.subscribed(&outsider, Level::Full).await;

    delete_role(&server, &owner, home.id, helpers.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    let deleted = Event::WorkspaceRoleDeleted {
        workspace_id: home.id,
        role_id: helpers.id,
    };
    let notice = Event::PermissionsChanged {
        workspace_id: Some(home.id),
    };
    assert_eq!(
        events_of(&server, &holder, &mut of_holder).await,
        [notice, deleted.clone()]
    );
    assert_eq!(events_of(&server, &member, &mut of_member).await, [deleted]);
    assert!(
        events_of(&server, &outsider, &mut of_outsider)
            .await
            .is_empty()
    );

    let entries = workspace_audit(&server, &owner, home.id).await;
    assert!(
        entries.contains(&AuditEvent::WorkspaceRoleDeleted {
            role_id: helpers.id,
            name: "role".to_owned(),
            permissions: vec![],
            holders: 1,
        }),
        "{entries:?}"
    );
}

#[tokio::test]
async fn rule_d05_a_server_role_is_deleted_by_an_administrator() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let inviter = server.join(&owner, "Inviter").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let inviters = server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;
    let kept = server
        .grant_server(&owner, &inviter, &[ServerPermission::ViewServerAuditLog])
        .await;
    let path = |id: ServerRoleId| format!("/api/v1/server-roles/{id}");
    server.invite(&inviter, json!({})).await;

    for refused in [&plain, &inviter] {
        server
            .delete(refused, &path(inviters))
            .await
            .error(ErrorCode::Forbidden);
    }
    server
        .delete(&administrator, &path(ServerRoleId::generate()))
        .await
        .error(ErrorCode::NotFound);
    let listed = |roles: Vec<ServerRole>| -> Vec<ServerRoleId> {
        roles.into_iter().map(|role| role.id).collect()
    };
    let before: Vec<ServerRole> = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    assert!(listed(before).contains(&inviters));

    let mut of_inviter = server.subscribed(&inviter, Level::Full).await;
    let mut of_plain = server.subscribed(&plain, Level::Full).await;
    let mut of_owner = server.subscribed(&owner, Level::Full).await;
    server
        .delete(&administrator, &path(inviters))
        .await
        .empty(StatusCode::NO_CONTENT);

    let after: Vec<ServerRole> = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    let after = listed(after);
    assert!(!after.contains(&inviters) && after.contains(&kept));
    server
        .post(&inviter, "/api/v1/invites", &json!({}))
        .await
        .error(ErrorCode::Forbidden);
    server
        .get(&inviter, "/api/v1/audit-log")
        .await
        .ok::<AuditPage>(StatusCode::OK);
    assert_eq!(
        count(
            &server,
            "select count(*) from account_server_roles where role_id = $1",
            inviters.as_uuid()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &server,
            "select count(*) from account_server_roles where role_id = $1",
            kept.as_uuid()
        )
        .await,
        1
    );

    assert_eq!(
        events_of(&server, &inviter, &mut of_inviter).await,
        [Event::PermissionsChanged { workspace_id: None }]
    );
    assert!(events_of(&server, &plain, &mut of_plain).await.is_empty());
    assert_eq!(
        events_of(&server, &owner, &mut of_owner).await,
        [Event::ServerRoleDeleted { role_id: inviters }]
    );
    let entries = server_audit(&server, &owner).await;
    assert!(
        entries.contains(&AuditEvent::ServerRoleDeleted {
            role_id: inviters,
            name: "role".to_owned(),
            permissions: vec![ServerPermission::CreateInvites],
            holders: 1,
        }),
        "{entries:?}"
    );
}

#[tokio::test]
async fn rule_d06_a_deleted_category_leaves_its_channels() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let other = workspace(&server, &owner, "Other").await;
    let plain = server.join(&owner, "Plain").await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(
            &owner,
            &manager,
            home.id,
            &[WorkspacePermission::ManageChannel],
        )
        .await;
    let insiders = role(&server, &owner, home.id, &[]).await;
    let projects = server.category(&owner, home.id, "Projects", 0).await;
    let kept = server.category(&owner, home.id, "Kept", 1).await;
    let elsewhere = server.category(&owner, other.id, "Elsewhere", 0).await;
    let in_category = |name: &'static str, category: &Category| {
        let (server, owner, home, category_id) = (&server, &owner, home.id, category.id);
        async move {
            server
                .post(
                    owner,
                    &format!("/api/v1/workspaces/{home}/channels"),
                    &json!({ "name": name, "kind": "text", "category_id": category_id }),
                )
                .await
                .ok::<Channel>(StatusCode::CREATED)
        }
    };
    let open = in_category("open", &projects).await;
    let hidden = in_category("hidden", &projects).await;
    let gone = in_category("gone", &projects).await;
    let beside = in_category("beside", &kept).await;
    server
        .make_private(&owner, home.id, hidden.id, insiders.id)
        .await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", gone.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let path = |workspace: WorkspaceId, category: CategoryId| {
        format!("/api/v1/workspaces/{workspace}/categories/{category}")
    };

    server
        .delete(&plain, &path(home.id, projects.id))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&manager, &path(home.id, CategoryId::generate()))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&manager, &path(home.id, elsewhere.id))
        .await
        .error(ErrorCode::NotFound);
    let categories = |list: ChannelList| -> Vec<CategoryId> {
        list.categories.into_iter().map(|found| found.id).collect()
    };
    let list: ChannelList = server
        .get(&owner, &format!("/api/v1/workspaces/{}/channels", home.id))
        .await
        .ok(StatusCode::OK);
    assert!(categories(list).contains(&projects.id));

    let mut of_plain = server.subscribed(&plain, Level::Full).await;
    // The manager does not see the hidden channel, and deletes its category.
    server
        .delete(&manager, &path(home.id, projects.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let list: ChannelList = server
        .get(&owner, &format!("/api/v1/workspaces/{}/channels", home.id))
        .await
        .ok(StatusCode::OK);
    for channel in &list.channels {
        let expected = (channel.id == beside.id).then_some(kept.id);
        if [open.id, hidden.id, beside.id].contains(&channel.id) {
            assert_eq!(channel.category_id, expected, "{}", channel.name);
        }
    }
    assert!(list.channels.iter().any(|channel| channel.id == hidden.id));
    let left = categories(list);
    assert!(!left.contains(&projects.id) && left.contains(&kept.id));
    assert_eq!(
        count(
            &server,
            "select count(*) from channels where category_id = $1",
            projects.id.as_uuid()
        )
        .await,
        0
    );
    assert_eq!(
        events_of(&server, &plain, &mut of_plain).await,
        [Event::CategoryDeleted {
            workspace_id: home.id,
            category_id: projects.id
        }]
    );
    let entries = workspace_audit(&server, &owner, home.id).await;
    assert!(
        entries.contains(&AuditEvent::CategoryDeleted {
            category_id: projects.id,
            name: "Projects".to_owned()
        }),
        "{entries:?}"
    );
    server
        .delete(&manager, &path(home.id, projects.id))
        .await
        .error(ErrorCode::NotFound);
}

async fn access_token(server: &TestServer, creator: &Member) -> String {
    let created: junto_protocol::access_token::AccessTokenCreated = server
        .post(
            creator,
            "/api/v1/access-tokens",
            &json!({
                "name": "setup",
                "actions": ["read_structure", "manage_structure"],
                "expires_at": (server.clock.now() + time::Duration::days(1))
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
            }),
        )
        .await
        .ok(StatusCode::CREATED);
    created.token.expose().to_owned()
}

#[tokio::test]
async fn rule_d07_who_deletes_a_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let plain = server.join(&owner, "Plain").await;
    let creator = server.join(&owner, "Creator").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateWorkspace])
        .await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let simple = workspace(&server, &creator, "Simple").await;
    let records = workspace(&server, &owner, "Records").await;
    let channel = server.channel(&owner, records.id, "records").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({ "permanent_messages": true }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    send(&server, &owner, &channel, "a record").await;

    // A member of the workspace who created it does not delete it.
    for refused in [&plain, &creator] {
        for id in [simple.id, records.id, WorkspaceId::generate()] {
            delete_workspace(&server, refused, id)
                .await
                .error(ErrorCode::Forbidden);
        }
    }
    delete_workspace(&server, &administrator, WorkspaceId::generate())
        .await
        .error(ErrorCode::NotFound);
    for caller in [&administrator, &owner] {
        delete_workspace(&server, caller, home.id)
            .await
            .error(ErrorCode::Conflict);
    }
    delete_workspace(&server, &administrator, records.id)
        .await
        .error(ErrorCode::OwnerOnly);
    let token = access_token(&server, &owner).await;
    server
        .send::<Value>(
            reqwest::Method::DELETE,
            &format!("/api/v1/workspaces/{}", records.id),
            Some(&token),
            None,
            None,
        )
        .await
        .error(ErrorCode::OwnerOnly);
    let all: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(all.len(), 3);

    delete_workspace(&server, &administrator, simple.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    delete_workspace(&server, &owner, records.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    let all: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(all, [home]);
}

#[tokio::test]
async fn rule_d08_a_deleted_workspace_is_gone_for_every_route() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let closed = workspace(&server, &owner, "Closed").await;
    let ann = server.join_into(&owner, "Ann", &[closed.id]).await;
    let ben = server.join_into(&owner, "Ben", &[closed.id]).await;
    let helpers = role(&server, &owner, closed.id, &[]).await;
    assign(&server, &owner, closed.id, &ann, helpers.id).await;
    let category = server.category(&owner, closed.id, "Projects", 0).await;
    let channel = server.channel(&owner, closed.id, "plans").await;
    set_override(
        &server,
        &owner,
        channel.id,
        helpers.id,
        &[WorkspacePermission::LockRoom],
        &[],
    )
    .await;
    send(&server, &ann, &channel, "content").await;
    for (path, body) in [
        ("glossary", json!({ "terms": ["Junto"] })),
        ("notification-default", json!({ "level": "all" })),
        ("file-retention", json!({ "days": 30 })),
        ("recording-retention", json!({ "days": 30 })),
        ("archive-period", json!({ "days": 30 })),
    ] {
        let reply = server
            .put(
                &owner,
                &format!("/api/v1/workspaces/{}/{path}", closed.id),
                &body,
            )
            .await;
        assert!(reply.status.is_success(), "{path}: {}", reply.text());
    }
    // Ann and Ben share the deleted workspace only: they reach each other through it.
    let conversation: Conversation = server
        .post(
            &ann,
            "/api/v1/conversations",
            &json!({ "account_id": ben.account_id }),
        )
        .await
        .ok(StatusCode::OK);

    delete_workspace(&server, &owner, closed.id)
        .await
        .empty(StatusCode::NO_CONTENT);

    let id = closed.id;
    for reader in [&owner, &ann] {
        let listed: Vec<Workspace> = server
            .get(reader, "/api/v1/workspaces")
            .await
            .ok(StatusCode::OK);
        assert!(listed.iter().all(|workspace| workspace.id != id));
        let snapshot = server.snapshot(reader).await;
        assert!(
            snapshot
                .workspaces
                .iter()
                .all(|state| state.workspace.id != id)
        );
        assert!(!snapshot.account.workspace_ids.contains(&id));
        for path in [
            format!("/api/v1/workspaces/{id}/channels"),
            format!("/api/v1/workspaces/{id}/roles"),
            format!("/api/v1/workspaces/{id}/members"),
            format!("/api/v1/workspaces/{id}/archived-channels"),
            format!("/api/v1/workspaces/{id}/recordings"),
            format!("/api/v1/workspaces/{id}/glossary"),
            format!("/api/v1/channels/{}/messages", channel.id),
        ] {
            server.get(reader, &path).await.error(ErrorCode::NotFound);
        }
    }
    let structure: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    assert_eq!(structure.structure.workspaces.len(), 1);
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{id}"),
            &json!({ "name": "Again" }),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(
            &owner,
            &format!("/api/v1/workspaces/{id}/members/{}", ann.account_id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{id}/channels"),
            &json!({ "name": "late", "kind": "text" }),
        )
        .await
        .error(ErrorCode::NotFound);
    delete_workspace(&server, &owner, id)
        .await
        .error(ErrorCode::NotFound);

    for statement in [
        "select count(*) from workspace_members where workspace_id = $1",
        "select count(*) from workspace_roles where workspace_id = $1",
        "select count(*) from member_roles where workspace_id = $1",
        "select count(*) from channel_overrides where workspace_id = $1",
        "select count(*) from categories where workspace_id = $1",
        "select count(*) from glossary_terms where workspace_id = $1",
        "select count(*) from workspace_notification_defaults where workspace_id = $1",
        "select count(*) from workspace_file_retention where workspace_id = $1",
        "select count(*) from workspace_recording_retention where workspace_id = $1",
        "select count(*) from workspace_archive_periods where workspace_id = $1",
        "select count(*) from channels where workspace_id = $1 and deleted_at is null",
        "select count(*) from workspaces where id = $1 and deleted_at is null",
    ] {
        assert_eq!(
            count(&server, statement, id.as_uuid()).await,
            0,
            "{statement}"
        );
    }
    assert_eq!(
        count(
            &server,
            "select count(*) from categories where id = $1",
            category.id.as_uuid()
        )
        .await,
        0
    );
    // The accounts stay, and the other workspace is as it was.
    server
        .get(&ann, "/api/v1/account")
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        count(
            &server,
            "select count(*) from workspace_roles where workspace_id = $1",
            home.id.as_uuid()
        )
        .await,
        1
    );
    assert!(
        count(
            &server,
            "select count(*) from channels where workspace_id = $1 and deleted_at is null",
            home.id.as_uuid()
        )
        .await
            >= 2
    );
    // Without a common workspace, Ann and Ben no longer reach each other.
    server
        .post(
            &ann,
            "/api/v1/conversations",
            &json!({ "account_id": ben.account_id }),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .get(
            &ann,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn rule_d09_workspace_deletion_events_and_audit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let closed = workspace(&server, &owner, "Closed").await;
    let member = server.join_into(&owner, "Member", &[closed.id]).await;
    server
        .grant_workspace(
            &owner,
            &member,
            closed.id,
            &[WorkspacePermission::ViewAuditEntries],
        )
        .await;
    let plain = server.join(&owner, "Plain").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    server.channel(&owner, closed.id, "plans").await;
    assert!(
        !workspace_audit(&server, &member, closed.id)
            .await
            .is_empty()
    );
    let mut of_member = server.subscribed(&member, Level::Background).await;
    let mut of_plain = server.subscribed(&plain, Level::Full).await;
    let mut of_administrator = server.subscribed(&administrator, Level::Full).await;

    delete_workspace(&server, &owner, closed.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    let deleted = Event::WorkspaceDeleted {
        workspace_id: closed.id,
    };
    assert_eq!(
        events_of(&server, &member, &mut of_member).await,
        std::slice::from_ref(&deleted)
    );
    assert!(events_of(&server, &plain, &mut of_plain).await.is_empty());
    assert_eq!(
        events_of(&server, &administrator, &mut of_administrator).await,
        [deleted]
    );

    let entries = server_audit(&server, &owner).await;
    assert!(
        entries.contains(&AuditEvent::WorkspaceDeleted {
            workspace_id: closed.id,
            name: "Closed".to_owned()
        }),
        "{entries:?}"
    );
    // The entries of the deleted workspace stay for the administrators only.
    let kept = workspace_audit(&server, &administrator, closed.id).await;
    assert!(
        kept.iter()
            .any(|event| matches!(event, AuditEvent::ChannelCreated { .. })),
        "{kept:?}"
    );
    for reader in [&member, &plain] {
        server
            .get(
                reader,
                &format!("/api/v1/workspaces/{}/audit-log", closed.id),
            )
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn rule_d10_invites_of_a_deleted_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let closed = workspace(&server, &owner, "Closed").await;
    let both: InviteCreated = server
        .invite(
            &owner,
            json!({ "workspace_ids": [closed.id, home.id], "max_uses": 5 }),
        )
        .await;
    let only: InviteCreated = server
        .invite(
            &owner,
            json!({ "workspace_ids": [closed.id], "max_uses": 5 }),
        )
        .await;
    let existing = server.join(&owner, "Existing").await;
    let waiting = DeviceKey::generate();
    let proof = server
        .proof(
            &waiting,
            junto_protocol::auth::ChallengeOperation::RequestToJoin,
        )
        .await;
    let request: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Waiting" }),
        )
        .await
        .ok(StatusCode::CREATED);

    delete_workspace(&server, &owner, closed.id)
        .await
        .empty(StatusCode::NO_CONTENT);

    let key = DeviceKey::generate();
    let joined: junto_protocol::auth::SessionCreated = server
        .join_reply(&key, both.code.expose(), "Joined")
        .await
        .ok(StatusCode::CREATED);
    let joined = crate::support::member(key, joined);
    let account: Value = server
        .get(&joined, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account["workspace_ids"], json!([home.id]));

    server
        .join_reply(&DeviceKey::generate(), only.code.expose(), "Refused")
        .await
        .error(ErrorCode::InviteRevoked);
    server
        .post(
            &existing,
            "/api/v1/invites/redemption",
            &json!({ "invite_code": only.code.expose() }),
        )
        .await
        .error(ErrorCode::InviteRevoked);
    let invites: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    let of = |id| invites.iter().find(|invite| invite.id == id).unwrap();
    assert_eq!(of(both.invite.id).workspace_ids, [home.id]);
    assert_eq!(of(both.invite.id).uses, 1);
    assert!(of(only.invite.id).workspace_ids.is_empty());
    assert_eq!(of(only.invite.id).uses, 0);

    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({ "workspace_ids": [closed.id] }),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &owner,
            "/api/v1/invites",
            &json!({ "workspace_ids": [closed.id] }),
        )
        .await
        .error(ErrorCode::NotFound);
}

/// The rows that a channel or a workspace holds outside the structure, per table.
async fn content(server: &TestServer, channel_id: ChannelId) -> Vec<(&'static str, i64)> {
    let mut rows = Vec::new();
    for (table, statement) in [
        (
            "messages",
            "select count(*) from messages where place_id = $1",
        ),
        (
            "message_reactions",
            "select count(*) from message_reactions r join messages m on m.id = r.message_id
             where m.place_id = $1",
        ),
        (
            "message_pins",
            "select count(*) from message_pins where channel_id = $1",
        ),
        (
            "message_mentions",
            "select count(*) from message_mentions where place_id = $1",
        ),
        (
            "message_mention_accounts",
            "select count(*) from message_mention_accounts where place_id = $1",
        ),
        (
            "notifications",
            "select count(*) from notifications n join messages m on m.id = n.message_id
             where m.place_id = $1",
        ),
        (
            "message_reads",
            "select count(*) from message_reads where place_id = $1",
        ),
        (
            "thread_reads",
            "select count(*) from thread_reads where place_id = $1",
        ),
        (
            "notification_settings",
            "select count(*) from notification_settings where scope_id = $1",
        ),
        (
            "attachments",
            "select count(*) from attachments where place_id = $1 and removed_at is null",
        ),
        (
            "call_notes",
            "select count(*) from call_notes where channel_id = $1",
        ),
        (
            "call_message_privacy",
            "select count(*) from call_message_privacy where channel_id = $1",
        ),
        (
            "recordings",
            "select count(*) from recordings where channel_id = $1",
        ),
        (
            "exports",
            "select count(*) from exports where place_id = $1 and removed_at is null",
        ),
    ] {
        rows.push((table, count(server, statement, channel_id.as_uuid()).await));
    }
    rows
}

/// Runs `work`, and calls `observe` each time one of its transactions is about to commit:
/// the database then shows the state after the transactions before it.
async fn observed<T>(
    server: &TestServer,
    work: impl Future<Output = T>,
    mut observe: impl AsyncFnMut(),
) -> T {
    tokio::pin!(work);
    let output = loop {
        server.gate().arm(0);
        tokio::select! {
            biased;
            () = server.gate().arrived() => {
                observe().await;
                server.gate().release();
            }
            output = &mut work => break output,
        }
    };
    // No later transaction is held.
    server.gate().arm(usize::MAX - 1);
    output
}

#[tokio::test]
async fn rule_d11_the_content_is_removed_in_bounded_parts() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let closed = workspace(&server, &owner, "Closed").await;
    let member = server
        .join_into(&owner, "Member", &[closed.id, home.id])
        .await;
    let channel = server.channel(&owner, closed.id, "plans").await;
    let sibling = server.channel(&owner, home.id, "sibling").await;
    let batch = i64::from(server.state.config.limits.maintenance_batch);
    let filled = 2 * batch + 200;

    for place in [&channel, &sibling] {
        let root = send(&server, &owner, place, "root").await;
        server
            .post(
                &member,
                &format!("/api/v1/messages/{}/thread", root.id),
                &json!({"document":{"blocks":[{"kind":"paragraph","content":[
                    {"kind":"account","account_id": owner.account_id}]}]}}),
            )
            .await
            .ok::<junto_protocol::message::Message>(StatusCode::CREATED);
        server
            .put_empty(&owner, &format!("/api/v1/messages/{}/pin", root.id))
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .put(
                &member,
                &format!("/api/v1/messages/{}/reaction", root.id),
                &json!({ "emoji": "👍" }),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .put(
                &member,
                &format!("/api/v1/notification-settings/{}", place.id),
                &json!({ "level": "all" }),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        // More messages than two batches, each a reply to the message before it, so that a
        // deleted message has a row that refers to it.
        sqlx::query(
            "with filled as (
                 insert into messages (id, place_id, position, author_id, created_at, document)
                 select gen_random_uuid(), $1, 1000 + n, $2, $3,
                        '{\"blocks\":[]}'::jsonb
                 from generate_series(1, $4) as n
                 returning id, position)
             update messages m set reply_to = previous.id
             from filled, filled previous
             where m.id = filled.id and previous.position = filled.position - 1",
        )
        .bind(place.id)
        .bind(owner.account_id)
        .bind(server.clock.now())
        .bind(filled)
        .execute(&server.db)
        .await
        .unwrap();
        sqlx::query(
            "insert into attachments (id, storage_id, place_id, owner_id, name, bytes, sha256,
                                      expires_at, touched_at)
             select id, id, $1, $2, 'file', 1, repeat('0', 64), $3, $3
             from (select gen_random_uuid() as id from generate_series(1, $4)) new",
        )
        .bind(place.id)
        .bind(owner.account_id)
        .bind(server.clock.now())
        .bind(batch + 10)
        .execute(&server.db)
        .await
        .unwrap();
    }
    server
        .put(
            &member,
            &format!("/api/v1/notification-settings/{}", closed.id),
            &json!({ "level": "all" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let before_sibling = content(&server, sibling.id).await;
    let before = content(&server, channel.id).await;
    let messages = |rows: &[(&str, i64)]| rows[0].1;
    assert!(messages(&before) > 2 * batch);
    for (table, rows) in &before {
        let empty = [
            "call_notes",
            "call_message_privacy",
            "recordings",
            "exports",
        ];
        assert_eq!(*rows == 0, empty.contains(table), "{table}");
    }

    delete_workspace(&server, &owner, closed.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    // The request removed the structure. The content waits for the purge.
    assert_eq!(content(&server, channel.id).await, before);
    assert_eq!(server.count("purges").await, 2);

    let mut seen: Vec<i64> = vec![messages(&before)];
    let mut live: Vec<i64> = vec![before[9].1];
    let finished = observed(
        &server,
        junto_server::purge::run_once(&server.state),
        async || {
            let now = content(&server, channel.id).await;
            seen.push(now[0].1);
            live.push(now[9].1);
        },
    )
    .await
    .unwrap();
    assert_eq!(finished, 2);
    for counts in [&seen, &live] {
        let largest = counts
            .windows(2)
            .map(|pair| pair[0] - pair[1])
            .max()
            .unwrap();
        assert!(largest <= batch, "{counts:?}");
        assert!(counts.windows(2).filter(|pair| pair[0] > pair[1]).count() >= 2);
    }

    for (table, rows) in content(&server, channel.id).await {
        assert_eq!(rows, 0, "{table}");
    }
    assert_eq!(
        count(
            &server,
            "select count(*) from notification_settings where scope_id = $1",
            closed.id.as_uuid()
        )
        .await,
        0
    );
    assert_eq!(server.count("purges").await, 0);
    assert_eq!(
        count(
            &server,
            "select permanent_message_count from channels where id = $1",
            channel.id.as_uuid()
        )
        .await,
        0
    );
    // Nothing of the other workspace was removed.
    assert_eq!(content(&server, sibling.id).await, before_sibling);
    server
        .get(
            &member,
            &format!("/api/v1/channels/{}/messages", sibling.id),
        )
        .await
        .ok::<junto_protocol::message::MessagePage>(StatusCode::OK);
}

#[tokio::test]
async fn rule_d12_the_purge_waits_for_a_running_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let voice = &scene.voice;
    let recording = studio.record().await;
    let directory = server.recordings_dir().join(recording.id.to_string());
    assert!(directory.exists());

    server
        .delete(owner, &format!("/api/v1/channels/{}", voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    // The call and its recording still run: the purge leaves the channel.
    assert_eq!(
        junto_server::purge::run_once(&server.state).await.unwrap(),
        0
    );
    assert_eq!(server.count("recordings").await, 1);
    assert_eq!(server.count("purges").await, 1);

    // The check ends the call of the deleted channel, the recording ends with it, and its
    // files are processed.
    server.check().await;
    for _ in 0..3 {
        server.tend().await;
    }
    server.process().await;
    assert_eq!(server.count("calls").await, 0);
    server.clock.advance(time::Duration::minutes(1));
    assert_eq!(
        junto_server::purge::run_once(&server.state).await.unwrap(),
        1
    );
    assert_eq!(server.count("recordings").await, 0);
    assert_eq!(server.count("recording_segments").await, 0);
    assert_eq!(server.count("storage_reservations").await, 0);
    assert_eq!(server.count("purges").await, 0);
    server.process().await;
    assert!(!directory.exists());
    assert_eq!(server.count("file_removals").await, 0);
}

#[tokio::test]
async fn rule_d13_a_deleted_channel_loses_its_recordings() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = &scene.voice;
    let recording = studio.finished().await;
    let recording_id = recording.recording.id;
    let directory = server.recordings_dir().join(recording_id.to_string());
    assert!(directory.exists());
    let other = server.voice(owner, scene.workspace_id, "other").await;
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server.check().await;

    server
        .delete(owner, &format!("/api/v1/channels/{}", voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        junto_server::purge::run_once(&server.state).await.unwrap(),
        1
    );
    for (table, rows) in content(server, voice.id).await {
        assert_eq!(rows, 0, "{table}");
    }
    assert_eq!(server.count("recordings").await, 0);
    assert_eq!(server.count("transcripts").await, 0);
    server.process().await;
    assert!(!directory.exists());
    // The channel beside it keeps its row and takes a call.
    server.enter_call(ann, other.id).await;
}

fn uses_index(plan: &Value, index: &str) -> bool {
    match plan {
        Value::Object(node) => {
            node.get("Index Name").and_then(Value::as_str) == Some(index)
                || node.values().any(|child| uses_index(child, index))
        }
        Value::Array(nodes) => nodes.iter().any(|child| uses_index(child, index)),
        _ => false,
    }
}

fn sequential_scans(plan: &Value, found: &mut Vec<String>) {
    match plan {
        Value::Object(node) => {
            if node.get("Node Type").and_then(Value::as_str) == Some("Seq Scan") {
                let table = node.get("Relation Name").and_then(Value::as_str);
                found.push(table.unwrap_or("?").to_owned());
            }
            node.values()
                .for_each(|child| sequential_scans(child, found));
        }
        Value::Array(nodes) => nodes
            .iter()
            .for_each(|child| sequential_scans(child, found)),
        _ => {}
    }
}

#[tokio::test]
async fn rule_d14_the_statements_of_the_purge_read_through_indexes() {
    let files = Files::open().await;
    let server = &files.server;
    let (owner, channel) = (&files.owner, &files.channel);
    send(server, owner, channel, "one").await;
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan = off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let now = server.clock.now();
    let explain = |statement: &str| format!("explain (format json) {statement}");
    let expected = [
        ("attachments", "attachments_live_of_place"),
        ("reply references", "messages_answers_of_place"),
        ("replies", "messages_replies_of_place"),
        ("messages", "messages_history"),
        ("read positions", "message_reads_pkey"),
        ("notification settings", "notification_settings_scope"),
        ("call notes", "call_notes_channel"),
        ("private call members", "call_message_privacy_channel"),
        ("private call lines", "call_message_privacy_channel"),
    ];
    assert_eq!(expected.len(), junto_server::purge::CHANNEL_STEPS.len());
    for ((name, statement), (expected_name, index)) in
        junto_server::purge::CHANNEL_STEPS.iter().zip(expected)
    {
        assert_eq!(*name, expected_name);
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(channel.id.as_uuid())
            .bind(500_i64)
            .bind(now)
            .fetch_one(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(uses_index(&plan, index), "{name}: {plan}");
        let mut scans = Vec::new();
        sequential_scans(&plan, &mut scans);
        assert!(scans.is_empty(), "{name}: {scans:?} {plan}");
    }
    for (statement, index, binds_time) in [
        (
            junto_server::purge::RECORDINGS_STEP,
            "recordings_of_channel",
            false,
        ),
        (junto_server::purge::EXPORTS_STEP, "exports_of_place", true),
    ] {
        let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(channel.id.as_uuid())
            .bind(500_i64);
        if binds_time {
            query = query.bind(now);
        }
        let plan: Value = query.fetch_one(&mut *connection).await.unwrap();
        assert!(uses_index(&plan, index), "{plan}");
        let mut scans = Vec::new();
        sequential_scans(&plan, &mut scans);
        assert!(scans.is_empty(), "{scans:?} {plan}");
    }
    // The rows of many accounts for the messages of one page, and the next page.
    let dependents = [
        "thread_reads_pkey",
        "notifications_message",
        "message_mention_accounts_message",
        "message_mentions_notified_pkey",
        "message_mentions_message",
        "message_reactions_pkey",
    ];
    assert_eq!(
        dependents.len(),
        junto_server::purge::MESSAGE_DEPENDENTS.len()
    );
    for ((table, column), index) in junto_server::purge::MESSAGE_DEPENDENTS
        .iter()
        .zip(dependents)
    {
        let statement = junto_server::purge::dependents_statement(table, column);
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(&statement)))
            .bind(vec![uuid::Uuid::nil(); 500])
            .bind(500_i64)
            .fetch_one(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("{table}: {error}"));
        assert!(uses_index(&plan, index), "{table}: {plan}");
        let mut scans = Vec::new();
        sequential_scans(&plan, &mut scans);
        assert!(scans.is_empty(), "{table}: {scans:?} {plan}");
    }
    let page = junto_server::purge::MESSAGE_PAGE;
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(page)))
        .bind(channel.id.as_uuid())
        .bind(0_i64)
        .bind(uuid::Uuid::nil())
        .bind(500_i64)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert!(uses_index(&plan, "messages_history"), "{plan}");
    let transcript = [
        ("speaker names", "transcript_speaker_names_of_speaker"),
        ("transcript corrections", "transcript_corrections_of_line"),
        ("transcript lines", "transcript_lines_pkey"),
    ];
    for ((name, statement), (expected_name, index)) in
        junto_server::purge::TRANSCRIPT_STEPS.iter().zip(transcript)
    {
        assert_eq!(*name, expected_name);
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(uuid::Uuid::nil())
            .bind(500_i64)
            .fetch_one(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(uses_index(&plan, index), "{name}: {plan}");
        let mut scans = Vec::new();
        sequential_scans(&plan, &mut scans);
        assert!(scans.is_empty(), "{name}: {scans:?} {plan}");
    }
    // The rows that refer to a deleted message, to a removed recording and to a deleted
    // role or category are found through an index as well.
    for (statement, index) in [
        (
            "update messages set reply_to = null where reply_to = $1",
            "messages_reply_to",
        ),
        (
            "update messages set recording_id = null where recording_id = $1",
            "messages_recording",
        ),
        (
            "delete from channel_overrides where role_id = $1",
            "channel_overrides_role",
        ),
        (
            "delete from account_server_roles where role_id = $1",
            "account_server_roles_role",
        ),
        (
            "update channels set category_id = null where category_id = $1",
            "channels_category",
        ),
        (
            "delete from thread_forks where channel_id = $1",
            "thread_forks_channel",
        ),
    ] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(uuid::Uuid::nil())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        assert!(uses_index(&plan, index), "{statement}: {plan}");
    }
}

#[tokio::test]
async fn rule_d15_a_message_beside_a_deletion_leaves_no_row() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let closed = workspace(&server, &owner, "Closed").await;
    let member = server.join_into(&owner, "Member", &[closed.id]).await;
    let channel = server.channel(&owner, closed.id, "plans").await;

    // The deletion is held before its commit, and a message is sent meanwhile. Every
    // transaction is serializable: the message commits first and is removed with the
    // content, or it runs again after the deletion and finds no channel.
    server.gate().arm(0);
    let mut deletion = Box::pin(delete_workspace(&server, &owner, closed.id));
    crate::support::drive_until(&mut deletion, server.gate().arrived()).await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let sent = async {
        let reply = server.post(&member, &path, &text("beside")).await;
        (reply.status, reply.text())
    };
    let release = async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        server.gate().release();
    };
    let (reply, (status, body), ()) = tokio::join!(&mut deletion, sent, release);
    reply.empty(StatusCode::NO_CONTENT);
    assert!(
        status == StatusCode::CREATED || status == StatusCode::NOT_FOUND,
        "{status} {body}"
    );
    junto_server::purge::run_once(&server.state).await.unwrap();
    for (table, rows) in content(&server, channel.id).await {
        assert_eq!(rows, 0, "{table}");
    }
    server
        .post(&member, &path, &text("after"))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn rule_d16_the_rows_of_many_accounts_and_a_long_transcript_are_removed_in_bounded_parts() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = &scene.voice;
    let recording_id = studio.finished().await.recording.id;
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server.check().await;
    let batch = i64::from(server.state.config.limits.maintenance_batch);
    let many = batch + 50;
    let now = server.clock.now();

    // One message with a reaction, a notification and a followed thread for each of more
    // accounts than one batch and with more answers than one batch, a call with more
    // people than one batch who read its lines, and a transcript with more lines and more
    // names of a speaker than one batch.
    let root = send(server, owner, voice, "root").await;
    sqlx::query(
        "with people as (
             insert into accounts (id, name, status, created_at)
             select gen_random_uuid(), 'filler', 'active', $2 from generate_series(1, $3)
             returning id),
         reacted as (
             insert into message_reactions (message_id, account_id, emoji)
             select $1, id, 'x' from people),
         followed as (
             insert into thread_reads (root_id, account_id, following)
             select $1, id, true from people)
         insert into notifications (id, account_id, message_id, kind, created_at)
         select gen_random_uuid(), id, $1, 'message', $2 from people",
    )
    .bind(root.id)
    .bind(now)
    .bind(many)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into transcripts (recording_id, state, requested_at, finished_at, lines)
         values ($1, 'ready', $2, $2, $3)
         on conflict (recording_id) do nothing",
    )
    .bind(recording_id)
    .bind(now)
    .bind(i32::try_from(many).unwrap())
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into transcript_lines (recording_id, line, start_ms, end_ms, segment_id, source,
                                       account_id, text, language)
         select $1, n, n, n,
                (select id from recording_segments where recording_id = $1 limit 1),
                'microphone', $2, 'line', 'en'
         from generate_series(0, $3 - 1) as n",
    )
    .bind(recording_id)
    .bind(owner.account_id)
    .bind(many)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "with speaker as (
             insert into transcript_speakers (recording_id, speaker, account_id)
             values ($1, 1, $2) on conflict do nothing)
         insert into transcript_speaker_names (id, recording_id, speaker, name, renamed_by,
                                               renamed_at)
         select gen_random_uuid(), $1, 1, 'name', $2, $3 from generate_series(1, $4)",
    )
    .bind(recording_id)
    .bind(owner.account_id)
    .bind(now)
    .bind(many)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into messages (id, place_id, position, author_id, created_at, document,
                               reply_to)
         select gen_random_uuid(), $1, 100000 + n, $2, $3, '{\"blocks\":[]}'::jsonb, $4
         from generate_series(1, $5) as n",
    )
    .bind(voice.id)
    .bind(owner.account_id)
    .bind(now)
    .bind(root.id)
    .bind(many)
    .execute(&server.db)
    .await
    .unwrap();
    let call_id: uuid::Uuid = sqlx::query_scalar(
        "select call_id from call_message_privacy where channel_id = $1 limit 1",
    )
    .bind(voice.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into call_message_members (call_id, account_id)
         select $1, id from accounts where name = 'filler'",
    )
    .bind(call_id)
    .execute(&server.db)
    .await
    .unwrap();

    let counts = || async {
        let mut found = Vec::new();
        for (statement, id) in [
            (
                "select count(*) from message_reactions where message_id = $1",
                root.id.as_uuid(),
            ),
            (
                "select count(*) from thread_reads where root_id = $1",
                root.id.as_uuid(),
            ),
            (
                "select count(*) from notifications where message_id = $1",
                root.id.as_uuid(),
            ),
            (
                "select count(*) from transcript_lines where recording_id = $1",
                recording_id.as_uuid(),
            ),
            (
                "select count(*) from transcript_speaker_names where recording_id = $1",
                recording_id.as_uuid(),
            ),
            (
                "select count(*) from messages where reply_to = $1",
                root.id.as_uuid(),
            ),
            (
                "select count(*) from call_message_members where call_id = $1
                 and account_id in (select id from accounts where name = 'filler')",
                call_id,
            ),
        ] {
            found.push(count(server, statement, id).await);
        }
        found
    };
    assert_eq!(counts().await, vec![many; 7]);

    server
        .delete(owner, &format!("/api/v1/channels/{}", voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut seen = vec![counts().await];
    let finished = observed(
        server,
        junto_server::purge::run_once(&server.state),
        async || seen.push(counts().await),
    )
    .await
    .unwrap();
    assert_eq!(finished, 1);
    seen.push(counts().await);
    for table in 0..7 {
        let steps: Vec<i64> = seen
            .windows(2)
            .map(|pair| pair[0][table] - pair[1][table])
            .collect();
        assert!(
            steps.iter().all(|step| *step <= batch),
            "{table}: {steps:?}"
        );
        assert!(
            steps.iter().filter(|step| **step > 0).count() >= 2,
            "{table}: {steps:?}"
        );
    }
    assert_eq!(counts().await, vec![0; 7]);
    assert_eq!(server.count("recordings").await, 0);
    assert_eq!(server.count("messages").await, 0);
}

/// ADR 0017: a channel that was deleted before the purge existed kept its recordings. The
/// migration that adds the purge puts each such channel into its queue.
#[tokio::test]
async fn rule_d17_a_channel_that_was_deleted_before_the_purge_existed_is_purged() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let voice = &scene.voice;
    let recording_id = studio.finished().await.recording.id;
    let directory = server.recordings_dir().join(recording_id.to_string());
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server.check().await;
    // The deletion of the earlier version: the channel is marked, and nothing is queued.
    sqlx::query("update channels set deleted_at = $2 where id = $1")
        .bind(voice.id)
        .bind(server.clock.now())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        junto_server::purge::run_once(&server.state).await.unwrap(),
        0
    );
    assert_eq!(server.count("recordings").await, 1);

    let migration = include_str!("../../migrations/0010_archive_export_deletions.sql");
    let start = migration.find("insert into purges").unwrap();
    let end = start + migration[start..].find(';').unwrap();
    sqlx::query(sqlx::AssertSqlSafe(migration[start..end].to_owned()))
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(server.count("purges").await, 1);
    assert_eq!(
        junto_server::purge::run_once(&server.state).await.unwrap(),
        1
    );
    assert_eq!(server.count("recordings").await, 0);
    server.process().await;
    assert!(!directory.exists());
}
