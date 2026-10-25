use junto_protocol::ServerPermission;
use junto_protocol::account::{Device, DeviceOrigin, OwnAccount};
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::{ChallengeOperation, SessionCreated};
use junto_protocol::error::ErrorCode;
use junto_protocol::invite::{
    Invite, InviteRevocation, JoinRequest, JoinRequestState, JoinRequestStatus, Redemption,
};
use junto_protocol::structure::{Workspace, WorkspaceMember};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;
use time::format_description::well_known::Rfc3339;

use crate::support::{DeviceKey, Member, Reply, TestServer, member};

async fn request_to_join_reply(server: &TestServer, key: &DeviceKey, name: &str) -> Reply {
    let proof = server.proof(key, ChallengeOperation::RequestToJoin).await;
    server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": name }),
        )
        .await
}

async fn status(server: &TestServer, key: &DeviceKey) -> Reply {
    let proof = server
        .proof(key, ChallengeOperation::JoinRequestStatus)
        .await;
    server
        .open("/api/v1/join-requests/status", &json!({ "proof": proof }))
        .await
}

async fn withdraw(server: &TestServer, key: &DeviceKey) -> Reply {
    let proof = server
        .proof(key, ChallengeOperation::WithdrawJoinRequest)
        .await;
    server
        .open(
            "/api/v1/join-requests/withdrawal",
            &json!({ "proof": proof }),
        )
        .await
}

async fn requests_of(server: &TestServer, key: &DeviceKey) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "select state, decided_by::text from join_requests where public_key = $1
         order by created_at, id",
    )
    .bind(key.public().as_bytes().as_slice())
    .fetch_all(&server.db)
    .await
    .unwrap()
}

async fn create_workspace(server: &TestServer, creator: &Member, name: &str) -> Workspace {
    server
        .post(creator, "/api/v1/workspaces", &json!({ "name": name }))
        .await
        .ok(StatusCode::CREATED)
}

#[tokio::test]
async fn an_invite_code_creates_an_account_at_once() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;

    let created = server.invite(&owner, json!({})).await;
    assert_eq!(created.invite.max_uses, 1);
    assert_eq!(created.invite.uses, 0);
    assert_eq!(created.invite.workspace_ids, [default_workspace.id]);
    assert_eq!(
        created.invite.expires_at - server.clock.now(),
        Duration::days(7)
    );
    assert_eq!(created.code.expose().replace('-', "").len(), 16);

    let key = DeviceKey::generate();
    let session: SessionCreated = server
        .join_reply(&key, &created.code.expose().to_lowercase(), "  Guest  ")
        .await
        .ok(StatusCode::CREATED);
    let guest = member(key, session);

    let account: OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.name, "Guest");
    assert!(!account.is_owner);
    assert!(account.server_role_ids.is_empty());
    assert_eq!(account.workspace_ids, [default_workspace.id]);

    let members: Vec<WorkspaceMember> = server
        .get(
            &guest,
            &format!("/api/v1/workspaces/{}/members", default_workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    let joined = members
        .iter()
        .find(|member| member.account_id == guest.account_id)
        .unwrap();
    assert!(joined.role_ids.is_empty());

    let devices: Vec<Device> = server
        .get(&guest, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices[0].origin, DeviceOrigin::Invite);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountJoined {
            account_id: guest.account_id,
            invite_id: created.invite.id,
            invite_created_by: owner.account_id,
        }
    );
    assert!(matches!(
        log.entries[1].event,
        AuditEvent::InviteCreated { invite_id, max_uses: 1, .. } if invite_id == created.invite.id
    ));

    let invites: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert_eq!(invites[0].uses, 1);
    let listed = serde_json::to_string(&invites).unwrap();
    assert!(!listed.contains(created.code.expose()));
}

#[tokio::test]
async fn invite_codes_are_stored_hashed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let created = server.invite(&owner, json!({})).await;
    let stored: Vec<u8> = sqlx::query_scalar("select code_hash from invites")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, created.code.expose().as_bytes());
}

#[tokio::test]
async fn an_invite_stops_at_its_expiry() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let expires_at = (server.clock.now() + Duration::hours(2))
        .format(&Rfc3339)
        .unwrap();
    let created = server
        .invite(&owner, json!({ "expires_at": expires_at, "max_uses": 5 }))
        .await;

    server.clock.advance(Duration::minutes(119));
    server
        .join_reply(&DeviceKey::generate(), created.code.expose(), "Early")
        .await
        .ok::<Value>(StatusCode::CREATED);

    server.clock.advance(Duration::minutes(2));
    server
        .join_reply(&DeviceKey::generate(), created.code.expose(), "Late")
        .await
        .error(ErrorCode::InviteExpired);
}

