use junto_protocol::account::{AccountStatus, AccountSummary, OwnAccount};
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::error::ErrorCode;
use junto_protocol::invite::{Invite, InviteRevocation, JoinRequest};
use junto_protocol::roles::ServerRole;
use junto_protocol::structure::{Channel, ChannelList, Workspace};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{DeviceKey, Member, TestServer};

async fn accounts(server: &TestServer, caller: &Member) -> Vec<AccountSummary> {
    server
        .get(caller, "/api/v1/accounts")
        .await
        .ok(StatusCode::OK)
}

fn status_of(accounts: &[AccountSummary], member: &Member) -> AccountStatus {
    accounts
        .iter()
        .find(|account| account.id == member.account_id)
        .unwrap()
        .status
}

#[tokio::test]
async fn a_ban_blocks_the_keys_and_cancels_the_invites_and_an_unban_restores_the_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    let guest_invite = server.invite(&guest, json!({ "max_uses": 5 })).await;
    let owner_invite = server.invite(&owner, json!({})).await;
    let ban = format!("/api/v1/accounts/{}/ban", guest.account_id);

    server
        .put_empty(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &ban)
        .await
        .error(ErrorCode::Conflict);

    server
        .get(&guest, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .challenge_reply(&guest.key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::AccountBanned);
    server
        .join_reply(&DeviceKey::generate(), guest_invite.code.expose(), "Friend")
        .await
        .error(ErrorCode::InviteRevoked);
    server
        .join_reply(&guest.key, owner_invite.code.expose(), "Guest again")
        .await
        .error(ErrorCode::KeyAlreadyRegistered);
    assert_eq!(
        status_of(&accounts(&server, &owner).await, &guest),
        AccountStatus::Banned
    );

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountBanned {
            reason: None,
            recipient_message: None,
            account_id: guest.account_id,
            cancelled_invite_ids: vec![guest_invite.invite.id],
        }
    );
    assert_eq!(log.entries[0].actor_id, Some(owner.account_id));

    server
        .delete(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.delete(&owner, &ban).await.error(ErrorCode::Conflict);
    let session = server.login(&guest.key).await;
    assert_eq!(session.account_id, guest.account_id);
    let account: OwnAccount = server
        .send::<Value>(
            Method::GET,
            "/api/v1/account",
            Some(session.token.expose()),
            None,
            None,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.workspace_ids.len(), 1);
    assert_eq!(
        account.server_permissions,
        [ServerPermission::CreateInvites]
    );

    let invites: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    let cancelled = invites
        .iter()
        .find(|invite| invite.id == guest_invite.invite.id)
        .unwrap();
    assert_eq!(cancelled.revocation, Some(InviteRevocation::CreatorBanned));
    server
        .join_reply(&DeviceKey::generate(), guest_invite.code.expose(), "Friend")
        .await
        .error(ErrorCode::InviteRevoked);
    server
        .join_reply(&DeviceKey::generate(), owner_invite.code.expose(), "Friend")
        .await
        .ok::<Value>(StatusCode::CREATED);
    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert!(log.entries.iter().any(|entry| entry.event
        == AuditEvent::AccountUnbanned {
            account_id: guest.account_id
        }));
}

#[tokio::test]
async fn a_banned_person_returns_only_as_a_new_account_with_a_new_code() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    let invite = server.invite(&owner, json!({})).await;
    let new_key = DeviceKey::generate();
    let returned: junto_protocol::auth::SessionCreated = server
        .join_reply(&new_key, invite.code.expose(), "Guest")
        .await
        .ok(StatusCode::CREATED);
    assert_ne!(returned.account_id, guest.account_id);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountJoined {
            account_id: returned.account_id,
            invite_id: invite.invite.id,
            invite_created_by: owner.account_id,
        }
    );
}

