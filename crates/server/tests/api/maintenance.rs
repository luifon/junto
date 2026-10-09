use junto_protocol::auth::ChallengeOperation;
use junto_protocol::invite::{JoinRequest, JoinRequestState, JoinRequestStatus};
use junto_server::maintenance;
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

use crate::support::{DeviceKey, TestServer};

async fn rows(server: &TestServer, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select count(*) from {table}")))
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn counts(server: &TestServer) -> [i64; 4] {
    [
        rows(server, "challenges").await,
        rows(server, "device_links").await,
        rows(server, "sessions").await,
        rows(server, "join_requests").await,
    ]
}

#[tokio::test]
async fn maintenance_removes_what_has_expired_and_keeps_the_rest() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    server
        .challenge(&DeviceKey::generate(), ChallengeOperation::Join)
        .await;
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::LinkDevice)
        .await;
    server
        .open("/api/v1/device-links", &json!({ "proof": proof }))
        .await
        .ok::<serde_json::Value>(StatusCode::CREATED);
    let waiting = DeviceKey::generate();
    let proof = server
        .proof(&waiting, ChallengeOperation::RequestToJoin)
        .await;
    server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Waiting" }),
        )
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    let denied: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Denied" }),
        )
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

    let before = counts(&server).await;
    assert_eq!(before, [5, 1, 1, 2]);
    maintenance::run_once(&server.state).await.unwrap();
    assert_eq!(counts(&server).await, before);

    server.clock.advance(Duration::seconds(61));
    maintenance::run_once(&server.state).await.unwrap();
    assert_eq!(counts(&server).await, [0, 1, 1, 2]);

    server.clock.advance(Duration::minutes(10));
    maintenance::run_once(&server.state).await.unwrap();
    assert_eq!(counts(&server).await, [0, 0, 1, 2]);

    server.clock.advance(Duration::days(8));
    maintenance::run_once(&server.state).await.unwrap();
    let states: Vec<String> = sqlx::query_scalar("select state from join_requests order by state")
        .fetch_all(&server.db)
        .await
        .unwrap();
    assert_eq!(states, ["denied", "expired"]);
    let proof = server
        .proof(&waiting, ChallengeOperation::JoinRequestStatus)
        .await;
    let status: JoinRequestStatus = server
        .open("/api/v1/join-requests/status", &json!({ "proof": proof }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(status.state, JoinRequestState::Expired);
    server
        .get(&owner, "/api/v1/account")
        .await
        .ok::<serde_json::Value>(StatusCode::OK);

    server.clock.advance(Duration::days(30));
    maintenance::run_once(&server.state).await.unwrap();
    assert_eq!(counts(&server).await, [0, 0, 0, 0]);
}

#[tokio::test]
async fn maintenance_works_in_batches_that_each_commit() {
    let server = TestServer::start_with(|config| {
        config.limits.maintenance_batch = 2;
    })
    .await;
    for _ in 0..5 {
        server
            .challenge(&DeviceKey::generate(), ChallengeOperation::Join)
            .await;
    }
    server.clock.advance(Duration::seconds(61));

    server.gate().arm(1);
    let run = maintenance::run_once(&server.state);
    tokio::pin!(run);
    tokio::select! {
        biased;
        () = server.gate().arrived() => {}
        result = &mut run => panic!("maintenance finished in one transaction: {result:?}"),
    }
    assert_eq!(rows(&server, "challenges").await, 3);

    server.gate().release();
    run.await.unwrap();
    assert_eq!(rows(&server, "challenges").await, 0);
}