#[tokio::test]
async fn invite_expiry_and_uses_have_bounds() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let at = |duration: Duration| (server.clock.now() + duration).format(&Rfc3339).unwrap();
    for body in [
        json!({ "expires_at": at(Duration::seconds(-1)) }),
        json!({ "expires_at": at(Duration::days(366)) }),
        json!({ "max_uses": 0 }),
        json!({ "max_uses": 10_001 }),
        json!({ "max_uses": -1 }),
    ] {
        server
            .post(&owner, "/api/v1/invites", &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn an_invite_stops_at_its_maximum_number_of_uses() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let created = server.invite(&owner, json!({ "max_uses": 2 })).await;
    for name in ["First", "Second"] {
        server
            .join_reply(&DeviceKey::generate(), created.code.expose(), name)
            .await
            .ok::<Value>(StatusCode::CREATED);
    }
    server
        .join_reply(&DeviceKey::generate(), created.code.expose(), "Third")
        .await
        .error(ErrorCode::InviteExhausted);
}

#[tokio::test]
async fn two_simultaneous_joins_on_a_one_use_code_make_one_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    for _ in 0..5 {
        let created = server.invite(&owner, json!({ "max_uses": 1 })).await;
        let first = DeviceKey::generate();
        let second = DeviceKey::generate();
        let first_proof = server.proof(&first, ChallengeOperation::Join).await;
        let second_proof = server.proof(&second, ChallengeOperation::Join).await;
        let join = |proof, name: &'static str| {
            let server = &server;
            let code = created.code.expose().to_owned();
            async move {
                server
                    .open(
                        "/api/v1/join",
                        &json!({ "proof": proof, "invite_code": code, "name": name }),
                    )
                    .await
            }
        };
        let (first_reply, second_reply) =
            tokio::join!(join(first_proof, "First"), join(second_proof, "Second"));

        let (won, lost) = if first_reply.status == StatusCode::CREATED {
            (first_reply, second_reply)
        } else {
            (second_reply, first_reply)
        };
        won.ok::<SessionCreated>(StatusCode::CREATED);
        lost.error(ErrorCode::InviteExhausted);

        let (uses, max_uses): (i32, i32) =
            sqlx::query_as("select uses, max_uses from invites where id = $1")
                .bind(created.invite.id)
                .fetch_one(&server.db)
                .await
                .unwrap();
        assert_eq!((uses, max_uses), (1, 1));
    }
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1 + 5);
}

#[tokio::test]
async fn a_failed_join_does_not_use_up_the_invite() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let created = server.invite(&owner, json!({})).await;

    server
        .join_reply(&owner.key, created.code.expose(), "Again")
        .await
        .error(ErrorCode::KeyAlreadyRegistered);
    server
        .join_reply(&DeviceKey::generate(), created.code.expose(), "   ")
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .join_reply(&DeviceKey::generate(), created.code.expose(), "Guest")
        .await
        .ok::<Value>(StatusCode::CREATED);
}

#[tokio::test]
async fn an_unknown_or_cancelled_code_is_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    server
        .join_reply(&DeviceKey::generate(), "ZZZZ-ZZZZ-ZZZZ-ZZZZ", "Guest")
        .await
        .error(ErrorCode::InviteNotFound);

    let created = server.invite(&owner, json!({ "max_uses": 3 })).await;
    let path = format!("/api/v1/invites/{}", created.invite.id);
    server
        .delete(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .join_reply(&DeviceKey::generate(), created.code.expose(), "Guest")
        .await
        .error(ErrorCode::InviteRevoked);

    let invites: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert_eq!(invites[0].revocation, Some(InviteRevocation::Revoked));
    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    let revocations = log
        .entries
        .iter()
        .filter(|entry| matches!(entry.event, AuditEvent::InviteRevoked { .. }))
        .count();
    assert_eq!(revocations, 1);
}

#[tokio::test]
async fn an_invite_grants_the_workspaces_it_names() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let second = create_workspace(&server, &owner, "Second").await;
    let third = create_workspace(&server, &owner, "Third").await;

    let guest = server
        .join_into(&owner, "Guest", &[second.id, third.id, second.id])
        .await;
    let account: OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    let mut expected = vec![second.id, third.id];
    expected.sort();
    assert_eq!(account.workspace_ids, expected);
    server
        .get(
            &guest,
            &format!("/api/v1/workspaces/{}/members", default_workspace.id),
        )
        .await
        .error(ErrorCode::NotFound);

    server
        .post(
            &owner,
            "/api/v1/invites",
            &json!({ "workspace_ids": [uuid::Uuid::now_v7()] }),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn an_inviter_grants_only_workspaces_they_are_a_member_of() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let closed = create_workspace(&server, &owner, "Closed").await;
    let inviter = server.join(&owner, "Inviter").await;
    server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;

    server
        .post(
            &inviter,
            "/api/v1/invites",
            &json!({ "workspace_ids": [closed.id] }),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &inviter,
            "/api/v1/invites",
            &json!({ "workspace_ids": [default_workspace.id, closed.id] }),
        )
        .await
        .error(ErrorCode::NotFound);
    server.invite(&inviter, json!({})).await;
    server
        .invite(&inviter, json!({ "workspace_ids": [default_workspace.id] }))
        .await;
}

