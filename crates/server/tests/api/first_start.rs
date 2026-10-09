use junto_protocol::account::OwnAccount;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::SessionCreated;
use junto_protocol::error::ErrorCode;
use junto_protocol::structure::{ChannelKind, ChannelList, Workspace};
use junto_protocol::{API_VERSION, ServerPermission};
use reqwest::StatusCode;

use crate::support::{DeviceKey, TestServer, member};

#[tokio::test]
async fn server_information_is_public() {
    let server = TestServer::start().await;
    assert_eq!(server.info.name, "Acme");
    assert_eq!(server.info.logo_path, None);
    assert_eq!(server.info.api_versions, [API_VERSION]);
}

#[tokio::test]
async fn first_start_makes_a_usable_server_and_the_owner_code_makes_the_owner() {
    let server = TestServer::start().await;
    assert_eq!(server.owner_code.replace('-', "").len(), 20);

    let stranger = DeviceKey::generate();
    server
        .claim_owner_reply(&stranger, "AAAA-AAAA-AAAA-AAAA-AAAA")
        .await
        .error(ErrorCode::OwnerCodeInvalid);

    let key = DeviceKey::generate();
    let lower_case_without_hyphens = server.owner_code.replace('-', "").to_lowercase();
    let session: SessionCreated = server
        .claim_owner_reply(&key, &lower_case_without_hyphens)
        .await
        .ok(StatusCode::CREATED);
    assert!(session.recoveries.is_empty());
    let owner = member(key, session);

    let account: OwnAccount = server
        .get(&owner, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.id, owner.account_id);
    assert_eq!(account.name, "Owner");
    assert!(account.is_owner);
    assert_eq!(account.server_permissions, Vec::<ServerPermission>::new());

    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
    assert!(workspaces[0].is_default);
    assert_eq!(workspaces[0].name, "Acme");
    assert_eq!(account.workspace_ids, [workspaces[0].id]);

    let channels: ChannelList = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspaces[0].id),
        )
        .await
        .ok(StatusCode::OK);
    let kinds: Vec<(ChannelKind, &str)> = channels
        .channels
        .iter()
        .map(|channel| (channel.kind, channel.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (ChannelKind::Text, "general"),
            (ChannelKind::Voice, "voice")
        ]
    );

    let late = DeviceKey::generate();
    server
        .claim_owner_reply(&late, &server.owner_code)
        .await
        .error(ErrorCode::OwnerAlreadyClaimed);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].actor_id, Some(owner.account_id));
    assert_eq!(
        log.entries[0].event,
        AuditEvent::OwnerClaimed {
            account_id: owner.account_id
        }
    );
}

#[tokio::test]
async fn a_restart_before_the_claim_replaces_the_owner_code() {
    let first = TestServer::start().await;
    let second = first.restart().await;
    assert_ne!(first.owner_code, second.owner_code);
    assert_eq!(first.info.server_id, second.info.server_id);

    second
        .claim_owner_reply(&DeviceKey::generate(), &first.owner_code)
        .await
        .error(ErrorCode::OwnerCodeInvalid);
    let owner = second.owner().await;

    let third = second.restart().await;
    assert_eq!(third.owner_code, "");
    let workspaces: Vec<Workspace> = third
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
    let stored_code: Option<Vec<u8>> =
        sqlx::query_scalar("select owner_code_hash from server_state")
            .fetch_one(&third.db)
            .await
            .unwrap();
    assert_eq!(stored_code, None);
}

#[tokio::test]
async fn the_owner_code_is_stored_only_as_a_hash() {
    let server = TestServer::start().await;
    let stored: Vec<u8> = sqlx::query_scalar("select owner_code_hash from server_state")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, server.owner_code.as_bytes());
}

#[tokio::test]
async fn two_devices_that_present_the_owner_code_at_once_make_one_owner() {
    let server = TestServer::start().await;
    let first = DeviceKey::generate();
    let second = DeviceKey::generate();
    let (first_reply, second_reply) = tokio::join!(
        server.claim_owner_reply(&first, &server.owner_code),
        server.claim_owner_reply(&second, &server.owner_code),
    );
    let mut statuses = [first_reply.status, second_reply.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}
