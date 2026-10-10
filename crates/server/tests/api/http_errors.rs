use junto_protocol::error::ErrorCode;
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::structure::{Channel, ChannelOverride, Workspace};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::structure::set_override;
use crate::support::{Member, Reply, TestServer};

#[tokio::test]
async fn an_unknown_path_and_a_wrong_method_get_the_error_body() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    for path in [
        "/",
        "/api",
        "/api/v1/nothing",
        "/api/v2/account",
        "/api/v1/account/",
    ] {
        let reply = server.get(&owner, path).await;
        reply.error(ErrorCode::NotFound);
        let anonymous = server
            .send::<Value>(Method::GET, path, None, None, None)
            .await;
        assert_eq!(anonymous.observable(), reply.observable(), "{path}");
    }

    for (method, path) in [
        (Method::PUT, "/api/server"),
        (Method::DELETE, "/api/v1/account"),
        (Method::GET, "/api/v1/join"),
        (Method::POST, "/api/v1/audit-log"),
    ] {
        let reply = server
            .send::<Value>(method.clone(), path, Some(&owner.token), None, None)
            .await;
        assert_eq!(
            reply.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path}"
        );
        reply.error(ErrorCode::MethodNotAllowed);
    }
}

#[tokio::test]
async fn a_request_that_takes_too_long_gets_the_error_body() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let strict = server
        .strict(|config| config.limits.request_timeout = Duration::milliseconds(300))
        .await;

    strict.gate().arm(0);
    let reply = strict
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Slow" }))
        .await;
    strict.gate().release();
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    reply.error(ErrorCode::Timeout);

    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
}

#[tokio::test]
async fn malformed_requests_get_the_error_body() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let http = reqwest::Client::new();
    let url = format!("{}/api/v1/workspaces", server.base);

    let cases = [
        http.post(&url).bearer_auth(&owner.token).body("{not json"),
        http.post(&url)
            .bearer_auth(&owner.token)
            .header("content-type", "application/json")
            .body("{not json"),
        http.post(&url)
            .bearer_auth(&owner.token)
            .header("content-type", "application/json")
            .body(format!("{{\"name\":\"{}\"}}", "x".repeat(3 * 1024 * 1024))),
        http.get(format!(
            "{}/api/v1/channels/not-an-id/overrides",
            server.base
        ))
        .bearer_auth(&owner.token),
    ];
    for request in cases {
        let response = request.send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert!(status.is_client_error(), "{status}");
        assert_eq!(body["error"]["code"], "invalid_request", "{body}");
    }
}

/// The server reads a body that is too large to its end before it answers. A server that
/// answered and closed while the client still sent would lose the response for some clients:
/// the client then sees a broken connection, not the error body.
#[tokio::test]
async fn a_body_that_is_too_large_is_read_to_its_end_and_answered_with_the_error_body() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let url = format!("{}/api/v1/workspaces", server.base);
    for megabytes in [3, 5, 8, 12] {
        let http = reqwest::Client::new();
        let response = http
            .post(&url)
            .bearer_auth(&owner.token)
            .header("content-type", "application/json")
            .body(format!(
                "{{\"name\":\"{}\"}}",
                "x".repeat(megabytes * 1024 * 1024)
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("{megabytes} MiB: {error:?}"));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_request", "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("2097152 bytes"),
            "{body}"
        );
    }
    let at_the_limit = format!("{{\"name\":\"{}\"}}", "x".repeat(2 * 1024 * 1024 - 11));
    assert_eq!(at_the_limit.len(), 2 * 1024 * 1024);
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&owner.token)
        .header("content-type", "application/json")
        .body(at_the_limit)
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("longer than 64 characters"),
        "{body}"
    );

    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
}

async fn replies(
    server: &TestServer,
    caller: &Member,
    requests: &[(Method, String, Value)],
) -> Vec<Reply> {
    let mut replies = Vec::new();
    for (method, path, body) in requests {
        let body = (!body.is_null()).then_some(body);
        replies.push(
            server
                .send(method.clone(), path, Some(&caller.token), body, None)
                .await,
        );
    }
    replies
}