#[tokio::test]
async fn a_join_request_waits_for_an_approval_that_names_the_workspaces() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let second = create_workspace(&server, &owner, "Second").await;
    let key = DeviceKey::generate();

    status(&server, &key).await.error(ErrorCode::NotFound);

    let request: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(request.name, "Newcomer");
    assert_eq!(request.expires_at - server.clock.now(), Duration::days(7));
    let again: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::OK);
    assert_eq!(again, request);

    let waiting: JoinRequestStatus = status(&server, &key).await.ok(StatusCode::OK);
    assert_eq!(waiting.state, JoinRequestState::Pending);
    assert_eq!(waiting.account_id, None);
    server
        .challenge_reply(&key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::UnknownDevice);
    status(&server, &DeviceKey::generate())
        .await
        .error(ErrorCode::NotFound);

    let listed: Value = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let mut shown: Vec<&str> = listed[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    shown.sort();
    assert_eq!(shown, ["created_at", "expires_at", "id", "name"]);

    let approved: JoinRequestStatus = server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({ "workspace_ids": [second.id] }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(approved.state, JoinRequestState::Approved);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Conflict);

    let result: JoinRequestStatus = status(&server, &key).await.ok(StatusCode::OK);
    assert_eq!(result.state, JoinRequestState::Approved);
    assert_eq!(result.account_id, approved.account_id);

    let session = server.login(&key).await;
    assert_eq!(Some(session.account_id), approved.account_id);
    let newcomer = member(key, session);
    let account: OwnAccount = server
        .get(&newcomer, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.name, "Newcomer");
    assert_eq!(account.workspace_ids, [second.id]);
    let devices: Vec<Device> = server
        .get(&newcomer, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices[0].origin, DeviceOrigin::JoinRequest);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::JoinRequestApproved {
            join_request_id: request.id,
            account_id: newcomer.account_id,
            workspace_ids: vec![second.id],
        }
    );
    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    assert!(listed.is_empty());
}

#[tokio::test]
async fn an_approval_without_workspaces_grants_the_default_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let key = DeviceKey::generate();
    let request: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .ok::<JoinRequestStatus>(StatusCode::OK);
    let newcomer = member(DeviceKey::generate(), server.login(&key).await);
    let account: OwnAccount = server
        .get(&newcomer, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.workspace_ids, [default_workspace.id]);
}

#[tokio::test]
async fn a_denied_or_expired_join_request_creates_no_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();

    let request: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    let denied: JoinRequestStatus = server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/denial", request.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(denied.state, JoinRequestState::Denied);
    let result: JoinRequestStatus = status(&server, &key).await.ok(StatusCode::OK);
    assert_eq!(result.state, JoinRequestState::Denied);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Conflict);

    let again: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    server.clock.advance(Duration::days(8));
    let result: JoinRequestStatus = status(&server, &key).await.ok(StatusCode::OK);
    assert_eq!(result.state, JoinRequestState::Expired);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", again.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Conflict);
    request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);

    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

#[tokio::test]
async fn an_expired_join_request_stays_the_latest_result_of_its_device() {
    let server = TestServer::start_with(|config| {
        config.limits.most_pending_join_requests = 2;
    })
    .await;
    let owner = server.owner().await;
    let first = DeviceKey::generate();
    let second = DeviceKey::generate();

    let denied: JoinRequest = request_to_join_reply(&server, &first, "First")
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/denial", denied.id),
            &json!({}),
        )
        .await
        .ok::<JoinRequestStatus>(StatusCode::OK);
    let expiring: JoinRequest = request_to_join_reply(&server, &first, "First")
        .await
        .ok(StatusCode::CREATED);
    request_to_join_reply(&server, &second, "Second")
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);

    server.clock.advance(Duration::days(8));
    for name in ["Third", "Fourth"] {
        request_to_join_reply(&server, &DeviceKey::generate(), name)
            .await
            .ok::<JoinRequest>(StatusCode::CREATED);
    }
    request_to_join_reply(&server, &DeviceKey::generate(), "Fifth")
        .await
        .error(ErrorCode::JoinRequestsFull);

    let result: JoinRequestStatus = status(&server, &first).await.ok(StatusCode::OK);
    assert_eq!(result.id, expiring.id);
    assert_eq!(result.state, JoinRequestState::Expired);
    let result: JoinRequestStatus = status(&server, &second).await.ok(StatusCode::OK);
    assert_eq!(result.state, JoinRequestState::Expired);

    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    let names: Vec<&str> = listed.iter().map(|request| request.name.as_str()).collect();
    assert_eq!(names, ["Third", "Fourth"]);

    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/denial", listed[0].id),
            &json!({}),
        )
        .await
        .ok::<JoinRequestStatus>(StatusCode::OK);
    let again: JoinRequest = request_to_join_reply(&server, &first, "First")
        .await
        .ok(StatusCode::CREATED);
    let result: JoinRequestStatus = status(&server, &first).await.ok(StatusCode::OK);
    assert_eq!(result.id, again.id);
    assert_eq!(result.state, JoinRequestState::Pending);
    let history: i64 =
        sqlx::query_scalar("select count(*) from join_requests where public_key = $1")
            .bind(first.public().as_bytes().as_slice())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(history, 3);
}

