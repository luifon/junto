//! The logo of a workspace (ADR 0122): a server administrator sets and removes it, a person
//! who sees the workspace reads it, and the members get the workspace with its logo.

use crate::{discovery_support::events, support::TestServer};
use junto_protocol::{
    account::Avatar,
    audit::AuditPage,
    error::ErrorCode,
    events::{Event, Level},
    ids::WorkspaceId,
    structure::Workspace,
};
use junto_server::image::samples;
use reqwest::StatusCode;
use serde_json::json;

fn route(workspace_id: WorkspaceId) -> String {
    format!("/api/v1/workspaces/{workspace_id}/logo")
}

#[tokio::test]
async fn an_administrator_sets_and_removes_the_logo_of_a_workspace_that_its_members_read() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let workspaces: Vec<Workspace> = server
        .get(&plain, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    let default = workspaces[0].id;
    assert_eq!(workspaces[0].logo, None);
    // A workspace that the plain member does not see.
    let lab: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Lab" }))
        .await
        .ok(StatusCode::CREATED);
    let bytes = samples::png(8, 8);

    server
        .get(&plain, &route(default))
        .await
        .error(ErrorCode::NotFound);
    server
        .put_bytes(&plain, &route(default), None, bytes.clone())
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&plain, &route(default))
        .await
        .error(ErrorCode::Forbidden);
    server
        .put_bytes(&owner, &route(WorkspaceId::generate()), None, bytes.clone())
        .await
        .error(ErrorCode::NotFound);

    let before = server.snapshot(&plain).await.cursor;
    let logo: Avatar = server
        .put_bytes(
            &administrator,
            &route(default),
            Some("text/html"),
            bytes.clone(),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!((logo.width, logo.height), (8, 8));
    let workspaces: Vec<Workspace> = server
        .get(&plain, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces[0].logo.as_ref(), Some(&logo));
    let snapshot = server.snapshot(&plain).await;
    assert_eq!(snapshot.workspaces[0].workspace.logo.as_ref(), Some(&logo));
    let changed = events(&server, &plain, before, Level::Full).await;
    assert_eq!(
        changed,
        [Event::WorkspaceUpdated {
            workspace: Workspace {
                logo: Some(logo.clone()),
                ..workspaces[0].clone()
            }
        }]
    );
    let reply = server.get(&plain, &route(default)).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.bytes(), bytes.as_slice());
    let (_, headers, _) = reply.observable();
    for (name, value) in [
        ("content-type", "image/png"),
        ("x-content-type-options", "nosniff"),
        ("cache-control", "private, no-cache"),
    ] {
        assert!(
            headers.iter().any(|(got, set)| got == name && set == value),
            "{name}: {headers:?}"
        );
    }
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "etag" && value.contains(&logo.hash))
    );

    // The same image again changes nothing and tells nobody.
    let before = server.snapshot(&plain).await.cursor;
    server
        .put_bytes(&owner, &route(default), None, bytes.clone())
        .await
        .ok::<Avatar>(StatusCode::OK);
    assert_eq!(server.snapshot(&plain).await.cursor, before);
    let audit: AuditPage = server
        .get(&owner, "/api/v1/audit-log?action=workspace_logo_changed")
        .await
        .ok(StatusCode::OK);
    assert_eq!(audit.entries.len(), 1);

    // The logo of a workspace that a person does not see is not read.
    server
        .put_bytes(&owner, &route(lab.id), None, samples::png(4, 4))
        .await
        .ok::<Avatar>(StatusCode::OK);
    server
        .get(&plain, &route(lab.id))
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        server.get(&owner, &route(lab.id)).await.status,
        StatusCode::OK
    );

    server
        .delete(&administrator, &route(default))
        .await
        .empty(StatusCode::NO_CONTENT);
    let after = events(&server, &plain, before, Level::Full).await;
    assert_eq!(
        after,
        [Event::WorkspaceUpdated {
            workspace: Workspace {
                logo: None,
                ..workspaces[0].clone()
            }
        }]
    );
    server
        .get(&plain, &route(default))
        .await
        .error(ErrorCode::NotFound);
    let before = server.snapshot(&plain).await.cursor;
    server
        .delete(&owner, &route(default))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.snapshot(&plain).await.cursor, before);
}

#[tokio::test]
async fn a_workspace_logo_uses_the_limits_of_an_avatar() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    let default = workspaces[0].id;
    for bytes in [
        samples::png(1025, 1),
        samples::animated_png(8, 8),
        b"not an image".to_vec(),
    ] {
        server
            .put_bytes(&owner, &route(default), None, bytes)
            .await
            .error(ErrorCode::ImageUnsupported);
    }
    let smaller = TestServer::start_with(|config| config.limits.avatar_bytes = 256 * 1024).await;
    let smaller_owner = smaller.owner().await;
    let workspaces: Vec<Workspace> = smaller
        .get(&smaller_owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    let mut bytes = samples::png(1, 1);
    bytes.resize(256 * 1024 + 1, 0);
    smaller
        .put_bytes(&smaller_owner, &route(workspaces[0].id), None, bytes)
        .await
        .error(ErrorCode::ImageTooLarge);
}
