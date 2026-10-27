use crate::support::{Member, TestServer};
use junto_protocol::WorkspacePermission as W;
use junto_protocol::structure::{Category, Channel, ChannelList, Workspace};
use reqwest::StatusCode;
use serde_json::json;

async fn fixture() -> (TestServer, Member, Member, Workspace, Category) {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let category: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/categories", workspace.id),
            &json!({"name":"Plans"}),
        )
        .await
        .ok(StatusCode::CREATED);
    (server, owner, reader, workspace, category)
}

#[tokio::test]
async fn a_category_needs_a_visible_child_or_management_permission() {
    let (server, owner, reader, workspace, category) = fixture().await;
    let route = format!("/api/v1/workspaces/{}/channels", workspace.id);
    let before: ChannelList = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert!(!before.categories.iter().any(|held| held.id == category.id));
    let _: Channel = server
        .post(
            &owner,
            &route,
            &json!({"name":"Notes","kind":"text","category_id":category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let after: ChannelList = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert!(after.categories.iter().any(|held| held.id == category.id));
    sqlx::query("insert into category_overrides(category_id,role_id,workspace_id,allow,deny) select $1,id,workspace_id,0,1 from workspace_roles where workspace_id=$2 and is_everyone")
        .bind(category.id).bind(workspace.id).execute(&server.db).await.unwrap();
    let hidden: ChannelList = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert!(!hidden.categories.iter().any(|held| held.id == category.id));
    let manager: ChannelList = server.get(&owner, &route).await.ok(StatusCode::OK);
    assert!(manager.categories.iter().any(|held| held.id == category.id));
}

#[tokio::test]
async fn a_workspace_manager_cannot_rename_a_category_hidden_from_them() {
    let (server, owner, reader, workspace, category) = fixture().await;
    server
        .grant_workspace(&owner, &reader, workspace.id, &[W::ManageChannel])
        .await;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    let hidden = format!(
        "/api/v1/workspaces/{}/categories/{}",
        workspace.id, category.id
    );
    let absent = format!(
        "/api/v1/workspaces/{}/categories/{}",
        workspace.id,
        junto_protocol::ids::CategoryId::generate()
    );
    let mut replies = Vec::new();
    for route in [hidden, absent] {
        let reply = server
            .patch(&reader, &route, &json!({"name":"Changed"}))
            .await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        replies.push(reply.observable());
    }
    assert_eq!(replies[0], replies[1]);
}

#[tokio::test]
async fn a_workspace_manager_cannot_delete_a_category_hidden_from_them() {
    let (server, owner, reader, workspace, category) = fixture().await;
    server
        .grant_workspace(&owner, &reader, workspace.id, &[W::ManageChannel])
        .await;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    let hidden = format!(
        "/api/v1/workspaces/{}/categories/{}",
        workspace.id, category.id
    );
    let absent = format!(
        "/api/v1/workspaces/{}/categories/{}",
        workspace.id,
        junto_protocol::ids::CategoryId::generate()
    );
    let mut replies = Vec::new();
    for route in [hidden, absent] {
        let reply = server.delete(&reader, &route).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        replies.push(reply.observable());
    }
    assert_eq!(replies[0], replies[1]);
}

#[tokio::test]
async fn a_person_can_manage_one_category_without_workspace_management() {
    let (server, _, reader, workspace, category) = fixture().await;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,4,0)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    let route = format!(
        "/api/v1/workspaces/{}/categories/{}",
        workspace.id, category.id
    );
    let changed: Category = server
        .patch(&reader, &route, &json!({"name":"Changed"}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed.name, "Changed");
    let listed: ChannelList = server
        .get(
            &reader,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(listed.categories.iter().any(|held| held.id == category.id));
}

async fn hidden_destination(moving: bool) {
    let (server, owner, reader, workspace, category) = fixture().await;
    server
        .grant_workspace(&owner, &reader, workspace.id, &[W::ManageChannel])
        .await;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    let create = format!("/api/v1/workspaces/{}/channels", workspace.id);
    let channel: Channel = server
        .post(&owner, &create, &json!({"name":"Notes","kind":"text"}))
        .await
        .ok(StatusCode::CREATED);
    let mut replies = Vec::new();
    for target in [category.id, junto_protocol::ids::CategoryId::generate()] {
        let reply = if moving {
            server
                .patch(
                    &reader,
                    &format!("/api/v1/channels/{}", channel.id),
                    &json!({"category_id":target}),
                )
                .await
        } else {
            server
                .post(
                    &reader,
                    &create,
                    &json!({"name":"New","kind":"text","category_id":target}),
                )
                .await
        };
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        replies.push(reply.observable());
    }
    assert_eq!(replies[0], replies[1]);
}

#[tokio::test]
async fn creating_a_channel_does_not_probe_hidden_categories() {
    hidden_destination(false).await;
}

#[tokio::test]
async fn moving_a_channel_does_not_probe_hidden_categories() {
    hidden_destination(true).await;
}

#[tokio::test]
async fn a_category_manager_can_create_a_channel_in_that_category() {
    let (server, _, reader, workspace, category) = fixture().await;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,4,0)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    let channel: Channel = server
        .post(
            &reader,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"New","kind":"text","category_id":category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(channel.category_id, Some(category.id));
}