async fn redeem(server: &TestServer, member: &Member, code: &str) -> Reply {
    server
        .post(
            member,
            "/api/v1/invites/redemption",
            &json!({ "invite_code": code }),
        )
        .await
}

#[tokio::test]
async fn a_logged_in_account_enters_further_workspaces_with_an_invite() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let second = create_workspace(&server, &owner, "Second").await;
    let third = create_workspace(&server, &owner, "Third").await;
    let guest = server.join(&owner, "Guest").await;

    let invite = server
        .invite(
            &owner,
            json!({ "workspace_ids": [default_workspace.id, second.id, third.id], "max_uses": 2 }),
        )
        .await;
    let redemption: Redemption = redeem(&server, &guest, &invite.code.expose().to_lowercase())
        .await
        .ok(StatusCode::OK);
    let mut added = vec![second.id, third.id];
    added.sort();
    assert_eq!(redemption.workspace_ids, added);

    let account: OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.workspace_ids.len(), 3);
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 2);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(log.entries[0].actor_id, Some(guest.account_id));
    assert_eq!(
        log.entries[0].event,
        AuditEvent::InviteRedeemed {
            account_id: guest.account_id,
            invite_id: invite.invite.id,
            invite_created_by: owner.account_id,
            workspace_ids: added,
        }
    );

    redeem(&server, &guest, invite.code.expose())
        .await
        .error(ErrorCode::Conflict);
    let invites: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert_eq!(invites[0].uses, 1);

    server
        .join_reply(&DeviceKey::generate(), invite.code.expose(), "Newcomer")
        .await
        .ok::<Value>(StatusCode::CREATED);
    let late = server.join(&owner, "Late").await;
    redeem(&server, &late, invite.code.expose())
        .await
        .error(ErrorCode::InviteExhausted);
}

#[tokio::test]
async fn a_redemption_follows_the_expiry_and_cancellation_of_the_invite() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let second = create_workspace(&server, &owner, "Second").await;
    let guest = server.join(&owner, "Guest").await;

    redeem(&server, &guest, "ZZZZ-ZZZZ-ZZZZ-ZZZZ")
        .await
        .error(ErrorCode::InviteNotFound);

    let cancelled = server
        .invite(&owner, json!({ "workspace_ids": [second.id] }))
        .await;
    server
        .delete(&owner, &format!("/api/v1/invites/{}", cancelled.invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    redeem(&server, &guest, cancelled.code.expose())
        .await
        .error(ErrorCode::InviteRevoked);

    let expiring = server
        .invite(&owner, json!({ "workspace_ids": [second.id] }))
        .await;
    server.clock.advance(Duration::days(8));
    let guest = member(DeviceKey::generate(), server.login(&guest.key).await);
    redeem(&server, &guest, expiring.code.expose())
        .await
        .error(ErrorCode::InviteExpired);

    let account: OwnAccount = server
        .get(&guest, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.workspace_ids.len(), 1);
    server
        .open(
            "/api/v1/invites/redemption",
            &json!({ "invite_code": expiring.code }),
        )
        .await
        .error(ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn two_simultaneous_redemptions_of_a_one_use_code_admit_one_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let second = create_workspace(&server, &owner, "Second").await;
    let first_guest = server.join(&owner, "First").await;
    let second_guest = server.join(&owner, "Second").await;
    let invite = server
        .invite(&owner, json!({ "workspace_ids": [second.id] }))
        .await;

    let (first_reply, second_reply) = tokio::join!(
        redeem(&server, &first_guest, invite.code.expose()),
        redeem(&server, &second_guest, invite.code.expose()),
    );
    let (won, lost) = if first_reply.status == StatusCode::OK {
        (first_reply, second_reply)
    } else {
        (second_reply, first_reply)
    };
    won.ok::<Redemption>(StatusCode::OK);
    lost.error(ErrorCode::InviteExhausted);
    let members: i64 =
        sqlx::query_scalar("select count(*) from workspace_members where workspace_id = $1")
            .bind(second.id.as_uuid())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(members, 2);
}

#[tokio::test]
async fn a_second_request_of_one_key_returns_the_first_one() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();
    let first: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    server.clock.advance(Duration::minutes(5));
    let second: JoinRequest = request_to_join_reply(&server, &key, "Other name")
        .await
        .ok(StatusCode::OK);
    assert_eq!(second, first);
    assert_eq!(requests_of(&server, &key).await.len(), 1);
    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed, std::slice::from_ref(&first));

    // The one request makes one account, and the key cannot ask again.
    let approved: JoinRequestStatus = server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", first.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    request_to_join_reply(&server, &key, "Newcomer")
        .await
        .error(ErrorCode::KeyAlreadyRegistered);
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts where name = 'Newcomer'")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
    assert_eq!(
        server.login(&key).await.account_id,
        approved.account_id.unwrap()
    );
}

