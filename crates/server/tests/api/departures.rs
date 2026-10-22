//! Leaving a workspace, deleting one's own account, and the return of an inactive account
//! (ADR 0121).

use junto_protocol::access_token::TokenAction;
use junto_protocol::account::{
    AccountProfile, AccountStatus, AccountSummary, Departure, ModerationNoticePage,
};
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::{ChallengeOperation, SessionCreated};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{EndReason, Event, Level, ServerMessage};
use junto_protocol::invite::{
    Invite, InviteCreated, JoinRequest, JoinRequestState, JoinRequestStatus,
};
use junto_protocol::message::{Conversation, Message};
use junto_protocol::structure::{Workspace, WorkspaceMember};
use junto_protocol::{ServerPermission, WorkspacePermission};
use junto_server::image::samples;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::messages_support::{channel, message, send, text};
use crate::support::{DeviceKey, Member, TestServer, member};

fn departure(workspace: &Workspace) -> String {
    format!("/api/v1/workspaces/{}/departure", workspace.id)
}

fn profile_path(member: &Member) -> String {
    format!("/api/v1/accounts/{}/profile", member.account_id)
}

async fn workspace(server: &TestServer, owner: &Member, name: &str) -> Workspace {
    server
        .post(owner, "/api/v1/workspaces", &json!({ "name": name }))
        .await
        .ok(StatusCode::CREATED)
}

async fn accounts(server: &TestServer, caller: &Member) -> Vec<AccountSummary> {
    server
        .get(caller, "/api/v1/accounts")
        .await
        .ok(StatusCode::OK)
}

async fn status_of(server: &TestServer, caller: &Member, account: &Member) -> AccountSummary {
    accounts(server, caller)
        .await
        .into_iter()
        .find(|listed| listed.id == account.account_id)
        .unwrap()
}

async fn members(
    server: &TestServer,
    caller: &Member,
    workspace: &Workspace,
) -> Vec<WorkspaceMember> {
    server
        .get(
            caller,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
        )
        .await
        .ok(StatusCode::OK)
}

async fn invites(server: &TestServer, caller: &Member) -> Vec<Invite> {
    server
        .get(caller, "/api/v1/invites")
        .await
        .ok(StatusCode::OK)
}

async fn server_log(server: &TestServer, owner: &Member) -> AuditPage {
    server
        .get(owner, "/api/v1/audit-log?limit=200")
        .await
        .ok(StatusCode::OK)
}

async fn workspace_log(server: &TestServer, owner: &Member, workspace: &Workspace) -> AuditPage {
    server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/audit-log?limit=200", workspace.id),
        )
        .await
        .ok(StatusCode::OK)
}

