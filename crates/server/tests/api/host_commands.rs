use junto_protocol::ServerPermission;
use junto_protocol::account::{Device, OwnAccount, RecoveryNotice};
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::SessionCreated;
use junto_protocol::error::ErrorCode;
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

use crate::support::{DeviceKey, TestServer, host_command_on, member, unused_database_url};

#[tokio::test]
async fn the_host_command_assigns_a_new_owner() {
    let server = TestServer::start().await;
    let old_owner = server.owner().await;
    let successor = server.join(&old_owner, "Successor").await;
    let guest = server.join(&old_owner, "Guest").await;
    let recovery = format!("/api/v1/accounts/{}/recovery", guest.account_id);
    let new_key = || json!({ "public_key": DeviceKey::generate().public() });

    server
        .post(&successor, &recovery, &new_key())
        .await
        .error(ErrorCode::OwnerOnly);

    let command = server
        .host_command(&["assign-owner", &successor.account_id.to_string()])
        .await;
    assert!(command.success, "{}", command.stderr);

    let account: OwnAccount = server
        .get(&successor, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert!(account.is_owner);
    server
        .post(&successor, &recovery, &new_key())
        .await
        .ok::<RecoveryNotice>(StatusCode::CREATED);

    let account: OwnAccount = server
        .get(&old_owner, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert!(!account.is_owner);
    server
        .post(&old_owner, &recovery, &new_key())
        .await
        .error(ErrorCode::OwnerOnly);
    server
        .get(&old_owner, "/api/v1/audit-log")
        .await
        .error(ErrorCode::Forbidden);

    let log: AuditPage = server
        .get(&successor, "/api/v1/audit-log?limit=200")
        .await
        .ok(StatusCode::OK);
    let entry = log
        .entries
        .iter()
        .find(|entry| entry.event.action() == "owner_assigned")
        .unwrap();
    assert_eq!(entry.actor_id, None);
    assert_eq!(
        entry.event,
        AuditEvent::OwnerAssigned {
            account_id: successor.account_id,
            previous_owner_id: Some(old_owner.account_id),
        }
    );
}

#[tokio::test]
async fn the_host_command_refuses_an_account_that_cannot_own_the_server() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let banned = server.join(&owner, "Banned").await;
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", banned.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    for (argument, reason) in [
        (banned.account_id.to_string(), "banned or removed"),
        (
            uuid::Uuid::now_v7().to_string(),
            "no account has this identifier",
        ),
        (
            "not-an-identifier".to_owned(),
            "is not an account identifier",
        ),
    ] {
        let command = server.host_command(&["assign-owner", &argument]).await;
        assert!(!command.success, "{argument}: {}", command.stdout);
        assert!(command.stderr.contains(reason), "{}", command.stderr);
        assert_eq!(command.stdout, "");
    }
    for arguments in [
        &["make-me-owner"][..],
        &["assign-owner"],
        &["issue-owner-code", "now"],
    ] {
        let command = server.host_command(arguments).await;
        assert!(!command.success);
        assert!(command.stderr.contains("usage:"), "{}", command.stderr);
    }

    let account: OwnAccount = server
        .get(&owner, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert!(account.is_owner);
    let entries: i64 =
        sqlx::query_scalar("select count(*) from audit_entries where action = 'owner_assigned'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(entries, 0);
}

#[tokio::test]
async fn the_host_commands_refuse_a_database_without_a_server() {
    let account = uuid::Uuid::now_v7().to_string();
    for arguments in [&["issue-owner-code"][..], &["assign-owner", &account]] {
        let command = host_command_on(unused_database_url(), arguments).await;
        assert!(!command.success, "{}", command.stdout);
        assert!(
            command.stderr.contains("has not been started"),
            "{}",
            command.stderr
        );
        assert_eq!(command.stdout, "");
    }
}

#[tokio::test]
async fn an_owner_without_devices_returns_with_a_code_from_the_host_command() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;

    let restarted = server.restart().await;
    assert_eq!(restarted.owner_code, "");
    server
        .claim_owner_reply(&DeviceKey::generate(), &server.owner_code)
        .await
        .error(ErrorCode::OwnerAlreadyClaimed);

    let command = server.host_command(&["issue-owner-code"]).await;
    assert!(command.success, "{}", command.stderr);
    let code = command.owner_code();

    server
        .claim_owner_reply(&DeviceKey::generate(), "AAAA-AAAA-AAAA-AAAA-AAAA")
        .await
        .error(ErrorCode::OwnerCodeInvalid);

    let new_key = DeviceKey::generate();
    let session: SessionCreated = server
        .claim_owner_reply(&new_key, &code)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(session.account_id, owner.account_id);
    assert_eq!(session.recoveries.len(), 1);
    assert_eq!(session.recoveries[0].performed_by, None);
    let returned = member(new_key, session);

    let account: OwnAccount = server
        .get(&returned, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert!(account.is_owner);
    assert_eq!(account.name, "Owner");
    let devices: Vec<Device> = server
        .get(&returned, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices.len(), 2);

    server
        .get(&owner, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .claim_owner_reply(&DeviceKey::generate(), &code)
        .await
        .error(ErrorCode::OwnerAlreadyClaimed);

    let log: AuditPage = server
        .get(&returned, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(log.entries[0].actor_id, None);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::OwnerAccessRestored {
            account_id: owner.account_id,
            device_id: returned.device_id,
        }
    );
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 2);
}

#[tokio::test]
async fn an_issued_owner_code_expires_and_a_new_one_replaces_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let first = server
        .host_command(&["issue-owner-code"])
        .await
        .owner_code();
    let second = server
        .host_command(&["issue-owner-code"])
        .await
        .owner_code();
    assert_ne!(first, second);
    server
        .claim_owner_reply(&DeviceKey::generate(), &first)
        .await
        .error(ErrorCode::OwnerCodeInvalid);

    server.clock.advance(Duration::minutes(61));
    server
        .claim_owner_reply(&DeviceKey::generate(), &second)
        .await
        .error(ErrorCode::OwnerCodeInvalid);
    server
        .get(&owner, "/api/v1/account")
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    let devices: i64 = sqlx::query_scalar("select count(*) from devices")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(devices, 1);
}

#[tokio::test]
async fn issuing_an_owner_code_is_written_to_the_audit_log_without_the_code() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let first = server
        .host_command(&["issue-owner-code"])
        .await
        .owner_code();
    let second = server
        .host_command(&["issue-owner-code"])
        .await
        .owner_code();

    let reply = server.get(&owner, "/api/v1/audit-log").await;
    for code in [&first, &second] {
        assert!(!reply.text().contains(code.as_str()));
        assert!(!reply.text().contains(&code.replace('-', "")));
    }
    let log: AuditPage = reply.ok(StatusCode::OK);
    let issued: Vec<_> = log
        .entries
        .iter()
        .filter(|entry| entry.event.action() == "owner_code_issued")
        .collect();
    assert_eq!(issued.len(), 2);
    for entry in &issued {
        assert_eq!(entry.actor_id, None);
    }
    let replaced: Vec<bool> = issued
        .iter()
        .map(|entry| match entry.event {
            AuditEvent::OwnerCodeIssued {
                replaced_a_code,
                expires_at,
            } => {
                assert!(expires_at.is_some());
                replaced_a_code
            }
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(replaced, [true, false]);

    server.clock.advance(Duration::minutes(61));
    let third = server.host_command(&["issue-owner-code"]).await;
    assert!(third.success);
    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries
            .iter()
            .filter(|entry| entry.event.action() == "owner_code_issued")
            .count(),
        3
    );
    let devices: i64 = sqlx::query_scalar("select count(*) from devices")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(devices, 1);
}