#[tokio::test]
async fn simultaneous_requests_of_one_key_make_one_request() {
    let server = TestServer::start().await;
    // Three rounds and the last send stay under the limit of join attempts per address.
    for _ in 0..3 {
        let key = DeviceKey::generate();
        let ask = || {
            let (server, key) = (&server, &key);
            async move { request_to_join_reply(server, key, "Newcomer").await }
        };
        let replies = tokio::join!(ask(), ask(), ask(), ask());
        let mut created = Vec::new();
        for reply in [replies.0, replies.1, replies.2, replies.3] {
            match reply.status {
                StatusCode::CREATED | StatusCode::OK => {
                    created.push(reply.ok::<JoinRequest>(reply.status).id);
                }
                // Two sends of one challenge: the second finds it used.
                _ => reply.error(ErrorCode::ChallengeInvalid),
            }
        }
        assert!(!created.is_empty());
        assert!(created.iter().all(|id| *id == created[0]), "{created:?}");
        assert_eq!(requests_of(&server, &key).await.len(), 1);
    }

    // One proof sent twice at the same time makes one request.
    let key = DeviceKey::generate();
    let proof = server.proof(&key, ChallengeOperation::RequestToJoin).await;
    let send = || {
        let (server, proof) = (&server, proof.clone());
        async move {
            server
                .open(
                    "/api/v1/join-requests",
                    &json!({ "proof": proof, "name": "Newcomer" }),
                )
                .await
        }
    };
    let (first, second) = tokio::join!(send(), send());
    let (won, lost) = if first.status == StatusCode::CREATED {
        (first, second)
    } else {
        (second, first)
    };
    won.ok::<JoinRequest>(StatusCode::CREATED);
    lost.error(ErrorCode::ChallengeInvalid);
    assert_eq!(requests_of(&server, &key).await.len(), 1);
}

#[tokio::test]
async fn a_withdrawn_request_cannot_be_approved_and_the_key_can_ask_again() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();

    // A key without a request gets the same answer as one with a request.
    withdraw(&server, &key).await.empty(StatusCode::NO_CONTENT);
    let first: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    withdraw(&server, &key).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(
        requests_of(&server, &key).await,
        [("withdrawn".to_owned(), None)]
    );
    status(&server, &key).await.error(ErrorCode::NotFound);
    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    assert!(listed.is_empty());
    for decision in ["approval", "denial"] {
        server
            .post(
                &owner,
                &format!("/api/v1/join-requests/{}/{decision}", first.id),
                &json!({}),
            )
            .await
            .error(ErrorCode::Conflict);
    }
    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::JoinRequestWithdrawn {
            join_request_id: first.id
        }
    );
    assert_eq!(log.entries[0].actor_id, None);

    // The key asks again: a new request, the only one that waits.
    let second: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    assert_ne!(second.id, first.id);
    let result: JoinRequestStatus = status(&server, &key).await.ok(StatusCode::OK);
    assert_eq!(result.id, second.id);

    // A denial stays the result after a later withdrawal of another request.
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/denial", second.id),
            &json!({}),
        )
        .await
        .ok::<JoinRequestStatus>(StatusCode::OK);
    let third: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    withdraw(&server, &key).await.empty(StatusCode::NO_CONTENT);
    let result: JoinRequestStatus = status(&server, &key).await.ok(StatusCode::OK);
    assert_eq!(
        (result.id, result.state),
        (second.id, JoinRequestState::Denied)
    );
    assert_ne!(third.id, second.id);

    // A key of an account cannot withdraw: its request was approved.
    let fourth: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", fourth.id),
            &json!({}),
        )
        .await
        .ok::<JoinRequestStatus>(StatusCode::OK);
    withdraw(&server, &key)
        .await
        .error(ErrorCode::KeyAlreadyRegistered);
    let mut states: Vec<String> = requests_of(&server, &key)
        .await
        .into_iter()
        .map(|(state, _)| state)
        .collect();
    states.sort();
    assert_eq!(states, ["approved", "denied", "withdrawn", "withdrawn"]);
}

#[tokio::test]
async fn a_key_that_joins_with_a_code_ends_its_waiting_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();
    let request: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    let created = server.invite(&owner, json!({})).await;
    let proof = server.proof(&key, ChallengeOperation::Join).await;
    server
        .open(
            "/api/v1/join",
            &json!({ "proof": proof, "invite_code": created.code.expose(), "name": "Newcomer" }),
        )
        .await
        .ok::<SessionCreated>(StatusCode::CREATED);
    assert_eq!(
        requests_of(&server, &key).await,
        [("withdrawn".to_owned(), None)]
    );
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Conflict);
    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    assert!(listed.is_empty());
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts where name = 'Newcomer'")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