async fn rows(server: &TestServer, table: &str, account: &Member) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select count(*) from {table} where account_id = $1"
    )))
    .bind(account.account_id.as_uuid())
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn entries_naming(server: &TestServer, account: &Member) -> Vec<(uuid::Uuid, Value)> {
    sqlx::query_as(
        "select e.id, e.event from audit_entries e
         join audit_entry_targets t on t.entry_id = e.id
         where t.target_id = $1 or e.actor_id = $1
         order by e.id",
    )
    .bind(account.account_id.as_uuid())
    .fetch_all(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
async fn leaving_a_workspace_ends_the_membership_and_keeps_the_content_and_the_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let other = workspace(&server, &owner, "Other").await;
    let guest = server
        .join_into(&owner, "Guest", &[default.id, other.id])
        .await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    server
        .grant_workspace(
            &owner,
            &guest,
            default.id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let only_default: InviteCreated = server
        .invite(&guest, json!({ "workspace_ids": [default.id] }))
        .await;
    let both: InviteCreated = server
        .invite(&guest, json!({ "workspace_ids": [default.id, other.id] }))
        .await;
    let text_channel = channel(&server, &owner, "text").await;
    let written = send(&server, &guest, &text_channel, "kept words").await;

    server
        .post(&guest, &departure(&default), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&guest, &departure(&default), &Value::Null)
        .await
        .error(ErrorCode::NotFound);

    assert!(
        !members(&server, &owner, &default)
            .await
            .iter()
            .any(|listed| listed.account_id == guest.account_id)
    );
    assert_eq!(rows(&server, "member_roles", &guest).await, 0);
    let account: junto_protocol::account::OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.id, guest.account_id);
    assert_eq!(
        status_of(&server, &owner, &guest).await.status,
        AccountStatus::Active
    );
    let kept: Message = server
        .get(&owner, &message(written.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(kept.author_id, Some(guest.account_id));
    assert_eq!(kept.author_name.as_deref(), Some("Guest"));

    let listed = invites(&server, &owner).await;
    let first = listed
        .iter()
        .find(|invite| invite.id == only_default.invite.id)
        .unwrap();
    assert!(first.revocation.is_some());
    let second = listed
        .iter()
        .find(|invite| invite.id == both.invite.id)
        .unwrap();
    assert_eq!(second.revocation, None);
    assert_eq!(second.workspace_ids, vec![other.id]);

    let log = workspace_log(&server, &owner, &default).await;
    assert_eq!(
        log.entries[0].event,
        AuditEvent::WorkspaceMemberLeft {
            account_id: guest.account_id
        }
    );
    assert_eq!(log.entries[0].actor_id, Some(guest.account_id));
    let notices: ModerationNoticePage = server
        .get(&guest, "/api/v1/account/moderation-notices")
        .await
        .ok(StatusCode::OK);
    assert!(notices.notices.is_empty());
}

#[tokio::test]
async fn leaving_the_last_workspace_makes_the_account_inactive_and_an_invite_returns_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let other = workspace(&server, &owner, "Other").await;
    let guest = server.join(&owner, "Guest").await;
    let stranger = server.join_into(&owner, "Stranger", &[other.id]).await;
    let fellow = server.join(&owner, "Fellow").await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    let held: InviteCreated = server
        .invite(&guest, json!({ "workspace_ids": [default.id] }))
        .await;
    server
        .put_bytes(
            &guest,
            "/api/v1/account/avatar",
            Some("image/png"),
            samples::png(64, 48),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let mut guest_stream = server.subscribed(&guest, Level::Full).await;
    let mut stranger_stream = server.subscribed(&stranger, Level::Full).await;
    let mut fellow_stream = server.subscribed(&fellow, Level::Full).await;

    server
        .post(&guest, &departure(&default), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);

    let (messages, _) = guest_stream.until_closed().await;
    assert!(messages.iter().any(|message| matches!(
        message,
        ServerMessage::Ended {
            reason: EndReason::Inactive,
            ..
        }
    )));
    server
        .get(&guest, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .challenge_reply(&guest.key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::AccountInactive);
    let summary = status_of(&server, &owner, &guest).await;
    assert_eq!(summary.status, AccountStatus::Inactive);
    assert!(summary.server_role_ids.is_empty());
    assert_eq!(rows(&server, "devices", &guest).await, 1);
    let revoked = invites(&server, &owner)
        .await
        .into_iter()
        .find(|invite| invite.id == held.invite.id)
        .unwrap();
    assert!(revoked.revocation.is_some());
    let log = server_log(&server, &owner).await;
    assert!(log.entries.iter().any(|entry| entry.event
        == AuditEvent::AccountLeft {
            account_id: guest.account_id
        }
        && entry.actor_id == Some(guest.account_id)));

    let departed = AccountProfile {
        id: guest.account_id,
        name: "Guest".to_owned(),
        avatar: None,
        departure: Some(Departure::Inactive),
    };
    for viewer in [&owner, &stranger, &fellow] {
        let seen: AccountProfile = server
            .get(viewer, &profile_path(&guest))
            .await
            .ok(StatusCode::OK);
        assert_eq!(seen, departed);
        server
            .get(
                viewer,
                &format!("/api/v1/accounts/{}/avatar", guest.account_id),
            )
            .await
            .error(ErrorCode::NotFound);
    }
    let told = |events: &[(u64, Event)], departure: Option<Departure>| {
        events.iter().any(|(_, event)| {
            *event
                == Event::AccountDepartureChanged {
                    account_id: guest.account_id,
                    departure,
                }
        })
    };
    let stranger_events = server.traffic(&stranger, &mut stranger_stream).await;
    assert!(told(&stranger_events, Some(Departure::Inactive)));
    let fellow_events = server.traffic(&fellow, &mut fellow_stream).await;
    assert!(told(&fellow_events, Some(Departure::Inactive)));
    assert!(fellow_events.iter().any(|(_, event)| matches!(
        event,
        Event::WorkspaceMemberRemoved { account_id, .. } if *account_id == guest.account_id
    )));

    let invite = server
        .invite(&owner, json!({ "workspace_ids": [other.id] }))
        .await;
    let returned: SessionCreated = server
        .join_reply(&guest.key, invite.code.expose(), "Another name")
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(returned.account_id, guest.account_id);
    assert_eq!(returned.device_id, guest.device_id);
    let guest = member(guest.key, returned);
    let summary = status_of(&server, &owner, &guest).await;
    assert_eq!(summary.status, AccountStatus::Active);
    assert_eq!(summary.name, "Guest");
    let seen: AccountProfile = server
        .get(&stranger, &profile_path(&guest))
        .await
        .ok(StatusCode::OK);
    assert_eq!(seen.departure, None);
    assert!(seen.avatar.is_some());
    assert!(
        members(&server, &owner, &other)
            .await
            .iter()
            .any(|listed| listed.account_id == guest.account_id)
    );
    let stranger_events = server.traffic(&stranger, &mut stranger_stream).await;
    assert!(told(&stranger_events, None));
    let log = server_log(&server, &owner).await;
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountReturned {
            account_id: guest.account_id,
            invite_id: Some(invite.invite.id),
            invite_created_by: Some(owner.account_id),
            join_request_id: None,
        }
    );
    assert!(
        !log.entries
            .iter()
            .any(|entry| matches!(entry.event, AuditEvent::AccountJoined { account_id, .. } if account_id == guest.account_id && entry.id > log.entries[1].id))
    );
}

#[tokio::test]
async fn an_inactive_key_asks_to_join_and_the_approval_returns_the_same_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    server
        .post(&guest, &departure(&default), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);

    let proof = server
        .proof(&guest.key, ChallengeOperation::RequestToJoin)
        .await;
    let request: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Asked name" }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(request.returning_account_id, Some(guest.account_id));
    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed[0].returning_account_id, Some(guest.account_id));
    let decided: JoinRequestStatus = server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({ "workspace_ids": [default.id] }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(decided.state, JoinRequestState::Approved);
    assert_eq!(decided.account_id, Some(guest.account_id));
    let session = server.login(&guest.key).await;
    assert_eq!(session.account_id, guest.account_id);
    let summary = status_of(&server, &owner, &guest).await;
    assert_eq!(summary.status, AccountStatus::Active);
    assert_eq!(summary.name, "Guest");
    let log = server_log(&server, &owner).await;
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountReturned {
            account_id: guest.account_id,
            invite_id: None,
            invite_created_by: None,
            join_request_id: Some(request.id),
        }
    );
    assert_eq!(log.entries[0].actor_id, Some(owner.account_id));

    let other = server.join(&owner, "Other").await;
    let proof = server
        .proof(&other.key, ChallengeOperation::RequestToJoin)
        .await;
    server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Other" }),
        )
        .await
        .error(ErrorCode::KeyAlreadyRegistered);
}