#[tokio::test]
async fn a_removal_ends_the_account_and_frees_its_key() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    server
        .grant_workspace(
            &owner,
            &guest,
            default_workspace.id,
            &[WorkspacePermission::ManageRoles],
        )
        .await;
    let removal = format!("/api/v1/accounts/{}/removal", guest.account_id);

    server
        .post(&owner, &removal, &json!({}))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&owner, &removal, &json!({}))
        .await
        .error(ErrorCode::Conflict);

    server
        .get(&guest, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .challenge_reply(&guest.key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::UnknownDevice);
    let listed = accounts(&server, &owner).await;
    assert_eq!(status_of(&listed, &guest), AccountStatus::Removed);
    let summary = listed
        .iter()
        .find(|account| account.id == guest.account_id)
        .unwrap();
    assert!(summary.server_role_ids.is_empty());
    for table in ["workspace_members", "member_roles", "devices"] {
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select count(*) from {table} where account_id = $1"
        )))
        .bind(guest.account_id.as_uuid())
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .error(ErrorCode::Conflict);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountRemoved {
            reason: None,
            recipient_message: None,
            account_id: guest.account_id
        }
    );

    let invite = server.invite(&owner, json!({})).await;
    let returned: junto_protocol::auth::SessionCreated = server
        .join_reply(&guest.key, invite.code.expose(), "Guest")
        .await
        .ok(StatusCode::CREATED);
    assert_ne!(returned.account_id, guest.account_id);
}

#[tokio::test]
async fn the_owner_and_the_caller_cannot_be_removed_or_banned() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;

    for (caller, target, code) in [
        (&administrator, &owner, ErrorCode::OwnerProtected),
        (&administrator, &administrator, ErrorCode::OwnAccountTarget),
        (&owner, &owner, ErrorCode::OwnerProtected),
    ] {
        server
            .put_empty(
                caller,
                &format!("/api/v1/accounts/{}/ban", target.account_id),
            )
            .await
            .error(code);
        server
            .post(
                caller,
                &format!("/api/v1/accounts/{}/removal", target.account_id),
                &json!({}),
            )
            .await
            .error(code);
    }
    let listed = accounts(&server, &owner).await;
    assert!(
        listed
            .iter()
            .all(|account| account.status == AccountStatus::Active)
    );
    assert!(listed.iter().any(|account| account.is_owner));

    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", uuid::Uuid::now_v7()),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn a_member_without_server_permissions_cannot_do_any_server_level_action() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let other = server.join(&owner, "Other").await;
    let default_workspace = server.default_workspace(&owner).await;
    let invite = server.invite(&owner, json!({})).await;
    let role: ServerRole = server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({ "name": "inviters" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    let request: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Newcomer" }),
        )
        .await
        .ok(StatusCode::CREATED);

    let other_id = other.account_id;
    let attempts: Vec<(Method, String, Value)> = vec![
        (Method::POST, "/api/v1/invites".into(), json!({})),
        (Method::GET, "/api/v1/invites".into(), Value::Null),
        (
            Method::DELETE,
            format!("/api/v1/invites/{}", invite.invite.id),
            Value::Null,
        ),
        (Method::GET, "/api/v1/join-requests".into(), Value::Null),
        (
            Method::POST,
            format!("/api/v1/join-requests/{}/approval", request.id),
            json!({}),
        ),
        (
            Method::POST,
            format!("/api/v1/join-requests/{}/denial", request.id),
            json!({}),
        ),
        (Method::GET, "/api/v1/accounts".into(), Value::Null),
        (
            Method::POST,
            format!("/api/v1/accounts/{other_id}/removal"),
            json!({}),
        ),
        (
            Method::PUT,
            format!("/api/v1/accounts/{other_id}/ban"),
            Value::Null,
        ),
        (
            Method::DELETE,
            format!("/api/v1/accounts/{other_id}/ban"),
            Value::Null,
        ),
        (Method::GET, "/api/v1/server-roles".into(), Value::Null),
        (
            Method::POST,
            "/api/v1/server-roles".into(),
            json!({ "name": "mine" }),
        ),
        (
            Method::PATCH,
            format!("/api/v1/server-roles/{}", role.id),
            json!({ "permissions": ["administrator"] }),
        ),
        (
            Method::PUT,
            format!(
                "/api/v1/accounts/{}/server-roles/{}",
                plain.account_id, role.id
            ),
            Value::Null,
        ),
        (
            Method::DELETE,
            format!("/api/v1/accounts/{other_id}/server-roles/{}", role.id),
            Value::Null,
        ),
        (
            Method::POST,
            "/api/v1/workspaces".into(),
            json!({ "name": "Mine" }),
        ),
        (
            Method::PATCH,
            format!("/api/v1/workspaces/{}", default_workspace.id),
            json!({ "name": "Mine" }),
        ),
        (Method::GET, "/api/v1/audit-log".into(), Value::Null),
    ];
    for (method, path, body) in attempts {
        let body = (!body.is_null()).then_some(&body);
        let reply = server
            .send(method.clone(), &path, Some(&plain.token), body, None)
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{method} {path}: {}",
            reply.text()
        );
        reply.error(ErrorCode::Forbidden);
    }
    server
        .post(
            &plain,
            &format!("/api/v1/accounts/{other_id}/recovery"),
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .error(ErrorCode::OwnerOnly);

    let after: Vec<AccountSummary> = accounts(&server, &owner).await;
    assert!(
        after
            .iter()
            .all(|account| account.status == AccountStatus::Active)
    );
    assert!(
        after
            .iter()
            .all(|account| account.server_role_ids.is_empty())
    );
    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "Acme");
    let invites: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert!(invites.iter().all(|invite| invite.revocation.is_none()));
}