#[tokio::test]
async fn an_inactive_account_that_returns_with_a_code_ends_its_waiting_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default = server.default_workspace(&owner).await;
    let guest = server.join(&owner, "Guest").await;
    server
        .post(
            &guest,
            &format!("/api/v1/workspaces/{}/departure", default.id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let request: JoinRequest = request_to_join_reply(&server, &guest.key, "Guest")
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(request.returning_account_id, Some(guest.account_id));
    assert_eq!(request.accounts_with_name, 0);

    let created = server.invite(&owner, json!({})).await;
    let proof = server.proof(&guest.key, ChallengeOperation::Join).await;
    let session: SessionCreated = server
        .open(
            "/api/v1/join",
            &json!({ "proof": proof, "invite_code": created.code.expose(), "name": "Guest" }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(session.account_id, guest.account_id);
    assert_eq!(
        requests_of(&server, &guest.key).await,
        [("withdrawn".to_owned(), None)]
    );
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Conflict);
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts where name = 'Guest'")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

#[tokio::test]
async fn a_key_that_becomes_a_linked_device_ends_its_waiting_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();
    let request: JoinRequest = request_to_join_reply(&server, &key, "Laptop of the owner")
        .await
        .ok(StatusCode::CREATED);
    let link = crate::devices::request_link(&server, &key, "laptop").await;
    server
        .post(
            &owner,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": key.public() }),
        )
        .await
        .ok::<Device>(StatusCode::CREATED);
    assert_eq!(
        requests_of(&server, &key).await,
        [("withdrawn".to_owned(), None)]
    );
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(server.login(&key).await.account_id, owner.account_id);
    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert!(log.entries.iter().any(|entry| entry.event
        == AuditEvent::JoinRequestWithdrawn {
            join_request_id: request.id
        }));
}

#[tokio::test]
async fn a_key_that_claims_the_server_ends_its_waiting_request() {
    let server = TestServer::start().await;
    let key = DeviceKey::generate();
    request_to_join_reply(&server, &key, "First")
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);
    server
        .claim_owner_reply(&key, &server.owner_code)
        .await
        .ok::<SessionCreated>(StatusCode::CREATED);
    assert_eq!(
        requests_of(&server, &key).await,
        [("withdrawn".to_owned(), None)]
    );
}

#[tokio::test]
async fn an_approval_and_a_withdrawal_at_the_same_time_make_at_most_one_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mut approved: i64 = 0;
    for _ in 0..3 {
        let key = DeviceKey::generate();
        let request: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
            .await
            .ok(StatusCode::CREATED);
        let proof = server
            .proof(&key, ChallengeOperation::WithdrawJoinRequest)
            .await;
        let path = format!("/api/v1/join-requests/{}/approval", request.id);
        let (empty, withdrawal_body) = (json!({}), json!({ "proof": proof }));
        let (approval, withdrawal) = tokio::join!(
            server.post(&owner, &path, &empty),
            server.open("/api/v1/join-requests/withdrawal", &withdrawal_body),
        );
        let states: Vec<String> = requests_of(&server, &key)
            .await
            .into_iter()
            .map(|(state, _)| state)
            .collect();
        if approval.status == StatusCode::OK {
            withdrawal.error(ErrorCode::KeyAlreadyRegistered);
            assert_eq!(states, ["approved"]);
            approved += 1;
        } else {
            approval.error(ErrorCode::Conflict);
            withdrawal.empty(StatusCode::NO_CONTENT);
            assert_eq!(states, ["withdrawn"]);
        }
    }
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts where name = 'Newcomer'")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, approved);
}

#[tokio::test]
async fn only_the_approvers_learn_how_many_accounts_have_the_name_of_a_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    server.join(&owner, "Linus").await;
    let key = DeviceKey::generate();

    // The requester has no account: neither answer tells which account names exist.
    for answered in [StatusCode::CREATED, StatusCode::OK] {
        let answer: Value = request_to_join_reply(&server, &key, " linus ")
            .await
            .ok(answered);
        assert!(answer.get("accounts_with_name").is_none(), "{answer}");
    }
    let answer: Value = status(&server, &key).await.ok(StatusCode::OK);
    assert!(answer.get("accounts_with_name").is_none(), "{answer}");

    request_to_join_reply(&server, &DeviceKey::generate(), "Marta")
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);
    let listed: Vec<JoinRequest> = server
        .get(&owner, "/api/v1/join-requests")
        .await
        .ok(StatusCode::OK);
    let counts: Vec<(&str, u32)> = listed
        .iter()
        .map(|request| (request.name.as_str(), request.accounts_with_name))
        .collect();
    assert_eq!(counts, [("linus", 1), ("Marta", 0)]);
}

