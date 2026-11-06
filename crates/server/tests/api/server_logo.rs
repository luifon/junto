use crate::{discovery_support::events, support::TestServer};
use junto_protocol::access_token::TokenAction;
use junto_protocol::{
    account::Avatar,
    audit::AuditPage,
    error::ErrorCode,
    events::{Event, Level},
    server::ServerInfo,
};
use junto_server::image::samples;
use reqwest::{Method, StatusCode};
use serde_json::json;

const ROUTE: &str = "/api/v1/server/logo";

#[tokio::test]
async fn the_logo_is_public_and_changes_are_authorized_and_atomic() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let removed = server.join(&owner, "Removed").await;
    let banned = server.join(&owner, "Banned").await;
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", removed.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(
            &owner,
            &format!("/api/v1/accounts/{}/ban", banned.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let bytes = samples::png(8, 8);
    server
        .send::<serde_json::Value>(Method::GET, ROUTE, None, None, None)
        .await
        .error(ErrorCode::NotFound);
    server
        .put_bytes(&plain, ROUTE, None, bytes.clone())
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&plain, ROUTE)
        .await
        .error(ErrorCode::Forbidden);
    let token = server.access_token(&owner, TokenAction::ALL).await;
    for method in [Method::PUT, Method::DELETE] {
        server
            .with_token(method, ROUTE, &token, Some(&json!({})))
            .await
            .error(ErrorCode::Forbidden);
    }
    let before = server.snapshot(&plain).await.cursor;
    let logo: Avatar = server
        .put_bytes(&administrator, ROUTE, Some("text/html"), bytes.clone())
        .await
        .ok(StatusCode::OK);
    assert_eq!(logo.width, 8);
    let info: ServerInfo = server
        .send::<serde_json::Value>(Method::GET, "/api/server", None, None, None)
        .await
        .ok(StatusCode::OK);
    assert_eq!(info.logo_path.as_deref(), Some(ROUTE));
    let reply = server
        .send::<serde_json::Value>(Method::GET, ROUTE, None, None, None)
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    let image = reqwest::get(format!("{}{ROUTE}", server.base))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(image.as_ref(), bytes.as_slice());
    for former in [&removed, &banned] {
        assert_eq!(server.get(former, ROUTE).await.status, StatusCode::OK);
    }
    let (_, headers, _) = reply.observable();
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "image/png")
    );
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "etag" && value.contains(&logo.hash))
    );
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "x-content-type-options" && value == "nosniff")
    );
    assert_eq!(
        events(&server, &plain, before, Level::Background).await,
        [Event::ServerLogoChanged {
            logo: Some(logo.clone())
        }]
    );
    let before = server.snapshot(&plain).await.cursor;
    server
        .put_bytes(&owner, ROUTE, None, bytes)
        .await
        .ok::<Avatar>(StatusCode::OK);
    assert_eq!(server.snapshot(&plain).await.cursor, before);
    let audit: AuditPage = server
        .get(&owner, "/api/v1/audit-log?action=server_logo_changed")
        .await
        .ok(StatusCode::OK);
    assert_eq!(audit.entries.len(), 1);
    server
        .delete(&administrator, ROUTE)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        events(&server, &plain, before, Level::Full).await,
        [Event::ServerLogoChanged { logo: None }]
    );
    server
        .send::<serde_json::Value>(Method::GET, ROUTE, None, None, None)
        .await
        .error(ErrorCode::NotFound);
    let before = server.snapshot(&plain).await.cursor;
    server
        .delete(&owner, ROUTE)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.snapshot(&plain).await.cursor, before);
    let info: ServerInfo = server
        .send::<serde_json::Value>(Method::GET, "/api/server", None, None, None)
        .await
        .ok(StatusCode::OK);
    assert!(info.logo_path.is_none());
}

