use crate::support::TestServer;
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn server_and_workspace_roles_keep_colour_hoist_and_explicit_resets() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    for route in [
        "/api/v1/server-roles".to_owned(),
        format!("/api/v1/workspaces/{}/roles", workspace.id),
    ] {
        let role: Value = server
            .post(
                &owner,
                &route,
                &json!({"name":"Guides","permissions":[],"color":"#336699","hoist":true}),
            )
            .await
            .ok(StatusCode::CREATED);
        assert_eq!(role["color"], "#336699");
        assert_eq!(role["hoist"], true);
        let item = format!("{route}/{}", role["id"].as_str().unwrap());
        let renamed: Value = server
            .patch(&owner, &item, &json!({"name":"Editors"}))
            .await
            .ok(StatusCode::OK);
        assert_eq!(renamed["color"], "#336699");
        assert_eq!(renamed["hoist"], true);
        let reset: Value = server
            .patch(&owner, &item, &json!({"color":null,"hoist":false}))
            .await
            .ok(StatusCode::OK);
        assert!(reset["color"].is_null());
        assert_eq!(reset["hoist"], false);
    }
}

#[tokio::test]
async fn public_appearance_uses_highest_coloured_and_hoisted_roles_without_permissions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let person = server.join(&owner, "Person").await;
    let viewer = server.join(&owner, "Viewer").await;
    let mut ids = Vec::new();
    for (name, color, hoist) in [
        ("Guides", Some("#336699"), true),
        ("Readers", Some("#AABBCC"), false),
        ("Hosts", None, true),
    ] {
        let role: Value = server
            .post(
                &owner,
                "/api/v1/server-roles",
                &json!({"name":name,"permissions":["create_invites"],"color":color,"hoist":hoist}),
            )
            .await
            .ok(StatusCode::CREATED);
        let id = role["id"].as_str().unwrap().to_owned();
        server
            .put_empty(
                &owner,
                &format!("/api/v1/accounts/{}/server-roles/{id}", person.account_id),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        ids.push(id);
    }
    // New roles enter below the existing roles.
    let profile: Value = server
        .get(
            &viewer,
            &format!("/api/v1/accounts/{}/profile", person.account_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(profile["appearance"]["color"], "#336699");
    assert_eq!(profile["appearance"]["group"]["id"], ids[0]);
    assert!(!profile.to_string().contains("create_invites"));
    server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{}", ids[0]),
            &json!({"color":null,"hoist":false}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let updated: Value = server
        .get(
            &viewer,
            &format!("/api/v1/accounts/{}/profile", person.account_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(updated["appearance"]["color"], "#AABBCC");
    assert_eq!(updated["appearance"]["group"]["id"], ids[2]);
    let workspace = server.default_workspace(&owner).await;
    let members: Value = server
        .get(
            &viewer,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        members
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["account_id"] == person.account_id.to_string())
            .unwrap()["appearance"],
        updated["appearance"]
    );
    server
        .patch(
            &viewer,
            &format!("/api/v1/server-roles/{}", ids[0]),
            &json!({"color":"#FFFFFF","hoist":true}),
        )
        .await
        .error(junto_protocol::error::ErrorCode::Forbidden);
    for invalid in ["red", "#fff", "#00112233", "#GGGGGG"] {
        server
            .patch(
                &owner,
                &format!("/api/v1/server-roles/{}", ids[0]),
                &json!({"color":invalid}),
            )
            .await
            .error(junto_protocol::error::ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn everyone_cannot_be_hoisted_and_structure_export_keeps_appearance() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let roles: Value = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    let everyone = roles
        .as_array()
        .unwrap()
        .iter()
        .find_map(|row| {
            let role = row.get("role").unwrap_or(row);
            role["is_everyone"]
                .as_bool()
                .unwrap_or(false)
                .then_some(role["id"].as_str().unwrap())
        })
        .unwrap();
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}/roles/{everyone}", workspace.id),
            &json!({"hoist":true}),
        )
        .await
        .error(junto_protocol::error::ErrorCode::InvalidRequest);
    let role: Value = server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({"name":"Guides","permissions":[],"color":"#112233","hoist":true}),
        )
        .await
        .ok(StatusCode::CREATED);
    let export: Value = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let structure = export.get("structure").unwrap_or(&export);
    assert_eq!(structure["format"], 6);
    assert!(
        structure["server_roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == role["id"]
                && entry["color"] == "#112233"
                && entry["hoist"] == true)
    );
}

#[tokio::test]
async fn appearance_invalidations_are_opt_in_and_cover_the_snapshot_subscribe_gap() {
    use crate::stream_support::Frame;
    use junto_protocol::events::{ClientMessage, Level, ServerMessage};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let viewer = server.join(&owner, "Viewer").await;
    let cursor = server.snapshot(&viewer).await.cursor;
    let mut legacy = server.subscribed(&viewer, Level::Full).await;
    let role: Value = server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({"name":"Guides","permissions":[],"color":"#112233","hoist":true}),
        )
        .await
        .ok(StatusCode::CREATED);
    let mut stream = server.stream(&viewer).await;
    stream
        .send(&ClientMessage::Subscribe {
            level: Level::Full,
            cursor: Some(cursor),
            presence: false,
            appearance: true,
        })
        .await;
    assert!(matches!(
        stream.next().await,
        Frame::Message(ServerMessage::Subscribed { .. })
    ));
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::AppearanceChanged {})
    );
    let cursor = server.snapshot(&viewer).await.cursor;
    stream
        .send(&ClientMessage::Subscribe {
            level: Level::Full,
            cursor: Some(cursor),
            presence: false,
            appearance: true,
        })
        .await;
    assert!(matches!(
        stream.next().await,
        Frame::Message(ServerMessage::Subscribed { .. })
    ));
    // No unconditional invalidation on resubscribe: a reload must settle.
    assert_eq!(
        stream.within(std::time::Duration::from_millis(100)).await,
        None
    );
    server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{}", role["id"].as_str().unwrap()),
            &json!({"color":"#AABBCC"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::AppearanceChanged {})
    );
    assert_eq!(
        legacy.within(std::time::Duration::from_millis(100)).await,
        None
    );
}

#[tokio::test]
async fn reordered_role_events_keep_the_stored_colour_and_group_at_both_levels() {
    use junto_protocol::events::{Event, Level};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    for route in [
        "/api/v1/server-roles".to_owned(),
        format!("/api/v1/workspaces/{}/roles", workspace.id),
    ] {
        let mut ids = Vec::new();
        for name in ["Guides", "Editors"] {
            let role: Value = server
                .post(
                    &owner,
                    &route,
                    &json!({"name":name,"permissions":[],"color":"#336699","hoist":true}),
                )
                .await
                .ok(StatusCode::CREATED);
            ids.push(role["id"].as_str().unwrap().to_owned());
        }
        let mut stream = server.subscribed(&owner, Level::Full).await;
        server.patch(&owner,&format!("{route}/positions"),&json!({"positions":[{"role_id":ids[0],"position":1},{"role_id":ids[1],"position":2}]})).await.empty(StatusCode::NO_CONTENT);
        for _ in 0..2 {
            let (_, event) = stream.event().await;
            let role = match event {
                Event::ServerRoleUpdated { role } => serde_json::to_value(role).unwrap(),
                Event::WorkspaceRoleUpdated { role } => serde_json::to_value(role).unwrap(),
                other => panic!("a moved role: {other:?}"),
            };
            assert_eq!(role["color"], "#336699");
            assert_eq!(role["hoist"], true);
        }
    }
}
