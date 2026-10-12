//! Proofs of the audit of the whole server: limits on what one account, one device or one
//! party without an account can cause. Each test failed while its defect existed.

use junto_protocol::account::{Device, OwnAccount};
use junto_protocol::auth::{ChallengeOperation, Proof};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{EndReason, Level, ServerMessage};
use junto_protocol::message::Message;
use junto_server::image::samples;
use junto_server::maintenance;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::Scene;
use crate::devices::request_link;
use crate::messages_support::{Files, channel, message, send};
use crate::support::{DeviceKey, Member, TestServer, origin};

const ACCOUNT: &str = "/api/v1/account";
const AVATAR: &str = "/api/v1/account/avatar";

fn device_path(member: &Member) -> String {
    format!("/api/v1/account/devices/{}", member.device_id)
}

async fn events_of(server: &TestServer, kind: &str) -> i64 {
    sqlx::query_scalar("select count(*) from events where kind = $1")
        .bind(kind)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn rows(server: &TestServer, statement: &'static str, id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(statement)
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// ADR 0119: the changes of the own profile and of the names of the own devices share a
/// limit of 30 requests per account per minute. Each of them can record an event, and the
/// log keeps a fixed number of positions (ADR 0207), so one account does not use them
/// without a bound.
#[tokio::test]
async fn the_changes_of_the_own_account_are_limited_per_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    for turn in 0..30 {
        server
            .patch(&owner, ACCOUNT, &json!({ "name": format!("Owner {turn}") }))
            .await
            .ok::<OwnAccount>(StatusCode::OK);
    }
    let profiles = events_of(&server, "account_profile_updated").await;
    assert_eq!(profiles, 30);

    let renamed = json!({ "name": "one more" });
    for refused in [
        server.patch(&owner, ACCOUNT, &renamed).await,
        server
            .put_bytes(&owner, AVATAR, Some("image/png"), samples::png(8, 8))
            .await,
        server.delete(&owner, AVATAR).await,
        server.patch(&owner, &device_path(&owner), &renamed).await,
    ] {
        refused.error(ErrorCode::RateLimited);
        assert_eq!(refused.retry_after.as_deref(), Some("60"));
    }
    assert_eq!(
        events_of(&server, "account_profile_updated").await,
        profiles
    );
    assert_eq!(events_of(&server, "device_renamed").await, 0);

    // The limit counts each account alone, and a new window starts after one minute.
    server
        .patch(&other, ACCOUNT, &renamed)
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    server.clock.advance(Duration::minutes(1));
    server
        .patch(&owner, &device_path(&owner), &renamed)
        .await
        .ok::<Device>(StatusCode::OK);
}

/// ADR 0119: a request that changes nothing records no event. A device that gets the name
/// that it has uses no position of the log.
#[tokio::test]
async fn a_device_rename_to_the_name_that_it_has_records_no_event() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let renamed = json!({ "name": "desk two" });
    let first: Device = server
        .patch(&owner, &device_path(&owner), &renamed)
        .await
        .ok(StatusCode::OK);
    assert_eq!(first.name, "desk two");
    assert_eq!(events_of(&server, "device_renamed").await, 1);
    let head = server.log().await.len();

    let again: Device = server
        .patch(&owner, &device_path(&owner), &renamed)
        .await
        .ok(StatusCode::OK);
    assert_eq!(again, first);
    assert_eq!(server.log().await.len(), head);
    let absent = format!("/api/v1/account/devices/{}", uuid::Uuid::now_v7());
    server
        .patch(&owner, &absent, &renamed)
        .await
        .error(ErrorCode::NotFound);
}

/// ADR 0109: a device holds at most 4 sessions. A further login ends the oldest one, with
/// its place in a call and its connection, so the limits that count per session do not
/// grow with the number of logins of one device.
#[tokio::test]
async fn a_device_holds_at_most_four_sessions_and_a_further_login_ends_the_oldest() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let _seat_of_ann = server.enter_call(ann, scene.voice.id).await;
    let _seat_of_ben = server.enter_call(ben, scene.voice.id).await;
    let mut stream = server.subscribed(ann, Level::Full).await;
    let sessions = "select count(*) from sessions where device_id = $1";
    let places = "select count(*) from call_members where account_id = $1";

    let mut later = Vec::new();
    for _ in 0..3 {
        later.push(server.login(&ann.key).await);
        server.clock.advance(Duration::seconds(1));
    }
    assert_eq!(rows(server, sessions, ann.device_id.as_uuid()).await, 4);
    assert_eq!(rows(server, places, ann.account_id.as_uuid()).await, 1);
    server
        .get(ann, ACCOUNT)
        .await
        .ok::<OwnAccount>(StatusCode::OK);

    later.push(server.login(&ann.key).await);
    assert_eq!(rows(server, sessions, ann.device_id.as_uuid()).await, 4);
    server
        .get(ann, ACCOUNT)
        .await
        .error(ErrorCode::Unauthenticated);
    for session in &later {
        let token = Some(session.token.expose());
        server
            .send(Method::GET, ACCOUNT, token, None::<&Value>, None)
            .await
            .ok::<OwnAccount>(StatusCode::OK);
    }
    assert_eq!(rows(server, places, ann.account_id.as_uuid()).await, 0);
    let (messages, _) = stream.until_closed().await;
    let ended = ServerMessage::Ended {
        reason: EndReason::SessionEnded,
        recovery: None,
    };
    assert_eq!(messages.last(), Some(&ended), "{messages:?}");
    // The sessions of another device are not counted.
    assert_eq!(rows(server, sessions, ben.device_id.as_uuid()).await, 1);
    assert_eq!(rows(server, places, ben.account_id.as_uuid()).await, 1);
    server
        .get(ben, ACCOUNT)
        .await
        .ok::<OwnAccount>(StatusCode::OK);
}

