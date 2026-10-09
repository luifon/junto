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
    let server = TestServer::start_with(|config| {
        config.limits.request_timeout = Duration::milliseconds(300);
    })
    .await;
    let owner = server.owner().await;

    server.gate().arm(0);
    let reply = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Slow" }))
        .await;
    server.gate().release();
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
