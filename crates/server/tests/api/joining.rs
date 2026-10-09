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
    request_to_join_reply(&server, &key, "Newcomer")
        .await
        .error(ErrorCode::JoinRequestPending);

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