#[tokio::test]
async fn deleting_the_account_keeps_its_content_and_audit_and_frees_its_key() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let other = workspace(&server, &owner, "Other").await;
    let guest = server
        .join_into(&owner, "Guest", &[default.id, other.id])
        .await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    server
        .grant_workspace(
            &owner,
            &guest,
            default.id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    server
        .put_bytes(
            &guest,
            "/api/v1/account/avatar",
            Some("image/png"),
            samples::png(64, 48),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let text_channel = channel(&server, &owner, "text").await;
    let written = send(&server, &guest, &text_channel, "kept words").await;
    let conversation: Conversation = server
        .post(
            &guest,
            "/api/v1/conversations",
            &json!({ "account_id": owner.account_id }),
        )
        .await
        .ok(StatusCode::OK);
    let direct_path = format!("/api/v1/conversations/{}/messages", conversation.id);
    let direct: Message = server
        .post(&guest, &direct_path, &text("a direct line"))
        .await
        .ok(StatusCode::CREATED);
    let before = entries_naming(&server, &guest).await;

    server
        .delete(&guest, "/api/v1/account")
        .await
        .empty(StatusCode::NO_CONTENT);

    server
        .get(&guest, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .challenge_reply(&guest.key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::UnknownDevice);
    let summary = status_of(&server, &owner, &guest).await;
    assert_eq!(summary.status, AccountStatus::Deleted);
    assert_eq!(summary.name, "Guest");
    assert!(summary.server_role_ids.is_empty());
    for table in [
        "workspace_members",
        "member_roles",
        "devices",
        "account_avatars",
        "account_server_roles",
    ] {
        assert_eq!(rows(&server, table, &guest).await, 0, "{table}");
    }
    let seen: AccountProfile = server
        .get(&owner, &profile_path(&guest))
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        seen,
        AccountProfile {
            id: guest.account_id,
            name: "Guest".to_owned(),
            avatar: None,
            departure: Some(Departure::Deleted),
        }
    );

    let kept: Message = server
        .get(&owner, &message(written.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(kept.author_name.as_deref(), Some("Guest"));
    let kept_direct: Message = server
        .get(&owner, &message(direct.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(kept_direct.author_id, Some(guest.account_id));
    server
        .post(&owner, &direct_path, &text("nobody reads this"))
        .await
        .error(ErrorCode::Forbidden);

    let after = entries_naming(&server, &guest).await;
    assert_eq!(after[..before.len()], before[..]);
    let log = server_log(&server, &owner).await;
    let mut left = vec![default.id, other.id];
    left.sort();
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountDeleted {
            account_id: guest.account_id,
            workspace_ids: left,
        }
    );
    for workspace in [&default, &other] {
        let log = workspace_log(&server, &owner, workspace).await;
        assert_eq!(
            log.entries[0].event,
            AuditEvent::WorkspaceMemberLeft {
                account_id: guest.account_id
            }
        );
    }

    for (method, path) in [
        (
            Method::POST,
            format!("/api/v1/accounts/{}/removal", guest.account_id),
        ),
        (
            Method::PUT,
            format!("/api/v1/accounts/{}/ban", guest.account_id),
        ),
        (
            Method::DELETE,
            format!("/api/v1/accounts/{}/ban", guest.account_id),
        ),
    ] {
        server
            .send(method, &path, Some(&owner.token), None::<&Value>, None)
            .await
            .error(ErrorCode::Conflict);
    }

    let invite = server.invite(&owner, json!({})).await;
    let again: SessionCreated = server
        .join_reply(&guest.key, invite.code.expose(), "Guest")
        .await
        .ok(StatusCode::CREATED);
    assert_ne!(again.account_id, guest.account_id);
}

#[tokio::test]
async fn the_owner_hands_over_the_server_before_leaving_it_and_an_administrator_need_not() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    server
        .delete(&owner, "/api/v1/account")
        .await
        .error(ErrorCode::OwnerProtected);
    server
        .post(&owner, &departure(&default), &Value::Null)
        .await
        .error(ErrorCode::OwnerProtected);
    let other = workspace(&server, &owner, "Other").await;
    server
        .post(&owner, &departure(&default), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&owner, &departure(&other), &Value::Null)
        .await
        .error(ErrorCode::OwnerProtected);

    let administrator = server.join_into(&owner, "Administrator", &[other.id]).await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    server
        .delete(&administrator, "/api/v1/account")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        status_of(&server, &owner, &administrator).await.status,
        AccountStatus::Deleted
    );
}

#[tokio::test]
async fn an_access_token_neither_leaves_nor_deletes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let token = server
        .access_token(
            &administrator,
            &[TokenAction::ManageMembers, TokenAction::ManageStructure],
        )
        .await;
    server
        .with_token(Method::POST, &departure(&default), &token, None)
        .await
        .error(ErrorCode::Forbidden);
    server
        .with_token(Method::DELETE, "/api/v1/account", &token, None)
        .await
        .error(ErrorCode::Forbidden);
    assert_eq!(
        status_of(&server, &owner, &administrator).await.status,
        AccountStatus::Active
    );
}

#[tokio::test]
async fn an_inactive_account_can_be_removed_and_not_banned() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    server
        .post(&guest, &departure(&default), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .error(ErrorCode::Conflict);
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", guest.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        status_of(&server, &owner, &guest).await.status,
        AccountStatus::Removed
    );
    let invite = server.invite(&owner, json!({})).await;
    let again: SessionCreated = server
        .join_reply(&guest.key, invite.code.expose(), "Guest")
        .await
        .ok(StatusCode::CREATED);
    assert_ne!(again.account_id, guest.account_id);
}

#[tokio::test]
async fn a_new_key_still_joins_as_a_new_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let invite = server.invite(&owner, json!({})).await;
    let key = DeviceKey::generate();
    let joined: SessionCreated = server
        .join_reply(&key, invite.code.expose(), "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    let newcomer = member(key, joined);
    let log = server_log(&server, &owner).await;
    assert!(matches!(
        log.entries[0].event,
        AuditEvent::AccountJoined { account_id, .. } if account_id == newcomer.account_id
    ));
}