/// Migration 0031 keeps the join requests of the earlier states and accepts a withdrawn one.
#[tokio::test]
async fn migration_0031_keeps_the_stored_join_requests() {
    use sqlx::ConnectOptions;

    let database = crate::support::empty_database().await;
    let mut connection = database.options.connect().await.unwrap();
    crate::migration_identifiers::migrations_up_to(30)
        .run(&mut connection)
        .await
        .unwrap();
    let account = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into accounts (id, name, status, created_at) values ($1, 'Ana', 'active', now())",
    )
    .bind(account)
    .execute(&mut connection)
    .await
    .unwrap();
    let insert = "insert into join_requests
        (id, public_key, name, state, created_at, expires_at, decided_at, decided_by, account_id)
        values ($1, $2, 'Ana', $3, now(), now(), $4, $5, $6)";
    let rows = [
        ("pending", false, false, false),
        ("expired", false, false, false),
        ("denied", true, true, false),
        ("approved", true, true, true),
    ];
    for (state, decided, by, made) in rows {
        sqlx::query(sqlx::AssertSqlSafe(insert))
            .bind(uuid::Uuid::now_v7())
            .bind(DeviceKey::generate().public().as_bytes().as_slice())
            .bind(state)
            .bind(decided.then(time::OffsetDateTime::now_utc))
            .bind(by.then_some(account))
            .bind(made.then_some(account))
            .execute(&mut connection)
            .await
            .unwrap();
    }
    crate::migration_identifiers::migrations_up_to(i64::MAX)
        .run(&mut connection)
        .await
        .unwrap();
    let mut states: Vec<String> = sqlx::query_scalar("select state from join_requests")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    states.sort();
    assert_eq!(states, ["approved", "denied", "expired", "pending"]);

    let withdrawn = |by: Option<uuid::Uuid>| {
        sqlx::query(sqlx::AssertSqlSafe(insert))
            .bind(uuid::Uuid::now_v7())
            .bind(DeviceKey::generate().public().as_bytes().as_slice())
            .bind("withdrawn")
            .bind(Some(time::OffsetDateTime::now_utc()))
            .bind(by)
            .bind(None::<uuid::Uuid>)
    };
    withdrawn(None).execute(&mut connection).await.unwrap();
    assert!(
        withdrawn(Some(account))
            .execute(&mut connection)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn join_requests_have_a_ceiling_and_refuse_registered_keys() {
    let server = TestServer::start_with(|config| {
        config.limits.most_pending_join_requests = 2;
    })
    .await;
    let owner = server.owner().await;
    request_to_join_reply(&server, &owner.key, "Owner")
        .await
        .error(ErrorCode::KeyAlreadyRegistered);
    for name in ["First", "Second"] {
        request_to_join_reply(&server, &DeviceKey::generate(), name)
            .await
            .ok::<JoinRequest>(StatusCode::CREATED);
    }
    request_to_join_reply(&server, &DeviceKey::generate(), "Third")
        .await
        .error(ErrorCode::JoinRequestsFull);
}

#[tokio::test]
async fn an_approver_grants_only_workspaces_they_are_a_member_of() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let closed = create_workspace(&server, &owner, "Closed").await;
    let approver = server.join(&owner, "Approver").await;
    server
        .grant_server(&owner, &approver, &[ServerPermission::ApproveJoinRequests])
        .await;
    let key = DeviceKey::generate();
    let request: JoinRequest = request_to_join_reply(&server, &key, "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/join-requests/{}/approval", request.id);

    server
        .post(&approver, &path, &json!({ "workspace_ids": [closed.id] }))
        .await
        .error(ErrorCode::NotFound);
    server
        .post(&approver, &path, &json!({}))
        .await
        .ok::<JoinRequestStatus>(StatusCode::OK);
}

async fn join_attempt(server: &TestServer, forwarded_for: Option<&str>) -> Reply {
    let key = DeviceKey::generate();
    let proof = server.proof(&key, ChallengeOperation::Join).await;
    server
        .send(
            Method::POST,
            "/api/v1/join",
            None,
            Some(&json!({ "proof": proof, "invite_code": "ZZZZ-ZZZZ-ZZZZ-ZZZZ", "name": "Guest" })),
            forwarded_for,
        )
        .await
}

#[tokio::test]
async fn join_attempts_are_limited_per_client_address() {
    let server = TestServer::start_with(|config| {
        config.limits.join_attempts.attempts = 3;
    })
    .await;
    for _ in 0..3 {
        join_attempt(&server, None)
            .await
            .error(ErrorCode::InviteNotFound);
    }
    let refused = join_attempt(&server, None).await;
    refused.error(ErrorCode::RateLimited);
    assert_eq!(refused.retry_after.as_deref(), Some("600"));

    let key = DeviceKey::generate();
    let proof = server.proof(&key, ChallengeOperation::RequestToJoin).await;
    server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Guest" }),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .claim_owner_reply(&DeviceKey::generate(), &server.owner_code)
        .await
        .error(ErrorCode::RateLimited);
    let proof = server.proof(&key, ChallengeOperation::LinkDevice).await;
    server
        .open("/api/v1/device-links", &json!({ "proof": proof }))
        .await
        .error(ErrorCode::RateLimited);

    server.clock.advance(Duration::minutes(10));
    join_attempt(&server, None)
        .await
        .error(ErrorCode::InviteNotFound);
}

#[tokio::test]
async fn a_forwarded_address_is_ignored_from_an_untrusted_peer() {
    let server = TestServer::start_with(|config| {
        config.limits.join_attempts.attempts = 2;
    })
    .await;
    join_attempt(&server, Some("198.51.100.1"))
        .await
        .error(ErrorCode::InviteNotFound);
    join_attempt(&server, Some("198.51.100.2"))
        .await
        .error(ErrorCode::InviteNotFound);
    join_attempt(&server, Some("198.51.100.3"))
        .await
        .error(ErrorCode::RateLimited);
}

