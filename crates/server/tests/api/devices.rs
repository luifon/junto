use junto_protocol::ServerPermission;
use junto_protocol::account::{
    Device, DeviceLinkCreated, DeviceOrigin, PendingDeviceLink, RecoveryNotice,
};
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::error::ErrorCode;
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

use crate::support::{DeviceKey, Member, TestServer, member};

pub async fn request_link(server: &TestServer, key: &DeviceKey, name: &str) -> DeviceLinkCreated {
    let proof = server.proof(key, ChallengeOperation::LinkDevice).await;
    server
        .open(
            "/api/v1/device-links",
            &json!({ "proof": proof, "device_name": name }),
        )
        .await
        .ok(StatusCode::CREATED)
}

pub async fn link_second_device(server: &TestServer, first: &Member) -> Member {
    let key = DeviceKey::generate();
    let link = request_link(server, &key, "laptop").await;
    server
        .post(
            first,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": key.public() }),
        )
        .await
        .ok::<Device>(StatusCode::CREATED);
    let session = server.login(&key).await;
    member(key, session)
}

#[tokio::test]
async fn a_second_device_is_linked_with_a_one_time_code_and_a_named_key() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let new_key = DeviceKey::generate();

    server
        .challenge_reply(&new_key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::UnknownDevice);

    let link = request_link(&server, &new_key, "laptop").await;
    assert_eq!(link.code.expose().len(), 9);
    assert_eq!(link.expires_at - server.clock.now(), Duration::minutes(10));

    let pending: PendingDeviceLink = server
        .post(
            &owner,
            "/api/v1/account/device-links/lookup",
            &json!({ "code": link.code.expose().to_lowercase() }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(pending.public_key, new_key.public());
    assert_eq!(pending.device_name, "laptop");

    let other_key = DeviceKey::generate();
    server
        .post(
            &owner,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": other_key.public() }),
        )
        .await
        .error(ErrorCode::PublicKeyMismatch);

    let device: Device = server
        .post(
            &owner,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": new_key.public() }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(device.origin, DeviceOrigin::Link);
    assert_eq!(device.public_key, new_key.public());

    server
        .post(
            &owner,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": new_key.public() }),
        )
        .await
        .error(ErrorCode::LinkCodeInvalid);

    let session = server.login(&new_key).await;
    assert_eq!(session.account_id, owner.account_id);
    assert_eq!(session.device_id, device.id);

    let devices: Vec<Device> = server
        .get(&owner, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].origin, DeviceOrigin::OwnerCode);
    assert!(devices[0].current);
    assert!(!devices[1].current);
    assert!(devices[1].last_login_at.is_some());
}

#[tokio::test]
async fn a_link_code_expires_and_a_new_request_replaces_the_code() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();

    let expired = request_link(&server, &key, "").await;
    server.clock.advance(Duration::minutes(11));
    for path in ["lookup", "approve"] {
        server
            .post(
                &owner,
                &format!("/api/v1/account/device-links/{path}"),
                &json!({ "code": expired.code, "public_key": key.public() }),
            )
            .await
            .error(ErrorCode::LinkCodeInvalid);
    }

    let first = request_link(&server, &key, "").await;
    let second = request_link(&server, &key, "").await;
    assert_ne!(first.code, second.code);
    server
        .post(
            &owner,
            "/api/v1/account/device-links/lookup",
            &json!({ "code": first.code }),
        )
        .await
        .error(ErrorCode::LinkCodeInvalid);
    server
        .post(
            &owner,
            "/api/v1/account/device-links/lookup",
            &json!({ "code": second.code }),
        )
        .await
        .ok::<PendingDeviceLink>(StatusCode::OK);
}