/// ADR 0408: the read of one message has a limit of 1,200 requests per account per
/// minute, as the history has one of 120.
#[tokio::test]
async fn reads_of_one_message_are_limited_per_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "message").await;
    for _ in 0..1200 {
        let read = server.get(&owner, &message(sent.id)).await;
        assert_eq!(read.status, StatusCode::OK);
    }
    server
        .get(&owner, &message(sent.id))
        .await
        .error(ErrorCode::RateLimited);
    server
        .get(&other, &message(sent.id))
        .await
        .ok::<Message>(StatusCode::OK);
    server.clock.advance(Duration::minutes(1));
    server
        .get(&owner, &message(sent.id))
        .await
        .ok::<Message>(StatusCode::OK);
}

/// ADR 0110: a link code has 40 bits. An account tries at most 10 link codes in 10
/// minutes, for the lookup and the approval together.
#[tokio::test]
async fn attempts_with_a_link_code_are_limited_per_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let key = DeviceKey::generate();
    let link = request_link(&server, &key, "laptop").await;
    let path = |step: &str| format!("/api/v1/account/device-links/{step}");
    let guess = json!({ "code": "0000-0000", "public_key": key.public() });
    for turn in 0..10 {
        let step = ["lookup", "approve"][turn % 2];
        server
            .post(&owner, &path(step), &guess)
            .await
            .error(ErrorCode::LinkCodeInvalid);
    }

    let real = json!({ "code": link.code, "public_key": key.public() });
    for step in ["lookup", "approve"] {
        let refused = server.post(&owner, &path(step), &real).await;
        refused.error(ErrorCode::RateLimited);
        assert_eq!(refused.retry_after.as_deref(), Some("600"));
    }
    assert_eq!(server.count("device_links").await, 1);
    server
        .post(&other, &path("lookup"), &real)
        .await
        .ok::<Value>(StatusCode::OK);

    server.clock.advance(Duration::minutes(10));
    let again = request_link(&server, &key, "laptop").await;
    let real = json!({ "code": again.code, "public_key": key.public() });
    server
        .post(&owner, &path("approve"), &real)
        .await
        .ok::<Device>(StatusCode::CREATED);
}

/// ADR 0108: a party that knows the public key of a device asks for a challenge of that
/// key. The challenge that the device waits to answer stays valid, so the party cannot make
/// the logins of the device fail.
#[tokio::test]
async fn a_challenge_request_of_another_party_does_not_end_a_waiting_challenge() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let login = ChallengeOperation::Login;
    let proof_of = |challenge: &junto_protocol::auth::Challenge| Proof {
        challenge_id: challenge.id,
        origin: origin(),
        signature: owner.key.sign(challenge),
    };
    let challenge = server.challenge(&owner.key, login).await;
    for _ in 0..3 {
        assert_eq!(server.challenge(&owner.key, login).await, challenge);
    }
    let stored: i64 =
        sqlx::query_scalar("select count(*) from challenges where operation = 'login'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(stored, 1);
    server
        .open(
            "/api/v1/auth/sessions",
            &json!({ "proof": proof_of(&challenge) }),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);

    // A used challenge is not given again. A challenge in the second half of its lifetime
    // is replaced, so that a device does not get one that is about to expire.
    let next = server.challenge(&owner.key, login).await;
    assert_ne!(next.id, challenge.id);
    server.clock.advance(Duration::seconds(30));
    assert_eq!(server.challenge(&owner.key, login).await, next);
    server.clock.advance(Duration::seconds(1));
    let fresh = server.challenge(&owner.key, login).await;
    assert_ne!(fresh.id, next.id);
    assert_eq!(
        fresh.expires_at - server.clock.now().replace_nanosecond(0).unwrap(),
        Duration::seconds(60)
    );
    server
        .open(
            "/api/v1/auth/sessions",
            &json!({ "proof": proof_of(&next) }),
        )
        .await
        .error(ErrorCode::ChallengeInvalid);
}

/// ADR 0009: a step of the maintenance task that fails does not keep the later steps from
/// running in the same pass. The trim of the event log fails here; the expiry of an upload
/// and the release of its reservation run, and the pass reports the failure.
#[tokio::test]
async fn a_failing_step_of_maintenance_does_not_stop_the_later_steps() {
    let files = Files::open().await;
    let server = &files.server;
    files.start(b"pending").await;
    assert_eq!(server.count("storage_reservations").await, 1);
    let rename = |from: &str, to: &str| {
        let statement = format!("alter table event_stream rename column {from} to {to}");
        async move {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&server.db)
                .await
                .unwrap();
        }
    };
    rename("trimmed_through", "trimmed_through_moved").await;

    server.clock.advance(Duration::hours(25));
    assert!(maintenance::run_once(&server.state).await.is_err());
    assert_eq!(server.count("storage_reservations").await, 0);
    assert_eq!(server.count("attachments").await, 0);

    rename("trimmed_through_moved", "trimmed_through").await;
    maintenance::run_once(&server.state).await.unwrap();
}
