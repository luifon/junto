use junto_protocol::account::Device;
use junto_protocol::device_relay::{RelayAnswer, RelayOffer};
use junto_protocol::events::{Event, Level};
use junto_protocol::server::ServerInfo;
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

use crate::devices::request_link;
use crate::support::{DeviceKey, Member, TestServer, member};

const OFFER: &str = "b2ZmZXI";
const ANSWER: &str = "YW5zd2Vy";

/// Links a new device with an offer, and returns the new device logged in.
async fn link_with_offer(server: &TestServer, first: &Member) -> (Member, Device) {
    let key = DeviceKey::generate();
    let link = request_link(server, &key, "laptop").await;
    let device: Device = server
        .post(
            first,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": key.public(), "relay": OFFER }),
        )
        .await
        .ok(StatusCode::CREATED);
    let session = server.login(&key).await;
    (member(key, session), device)
}

#[tokio::test]
async fn the_approval_creates_a_relay_that_only_its_two_devices_read_and_answer() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let info: ServerInfo = server.get(&owner, "/api/server").await.ok(StatusCode::OK);
    assert!(info.capabilities.device_link_relay);
    let mut stream = server.subscribed(&owner, Level::Background).await;
    let (new, device) = link_with_offer(&server, &owner).await;
    loop {
        if let (_, Event::DeviceLinkRelayChanged { device_id }) = stream.event().await {
            assert_eq!(device_id, device.id);
            break;
        }
    }

    let offer: RelayOffer = server
        .get(&new, "/api/v1/account/device-link-relay")
        .await
        .ok(StatusCode::OK);
    assert_eq!(serde_json::to_value(&offer.offer).unwrap(), OFFER);
    assert_eq!(offer.old_public_key, owner.key.public());
    assert_eq!(offer.expires_at - server.clock.now(), Duration::minutes(15));
    server
        .get(&owner, "/api/v1/account/device-link-relay")
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);

    let path = format!("/api/v1/account/device-link-relays/{}", device.id);
    let before: RelayAnswer = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(before.answer, None);
    assert_eq!(before.new_public_key, new.key.public());
    server
        .put(
            &new,
            "/api/v1/account/device-link-relay/answer",
            &json!({ "answer": ANSWER }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    loop {
        if let (_, Event::DeviceLinkRelayChanged { device_id }) = stream.event().await {
            assert_eq!(device_id, device.id);
            break;
        }
    }
    let after: RelayAnswer = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(serde_json::to_value(&after.answer).unwrap(), ANSWER);
    server
        .get(&new, &path)
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);

    let third = crate::devices::link_second_device(&server, &owner).await;
    server
        .get(&third, &path)
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);
    server
        .get(&third, "/api/v1/account/device-link-relay")
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);
    let other = server.join(&owner, "Other").await;
    server
        .delete(&other, &path)
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);

    server
        .delete(&new, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&owner, &path)
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);
}

#[tokio::test]
async fn a_relay_ends_after_fifteen_minutes_and_with_the_removal_of_a_device() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let (mut new, _) = link_with_offer(&server, &owner).await;
    server.clock.advance(Duration::minutes(15));
    server
        .get(&new, "/api/v1/account/device-link-relay")
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);
    server
        .put(
            &new,
            "/api/v1/account/device-link-relay/answer",
            &json!({ "answer": ANSWER }),
        )
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("select count(*) from device_link_relays")
        .fetch_one(&mut server.test_data().await)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    server.renew(&mut new).await;
    let (_, second) = link_with_offer(&server, &owner).await;
    server
        .delete(&owner, &format!("/api/v1/account/devices/{}", second.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let rows: i64 = sqlx::query_scalar("select count(*) from device_link_relays")
        .fetch_one(&mut server.test_data().await)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_relay_takes_a_bounded_message_and_a_bounded_count_of_answers() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = DeviceKey::generate();
    let link = request_link(&server, &key, "laptop").await;
    for relay in [json!(""), json!("AQID+g=="), json!("A".repeat(87_383))] {
        server
            .post(
                &owner,
                "/api/v1/account/device-links/approve",
                &json!({ "code": link.code, "public_key": key.public(), "relay": relay }),
            )
            .await
            .error(junto_protocol::error::ErrorCode::InvalidRequest);
    }
    let (new, _) = link_with_offer(&server, &owner).await;
    for _ in 0..256 {
        server
            .put(
                &new,
                "/api/v1/account/device-link-relay/answer",
                &json!({ "answer": ANSWER }),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    server
        .put(
            &new,
            "/api/v1/account/device-link-relay/answer",
            &json!({ "answer": ANSWER }),
        )
        .await
        .error(junto_protocol::error::ErrorCode::InvalidRequest);
}