#[tokio::test]
async fn an_absent_resource_and_one_the_caller_may_not_see_get_identical_responses() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let default_workspace = server.default_workspace(&owner).await;
    let caller = server.join(&owner, "Caller").await;
    server
        .grant_workspace(
            &owner,
            &caller,
            default_workspace.id,
            WorkspacePermission::ALL,
        )
        .await;
    server
        .grant_server(&owner, &caller, &[ServerPermission::CreateInvites])
        .await;

    let foreign: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Foreign" }))
        .await
        .ok(StatusCode::CREATED);
    let foreign_role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", foreign.id),
            &json!({ "name": "role" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let foreign_channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", foreign.id),
            &json!({ "name": "elsewhere", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);

    let everyone = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", default_workspace.id),
        )
        .await
        .ok::<Vec<WorkspaceRole>>(StatusCode::OK)
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap();
    let hidden: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", default_workspace.id),
            &json!({ "name": "hidden", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    set_override(
        &server,
        &owner,
        hidden.id,
        everyone.id,
        &[],
        &[WorkspacePermission::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    let deleted: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", default_workspace.id),
            &json!({ "name": "gone", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .delete(&owner, &format!("/api/v1/channels/{}", deleted.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    let absent = uuid::Uuid::now_v7();
    let workspace_requests = |workspace: String| -> Vec<(Method, String, Value)> {
        let role = foreign_role.id;
        let account = owner.account_id;
        vec![
            (
                Method::GET,
                format!("/api/v1/workspaces/{workspace}/members"),
                Value::Null,
            ),
            (
                Method::GET,
                format!("/api/v1/workspaces/{workspace}/roles"),
                Value::Null,
            ),
            (
                Method::GET,
                format!("/api/v1/workspaces/{workspace}/channels"),
                Value::Null,
            ),
            (
                Method::GET,
                format!("/api/v1/workspaces/{workspace}/audit-log"),
                Value::Null,
            ),
            (
                Method::POST,
                format!("/api/v1/workspaces/{workspace}/roles"),
                json!({ "name": "mine" }),
            ),
            (
                Method::PATCH,
                format!("/api/v1/workspaces/{workspace}/roles/{role}"),
                json!({ "name": "mine" }),
            ),
            (
                Method::PUT,
                format!("/api/v1/workspaces/{workspace}/members/{account}/roles/{role}"),
                Value::Null,
            ),
            (
                Method::DELETE,
                format!("/api/v1/workspaces/{workspace}/members/{account}/roles/{role}"),
                Value::Null,
            ),
            (
                Method::POST,
                format!("/api/v1/workspaces/{workspace}/categories"),
                json!({ "name": "mine" }),
            ),
            (
                Method::POST,
                format!("/api/v1/workspaces/{workspace}/channels"),
                json!({ "name": "mine", "kind": "text" }),
            ),
            (
                Method::POST,
                "/api/v1/invites".to_owned(),
                json!({ "workspace_ids": [workspace] }),
            ),
        ]
    };
    let channel_requests = |channel: String| -> Vec<(Method, String, Value)> {
        let role = everyone.id;
        vec![
            (
                Method::PATCH,
                format!("/api/v1/channels/{channel}"),
                json!({ "name": "mine" }),
            ),
            (
                Method::DELETE,
                format!("/api/v1/channels/{channel}"),
                Value::Null,
            ),
            (
                Method::GET,
                format!("/api/v1/channels/{channel}/overrides"),
                Value::Null,
            ),
            (
                Method::PUT,
                format!("/api/v1/channels/{channel}/overrides/{role}"),
                json!({ "allow": ["view_channel"] }),
            ),
        ]
    };

    let for_absent = replies(&server, &caller, &workspace_requests(absent.to_string())).await;
    let for_foreign = replies(
        &server,
        &caller,
        &workspace_requests(foreign.id.to_string()),
    )
    .await;
    for (absent_reply, foreign_reply) in for_absent.iter().zip(&for_foreign) {
        absent_reply.error(ErrorCode::NotFound);
        assert_eq!(absent_reply.observable(), foreign_reply.observable());
    }

    let for_absent = replies(&server, &caller, &channel_requests(absent.to_string())).await;
    for channel in [hidden.id, foreign_channel.id, deleted.id] {
        let for_channel = replies(&server, &caller, &channel_requests(channel.to_string())).await;
        for (absent_reply, channel_reply) in for_absent.iter().zip(&for_channel) {
            absent_reply.error(ErrorCode::NotFound);
            assert_eq!(absent_reply.observable(), channel_reply.observable());
        }
    }
}

/// Opens a chunked request, sends `bytes` of its body and withholds the end of the body.
async fn slow_body(
    server: &TestServer,
    request_line: &str,
    forwarded_for: Option<&str>,
    bytes: usize,
) -> tokio::net::TcpStream {
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    let forwarded = forwarded_for
        .map(|address| format!("x-forwarded-for: {address}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "{request_line} HTTP/1.1\r\nhost: chat.example.test\r\n{forwarded}\
         content-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let chunk = vec![b' '; 16 * 1024];
    let mut sent = 0;
    while sent < bytes {
        let size = chunk.len().min(bytes - sent);
        let framed = [format!("{size:x}\r\n").as_bytes(), &chunk[..size], b"\r\n"].concat();
        if stream.write_all(&framed).await.is_err() {
            break;
        }
        sent += size;
    }
    let _ = stream.flush().await;
    stream
}

/// The first line of the response, when one arrives within two seconds.
async fn early_answer(stream: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0u8; 512];
    let read = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buffer));
    match read.await {
        Ok(Ok(count)) if count > 0 => {
            let text = String::from_utf8_lossy(&buffer[..count]).into_owned();
            text.lines().next().map(str::to_owned)
        }
        _ => None,
    }
}

/// One address cannot make the server hold more than a bounded number of bytes of request
/// bodies: a further slow request is refused while it still sends.
#[tokio::test]
async fn slow_requests_of_one_address_hold_a_bounded_number_of_body_bytes() {
    let server = TestServer::start().await;
    let nearly_full = 2 * 1024 * 1024 - 64 * 1024;
    let mut held = Vec::new();
    for _ in 0..2 {
        let mut stream = slow_body(&server, "POST /api/v1/workspaces", None, nearly_full).await;
        assert_eq!(early_answer(&mut stream).await, None);
        held.push(stream);
    }
    for _ in 0..3 {
        let mut refused = slow_body(&server, "POST /api/v1/workspaces", None, nearly_full).await;
        let answer = early_answer(&mut refused).await;
        assert_eq!(answer.as_deref(), Some("HTTP/1.1 429 Too Many Requests"));
    }
    drop(held);
}

/// Many slow requests from many addresses hold at most the bytes that the server may hold
/// for all requests together. A further one is refused while it still sends, a request
/// without a body in this API holds nothing, and the bytes are given back.
#[tokio::test]
async fn many_slow_requests_hold_a_bounded_number_of_body_bytes() {
    const BODY: usize = 192 * 1024;
    const HELD: usize = 5;
    let server = TestServer::start_with(|config| {
        config.trusted_proxies = vec![std::net::IpAddr::from([127, 0, 0, 1])];
        config.limits.request_body_bytes = 256 * 1024;
        config.limits.request_bodies_held_per_address = 256 * 1024;
        config.limits.request_bodies_held_bytes = 1024 * 1024;
    })
    .await;
    let bodies = server.state.bodies.clone();
    let settled = async |bytes: usize| {
        for _ in 0..800 {
            if bodies.held() == bytes {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the server holds {} bytes, not {bytes}", bodies.held());
    };

    let mut held = Vec::new();
    for number in 0..HELD {
        let address = format!("198.51.100.{}", number + 1);
        held.push(slow_body(&server, "POST /api/v1/workspaces", Some(&address), BODY).await);
        settled((number + 1) * BODY).await;
    }
    for number in 0..40 {
        let address = format!("203.0.113.{}", number + 1);
        let mut refused = slow_body(&server, "POST /api/v1/workspaces", Some(&address), BODY).await;
        let answer = early_answer(&mut refused).await;
        assert_eq!(answer.as_deref(), Some("HTTP/1.1 503 Service Unavailable"));
        assert!(bodies.held() <= 1024 * 1024);
    }
    settled(HELD * BODY).await;
    assert_eq!(bodies.addresses(), HELD);

    let mut without_a_body = Vec::new();
    for number in 0..20 {
        let address = format!("192.0.2.{}", number + 1);
        without_a_body.push(slow_body(&server, "GET /api/v1/health", Some(&address), BODY).await);
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!((bodies.held(), bodies.addresses()), (HELD * BODY, HELD));
    server
        .send::<Value>(Method::GET, "/api/v1/health", None, None, None)
        .await
        .ok::<Value>(StatusCode::OK);

    drop((held, without_a_body));
    settled(0).await;
    assert_eq!(bodies.addresses(), 0);
}