#[tokio::test]
async fn a_forwarded_address_from_the_configured_proxy_is_the_client_address() {
    let server = TestServer::start_with(|config| {
        config.limits.join_attempts.attempts = 2;
        config.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
    })
    .await;
    for _ in 0..2 {
        join_attempt(&server, Some("198.51.100.1"))
            .await
            .error(ErrorCode::InviteNotFound);
    }
    join_attempt(&server, Some("198.51.100.1"))
        .await
        .error(ErrorCode::RateLimited);
    join_attempt(&server, Some("203.0.113.9, 198.51.100.1"))
        .await
        .error(ErrorCode::RateLimited);
    join_attempt(&server, Some("198.51.100.2"))
        .await
        .error(ErrorCode::InviteNotFound);
    join_attempt(&server, Some("198.51.100.1, 198.51.100.3"))
        .await
        .error(ErrorCode::InviteNotFound);
}

async fn redeem_from(server: &TestServer, member: &Member, code: &str, address: &str) -> Reply {
    server
        .send(
            Method::POST,
            "/api/v1/invites/redemption",
            Some(&member.token),
            Some(&json!({ "invite_code": code })),
            Some(address),
        )
        .await
}

#[tokio::test]
async fn redemptions_are_limited_per_account_and_per_client_address() {
    let server = TestServer::start_with(|config| {
        config.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
        config.limits.redemptions.attempts = 3;
    })
    .await;
    let owner = server.owner().await;
    let roaming = server.join(&owner, "Roaming").await;
    let first = server.join(&owner, "First").await;
    let second = server.join(&owner, "Second").await;
    let invite = server.invite(&owner, json!({ "max_uses": 5 })).await;

    for index in 0..3 {
        redeem_from(
            &server,
            &roaming,
            "ZZZZ-ZZZZ-ZZZZ-ZZZZ",
            &format!("198.51.100.{index}"),
        )
        .await
        .error(ErrorCode::InviteNotFound);
    }
    let refused = redeem_from(&server, &roaming, "ZZZZ-ZZZZ-ZZZZ-ZZZZ", "198.51.100.200").await;
    refused.error(ErrorCode::RateLimited);
    assert_eq!(refused.retry_after.as_deref(), Some("600"));

    for member in [&first, &second, &first] {
        redeem_from(&server, member, invite.code.expose(), "203.0.113.50")
            .await
            .error(ErrorCode::Conflict);
    }
    redeem_from(&server, &second, invite.code.expose(), "203.0.113.50")
        .await
        .error(ErrorCode::RateLimited);
    redeem_from(&server, &second, invite.code.expose(), "203.0.113.51")
        .await
        .error(ErrorCode::Conflict);

    let uses: i32 = sqlx::query_scalar("select uses from invites where id = $1")
        .bind(invite.invite.id.as_uuid())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(uses, 0);

    server.clock.advance(Duration::minutes(10));
    redeem_from(&server, &roaming, "ZZZZ-ZZZZ-ZZZZ-ZZZZ", "198.51.100.200")
        .await
        .error(ErrorCode::InviteNotFound);
}

#[tokio::test]
async fn invite_lists_name_the_creator_without_widening_the_audience() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let creator = server.join(&owner, "Creator").await;
    let outsider = server.join(&owner, "Outsider").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateInvites])
        .await;
    server
        .grant_server(&owner, &outsider, &[ServerPermission::CreateInvites])
        .await;
    let workspace: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Private"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                workspace.id, creator.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let owner_before = server.snapshot(&owner).await.cursor;
    let outsider_before = server.snapshot(&outsider).await.cursor;
    let created: junto_protocol::invite::InviteCreated = server
        .post(
            &creator,
            "/api/v1/invites",
            &json!({"workspace_ids":[workspace.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let sent = crate::discovery_support::events(
        &server,
        &owner,
        owner_before,
        junto_protocol::events::Level::Full,
    )
    .await;
    assert!(sent.iter().any(|event| matches!(event, junto_protocol::events::Event::InviteCreated { invite } if invite.id == created.invite.id && invite.created_by_name.as_deref() == Some("Creator"))));
    let hidden = crate::discovery_support::events(
        &server,
        &outsider,
        outsider_before,
        junto_protocol::events::Level::Full,
    )
    .await;
    assert!(!hidden.iter().any(|event| matches!(event, junto_protocol::events::Event::InviteCreated { invite } if invite.id == created.invite.id)));
    assert_eq!(created.invite.created_by_name.as_deref(), Some("Creator"));
    server
        .patch(&creator, "/api/v1/account", &json!({"name":"Renamed"}))
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    for reader in [&owner, &creator] {
        let list: Vec<Invite> = server
            .get(reader, "/api/v1/invites")
            .await
            .ok(StatusCode::OK);
        assert_eq!(
            list.iter()
                .find(|invite| invite.id == created.invite.id)
                .unwrap()
                .created_by_name
                .as_deref(),
            Some("Renamed")
        );
    }
    let list: Vec<Invite> = server
        .get(&outsider, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert!(!list.iter().any(|invite| invite.id == created.invite.id));
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", creator.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let list: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        list.iter()
            .find(|invite| invite.id == created.invite.id)
            .unwrap()
            .created_by_name
            .as_deref(),
        Some("Renamed")
    );
    server
        .get(&creator, "/api/v1/invites")
        .await
        .error(ErrorCode::Unauthenticated);
}
