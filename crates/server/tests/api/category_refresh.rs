use crate::support::TestServer;
use junto_protocol::events::{Event, Level};
use junto_protocol::structure::Channel;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn a_new_child_refreshes_the_category_and_its_effective_permissions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let category = server.category(&owner, workspace.id, "Plans", 0).await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    let _: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"Notes","kind":"text","category_id":category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let events = server.traffic(&reader, &mut stream).await;
    assert!(events.iter().any(|(_,event)|matches!(event,Event::PermissionsChanged{workspace_id:Some(id)} if *id==workspace.id)), "{events:?}");
}

#[tokio::test]
async fn moving_the_last_child_refreshes_people_who_lose_visibility() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let source = server.category(&owner, workspace.id, "Plans", 0).await;
    let destination = server.category(&owner, workspace.id, "Private", 1).await;
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"Notes","kind":"text","category_id":source.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(destination.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"category_id":destination.id}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let events = server.traffic(&reader, &mut stream).await;
    assert!(events.iter().any(|(_,event)|matches!(event,Event::PermissionsChanged{workspace_id:Some(id)} if *id==workspace.id)), "{events:?}");
    assert!(
        !events
            .iter()
            .any(|(_, event)| matches!(event, Event::ChannelUpdated { .. }))
    );
}