#[tokio::test]
async fn each_server_permission_opens_its_own_action_and_no_other() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let target = server.join(&owner, "Target").await;

    let probes: [(ServerPermission, Method, &str); 5] = [
        (
            ServerPermission::CreateInvites,
            Method::GET,
            "/api/v1/invites",
        ),
        (
            ServerPermission::ApproveJoinRequests,
            Method::GET,
            "/api/v1/join-requests",
        ),
        (
            ServerPermission::BanMembers,
            Method::GET,
            "/api/v1/accounts",
        ),
        (
            ServerPermission::ViewServerAuditLog,
            Method::GET,
            "/api/v1/audit-log",
        ),
        (
            ServerPermission::Administrator,
            Method::GET,
            "/api/v1/server-roles",
        ),
    ];
    for (held, _, _) in &probes {
        let holder = server.join(&owner, "Holder").await;
        server.grant_server(&owner, &holder, &[*held]).await;
        for (needed, method, path) in &probes {
            let reply = server
                .send::<Value>(method.clone(), path, Some(&holder.token), None, None)
                .await;
            let expected = if held == needed
                || *held == ServerPermission::Administrator
                || (*held == ServerPermission::BanMembers && *path == "/api/v1/server-roles")
            {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            };
            assert_eq!(reply.status, expected, "{held:?} on {path}");
        }
    }

    let creator = server.join(&owner, "Creator").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateWorkspace])
        .await;
    server
        .post(&creator, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok::<Workspace>(StatusCode::CREATED);
    server
        .put_empty(
            &creator,
            &format!("/api/v1/accounts/{}/ban", target.account_id),
        )
        .await
        .error(ErrorCode::Forbidden);

    let moderator = server.join(&owner, "Moderator").await;
    server
        .grant_server(&owner, &moderator, &[ServerPermission::BanMembers])
        .await;
    server
        .put_empty(
            &moderator,
            &format!("/api/v1/accounts/{}/ban", target.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn an_administrator_cannot_do_owner_only_actions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, ServerPermission::ALL)
        .await;
    server
        .grant_workspace(
            &owner,
            &administrator,
            default_workspace.id,
            WorkspacePermission::ALL,
        )
        .await;
    let guest = server.join(&owner, "Guest").await;

    server
        .post(
            &administrator,
            &format!("/api/v1/accounts/{}/recovery", guest.account_id),
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .error(ErrorCode::OwnerOnly);

    let channel: Channel = server
        .post(
            &administrator,
            &format!("/api/v1/workspaces/{}/channels", default_workspace.id),
            &json!({ "name": "records", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("update channels set permanent_message_count = 1 where id = $1")
        .bind(channel.id.as_uuid())
        .execute(&server.db)
        .await
        .unwrap();
    let path = format!("/api/v1/channels/{}", channel.id);
    server
        .delete(&administrator, &path)
        .await
        .error(ErrorCode::OwnerOnly);
    let listed: ChannelList = server
        .get(
            &administrator,
            &format!("/api/v1/workspaces/{}/channels", default_workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(listed.channels.iter().any(|listed| listed.id == channel.id));

    server
        .delete(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);

    let plain: Channel = server
        .post(
            &administrator,
            &format!("/api/v1/workspaces/{}/channels", default_workspace.id),
            &json!({ "name": "notes", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .delete(&administrator, &format!("/api/v1/channels/{}", plain.id))
        .await
        .empty(StatusCode::NO_CONTENT);
}