#[tokio::test]
async fn a_registered_key_cannot_ask_for_a_link_and_a_link_needs_a_session() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let proof = server
        .proof(&owner.key, ChallengeOperation::LinkDevice)
        .await;
    server
        .open("/api/v1/device-links", &json!({ "proof": proof }))
        .await
        .error(ErrorCode::KeyAlreadyRegistered);

    let key = DeviceKey::generate();
    let link = request_link(&server, &key, "").await;
    server
        .open(
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": key.public() }),
        )
        .await
        .error(ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn removing_a_device_ends_its_sessions_and_its_key() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let second = link_second_device(&server, &owner).await;
    let second_again = server.login(&second.key).await;

    server
        .patch(
            &owner,
            &format!("/api/v1/account/devices/{}", second.device_id),
            &json!({ "name": "old laptop" }),
        )
        .await
        .ok::<Device>(StatusCode::OK);

    server
        .delete(
            &owner,
            &format!("/api/v1/account/devices/{}", second.device_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    server
        .get(&second, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    let other_session = Member {
        key: DeviceKey::generate(),
        account_id: second.account_id,
        device_id: second.device_id,
        token: second_again.token.expose().to_owned(),
    };
    server
        .get(&other_session, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .challenge_reply(&second.key.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::UnknownDevice);

    server
        .get(&owner, "/api/v1/account")
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    let devices: Vec<Device> = server
        .get(&owner, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn a_device_can_remove_itself_but_the_last_device_stays() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    server
        .delete(
            &owner,
            &format!("/api/v1/account/devices/{}", owner.device_id),
        )
        .await
        .error(ErrorCode::LastDevice);

    let second = link_second_device(&server, &owner).await;
    server
        .delete(
            &second,
            &format!("/api/v1/account/devices/{}", second.device_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&second, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn devices_of_another_account_are_out_of_reach() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    link_second_device(&server, &guest).await;

    let path = format!("/api/v1/account/devices/{}", guest.device_id);
    server
        .delete(&owner, &path)
        .await
        .error(ErrorCode::NotFound);
    server
        .patch(&owner, &path, &json!({ "name": "taken" }))
        .await
        .error(ErrorCode::NotFound);
    let devices: Vec<Device> = server
        .get(&owner, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn the_owner_recovers_an_account_and_the_account_sees_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let new_key = DeviceKey::generate();

    let notice: RecoveryNotice = server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", guest.account_id),
            &json!({ "public_key": new_key.public(), "device_name": "new phone" }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(notice.performed_by, Some(owner.account_id));
    assert_eq!(notice.performed_at, server.clock.now());

    let recovered = server.login(&new_key).await;
    assert_eq!(recovered.account_id, guest.account_id);
    assert_eq!(recovered.device_id, notice.device_id.unwrap());
    assert_eq!(recovered.recoveries, std::slice::from_ref(&notice));
    assert_eq!(
        server.login(&new_key).await.recoveries,
        std::slice::from_ref(&notice)
    );

    server.clock.advance(Duration::hours(1));
    let first_device = server.login(&guest.key).await;
    assert_eq!(first_device.recoveries, std::slice::from_ref(&notice));
    let again = server.login(&guest.key).await;
    assert_eq!(again.recoveries, std::slice::from_ref(&notice));

    let guest = member(guest.key, again);
    let listed: Vec<RecoveryNotice> = server
        .get(&guest, "/api/v1/account/recoveries")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed, std::slice::from_ref(&notice));
    let devices: Vec<Device> = server
        .get(&guest, "/api/v1/account/devices")
        .await
        .ok(StatusCode::OK);
    assert_eq!(devices[1].origin, DeviceOrigin::Recovery);
    assert_eq!(devices[1].name, "new phone");

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::AccountRecovered {
            account_id: guest.account_id,
            device_id: notice.device_id.unwrap(),
        }
    );
    assert_eq!(log.entries[0].actor_id, Some(owner.account_id));
}

#[tokio::test]
async fn a_recovery_notice_stays_for_each_device_until_that_device_acknowledges_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let new_key = DeviceKey::generate();
    let recover = |key: &DeviceKey| {
        let body = json!({ "public_key": key.public() });
        let path = format!("/api/v1/accounts/{}/recovery", guest.account_id);
        let (server, owner) = (&server, &owner);
        async move {
            server
                .post(owner, &path, &body)
                .await
                .ok::<RecoveryNotice>(StatusCode::CREATED)
        }
    };
    let first = recover(&new_key).await;
    server.clock.advance(Duration::minutes(5));
    let second = recover(&DeviceKey::generate()).await;
    let both = [first.clone(), second.clone()];

    let session = server.login(&guest.key).await;
    let old_device = member(guest.key, session);
    let session = server.login(&new_key).await;
    let new_device = member(new_key, session);
    async fn pending(server: &TestServer, device: &Member) -> Vec<RecoveryNotice> {
        server
            .get(device, "/api/v1/account/recovery-notices")
            .await
            .ok(StatusCode::OK)
    }
    assert_eq!(pending(&server, &old_device).await, both);
    assert_eq!(pending(&server, &new_device).await, both);

    let notice = |id| format!("/api/v1/account/recovery-notices/{id}");
    server
        .delete(&old_device, &notice(first.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        pending(&server, &old_device).await,
        std::slice::from_ref(&second)
    );
    assert_eq!(pending(&server, &new_device).await, both);
    assert_eq!(
        server.login(&old_device.key).await.recoveries,
        std::slice::from_ref(&second)
    );
    assert_eq!(server.login(&new_device.key).await.recoveries, both);

    server
        .delete(&old_device, &notice(first.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(
            &old_device,
            &notice(junto_protocol::ids::RecoveryId::generate()),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&owner, &notice(second.id))
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(pending(&server, &new_device).await, both);

    server
        .delete(&old_device, &notice(second.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(pending(&server, &old_device).await.is_empty());
    assert!(server.login(&old_device.key).await.recoveries.is_empty());
    let listed: Vec<RecoveryNotice> = server
        .get(&old_device, "/api/v1/account/recoveries")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed, both);
}

#[tokio::test]
async fn a_recovery_ends_every_session_of_the_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let second = link_second_device(&server, &guest).await;
    let bystander = server.join(&owner, "Bystander").await;

    let notice: RecoveryNotice = server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", guest.account_id),
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .ok(StatusCode::CREATED);

    for stolen in [&guest, &second] {
        server
            .get(stolen, "/api/v1/account")
            .await
            .error(ErrorCode::Unauthenticated);
    }
    let sessions: i64 = sqlx::query_scalar(
        "select count(*) from sessions s join devices d on d.id = s.device_id
         where d.account_id = $1",
    )
    .bind(guest.account_id.as_uuid())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(sessions, 0);
    for untouched in [&owner, &bystander] {
        server
            .get(untouched, "/api/v1/account")
            .await
            .ok::<serde_json::Value>(StatusCode::OK);
    }

    for key in [&guest.key, &second.key] {
        let session = server.login(key).await;
        assert_eq!(session.account_id, guest.account_id);
        assert_eq!(session.recoveries, std::slice::from_ref(&notice));
    }
}

#[tokio::test]
async fn only_the_owner_recovers_an_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, ServerPermission::ALL)
        .await;
    let guest = server.join(&owner, "Guest").await;

    for caller in [&administrator, &guest] {
        server
            .post(
                caller,
                &format!("/api/v1/accounts/{}/recovery", guest.account_id),
                &json!({ "public_key": DeviceKey::generate().public() }),
            )
            .await
            .error(ErrorCode::OwnerOnly);
    }
    let devices: i64 = sqlx::query_scalar("select count(*) from devices where origin = 'recovery'")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(devices, 0);
}

#[tokio::test]
async fn a_recovery_needs_an_unused_key_and_an_active_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let path = format!("/api/v1/accounts/{}/recovery", guest.account_id);

    server
        .post(&owner, &path, &json!({ "public_key": owner.key.public() }))
        .await
        .error(ErrorCode::KeyAlreadyRegistered);

    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &owner,
            &path,
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .error(ErrorCode::Conflict);

    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", uuid::Uuid::now_v7()),
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn waiting_device_links_have_a_ceiling() {
    let server = TestServer::start_with(|config| {
        config.limits.most_pending_device_links = 2;
    })
    .await;
    for _ in 0..2 {
        request_link(&server, &DeviceKey::generate(), "").await;
    }
    let again = DeviceKey::generate();
    for key in [&DeviceKey::generate(), &again] {
        let proof = server.proof(key, ChallengeOperation::LinkDevice).await;
        server
            .open("/api/v1/device-links", &json!({ "proof": proof }))
            .await
            .error(ErrorCode::DeviceLinksFull);
    }
    let stored: i64 = sqlx::query_scalar("select count(*) from device_links")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored, 2);

    server.clock.advance(Duration::minutes(11));
    request_link(&server, &again, "").await;
}
