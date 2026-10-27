use crate::stream_support::Stream;
use crate::support::{Member, TestServer};
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

/// The owner, a reader who sees the only channel of a category, and a stream of the reader
/// that starts after both exist.
struct LastChild {
    server: TestServer,
    owner: Member,
    reader: Member,
    channel: Channel,
    stream: Stream,
}

async fn the_last_child() -> LastChild {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let category = server.category(&owner, workspace.id, "Plans", 0).await;
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"Notes","kind":"text","category_id":category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let stream = server.subscribed(&reader, Level::Full).await;
    LastChild {
        server,
        owner,
        reader,
        channel,
        stream,
    }
}

/// The reader got a refresh of the workspace, and its next snapshot lists the category no
/// longer.
async fn the_reader_loses_the_category(scene: &mut LastChild) {
    let events = scene.server.traffic(&scene.reader, &mut scene.stream).await;
    let workspace_id = scene.channel.workspace_id;
    assert!(
        events.iter().any(|(_, event)| matches!(
            event,
            Event::PermissionsChanged { workspace_id: Some(id) } if *id == workspace_id
        )),
        "{events:?}"
    );
    let snapshot = scene.server.snapshot(&scene.reader).await;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|held| held.workspace.id == workspace_id)
        .unwrap();
    assert!(
        workspace.categories.is_empty(),
        "{:?}",
        workspace.categories
    );
}

#[tokio::test]
async fn deleting_the_last_child_refreshes_people_who_lose_visibility() {
    let mut scene = the_last_child().await;
    scene
        .server
        .delete(
            &scene.owner,
            &format!("/api/v1/channels/{}", scene.channel.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    the_reader_loses_the_category(&mut scene).await;
}

#[tokio::test]
async fn archiving_the_last_child_refreshes_people_who_lose_visibility() {
    let mut scene = the_last_child().await;
    scene
        .server
        .put_empty(
            &scene.owner,
            &format!("/api/v1/channels/{}/archive", scene.channel.id),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    the_reader_loses_the_category(&mut scene).await;
}

#[tokio::test]
async fn a_category_whose_last_channel_is_archived_sends_its_new_name_only_to_managers() {
    let mut scene = the_last_child().await;
    let path = format!("/api/v1/channels/{}/archive", scene.channel.id);
    scene
        .server
        .put_empty(&scene.owner, &path)
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    let category_id = scene.channel.category_id.unwrap();
    let path = format!(
        "/api/v1/workspaces/{}/categories/{category_id}",
        scene.channel.workspace_id
    );
    let mut owner_stream = scene.server.subscribed(&scene.owner, Level::Full).await;
    scene
        .server
        .patch(&scene.owner, &path, &json!({"name":"Old plans"}))
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    let renamed = |events: &[(u64, Event)]| {
        events.iter().any(|(_, event)| {
            matches!(event, Event::CategoryUpdated { category } if category.id == category_id)
        })
    };
    let told = scene.server.traffic(&scene.owner, &mut owner_stream).await;
    assert!(renamed(&told), "{told:?}");
    let told = scene.server.traffic(&scene.reader, &mut scene.stream).await;
    assert!(!renamed(&told), "{told:?}");
}