#[tokio::test]
async fn logo_uploads_use_avatar_limits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    for bytes in [samples::png(1025, 1), b"not an image".to_vec()] {
        server
            .put_bytes(&owner, ROUTE, None, bytes)
            .await
            .error(ErrorCode::ImageUnsupported);
    }
    // A logo of 2 MiB is stored; a server with a smaller limit refuses a larger one.
    let mut bytes = samples::png(1, 1);
    bytes.resize(2 * 1024 * 1024, 0);
    server
        .put_bytes(&owner, ROUTE, None, bytes)
        .await
        .ok::<Avatar>(StatusCode::OK);
    let smaller = TestServer::start_with(|config| config.limits.avatar_bytes = 256 * 1024).await;
    let smaller_owner = smaller.owner().await;
    let mut bytes = samples::png(1, 1);
    bytes.resize(256 * 1024 + 1, 0);
    smaller
        .put_bytes(&smaller_owner, ROUTE, None, bytes)
        .await
        .error(ErrorCode::ImageTooLarge);
    // With the logo of 2 MiB above, 30 changes reach the limit.
    for i in 0..29 {
        server
            .put_bytes(&owner, ROUTE, None, samples::png(i + 1, 1))
            .await
            .ok::<Avatar>(StatusCode::OK);
    }
    server
        .delete(&owner, ROUTE)
        .await
        .error(ErrorCode::RateLimited);
}

/// The requests of a logo that a client sends again after its connection was cut: the client
/// names the logo that it read before its first attempt (ADR 0119).
#[tokio::test]
async fn a_logo_sent_again_after_a_cut_does_not_replace_a_logo_stored_in_between() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let other = server.administrator(&owner, "Other").await;
    let (blue, red) = (samples::png(8, 8), samples::png(9, 9));
    let changes = async || {
        let audit: AuditPage = server
            .get(&owner, "/api/v1/audit-log?action=server_logo_changed")
            .await
            .ok(StatusCode::OK);
        audit.entries.len()
    };
    let seen_none = format!("{ROUTE}?before=none");

    // The first attempt is stored, and its answer is lost; the attempt again changes nothing.
    let blue_logo: Avatar = server
        .put_bytes(&owner, &seen_none, None, blue.clone())
        .await
        .ok(StatusCode::OK);
    let cursor = server.snapshot(&plain).await.cursor;
    let again: Avatar = server
        .put_bytes(&owner, &seen_none, None, blue.clone())
        .await
        .ok(StatusCode::OK);
    assert_eq!(again, blue_logo);
    assert_eq!(server.snapshot(&plain).await.cursor, cursor);
    assert_eq!(changes().await, 1);

    // Another administrator stores a logo; the attempt again is refused and changes nothing.
    let red_logo: Avatar = server
        .put_bytes(&other, ROUTE, None, red.clone())
        .await
        .ok(StatusCode::OK);
    let cursor = server.snapshot(&plain).await.cursor;
    server
        .put_bytes(&owner, &seen_none, None, blue.clone())
        .await
        .error(ErrorCode::Conflict);
    let image = reqwest::get(format!("{}{ROUTE}", server.base))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(image.as_ref(), red.as_slice());
    assert_eq!(server.snapshot(&plain).await.cursor, cursor);
    assert_eq!(changes().await, 2);
    server
        .put_bytes(
            &owner,
            &format!("{ROUTE}?before={}", blue_logo.hash),
            None,
            blue.clone(),
        )
        .await
        .error(ErrorCode::Conflict);

    // A client that read the red logo replaces it.
    let seen_red = format!("{ROUTE}?before={}", red_logo.hash);
    server
        .put_bytes(&owner, &seen_red, None, blue.clone())
        .await
        .ok::<Avatar>(StatusCode::OK);
    assert_eq!(changes().await, 3);
    for wrong in [
        "",
        "None",
        &red_logo.hash.to_uppercase(),
        &red_logo.hash[..63],
        "g".repeat(64).as_str(),
    ] {
        server
            .put_bytes(
                &owner,
                &format!("{ROUTE}?before={wrong}"),
                None,
                red.clone(),
            )
            .await
            .error(ErrorCode::InvalidRequest);
    }
}
