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

#[tokio::test]
async fn one_request_of_several_moves_between_categories_sends_one_refresh() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let source = server.category(&owner, workspace.id, "Plans", 0).await;
    let destination = server.category(&owner, workspace.id, "Done", 1).await;
    let mut channels = Vec::new();
    for name in ["One", "Two", "Three"] {
        let channel: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{}/channels", workspace.id),
                &json!({"name":name,"kind":"text","category_id":source.id}),
            )
            .await
            .ok(StatusCode::CREATED);
        channels.push(channel);
    }
    let moves: Vec<_> = channels
        .iter()
        .map(|channel| {
            json!({
                "channel_id": channel.id,
                "from": {"category_id": source.id, "position": channel.position},
                "to": {"category_id": destination.id, "position": channel.position},
            })
        })
        .collect();
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}/channels/positions", workspace.id),
            &json!({ "moves": moves }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = server.traffic(&reader, &mut stream).await;
    let refreshes = events
        .iter()
        .filter(|(_, event)| {
            matches!(event, Event::PermissionsChanged { workspace_id: Some(id) } if *id == workspace.id)
        })
        .count();
    assert_eq!(refreshes, 1, "{events:?}");
    let moved = events
        .iter()
        .filter(|(_, event)| matches!(event, Event::ChannelUpdated { .. }))
        .count();
    assert_eq!(moved, 3, "{events:?}");
}

/// One structure request changes the order of the categories and then moves a voice channel
/// with a running call into a category whose rules deny "connect" to a person in the call.
/// The refresh of the workspace is recorded once, and the room of the call is still marked,
/// so the next check of the marked rooms removes that person (ADR 0509).
#[tokio::test]
async fn a_move_after_a_new_order_of_categories_in_one_request_marks_the_room_of_the_call() {
    let scene = crate::calls_support::Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let workspace = scene.workspace_id;
    let first = server.category(owner, workspace, "First", 0).await;
    let denied = server.category(owner, workspace, "Denied", 1).await;
    let connect = 1i64 << 24;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(denied.id).bind(ann.account_id).bind(workspace).bind(connect).execute(&server.db).await.unwrap();
    let seat = server.enter_call(ann, scene.voice.id).await;
    assert_eq!(server.fake().identities(&seat.room).len(), 1);
    // A full check finds nothing to remove and states that the media server answers.
    server.check().await;
    let revision = server.structure(owner).await.revision;
    server
        .change_structure(
            owner,
            &revision,
            json!([
                { "change": "update_category", "id": first.id, "position": 1 },
                { "change": "update_channel", "id": scene.voice.id, "category": denied.id },
            ]),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    assert_eq!(
        junto_server::calls::admission::check_marked(&server.state)
            .await
            .unwrap(),
        junto_server::calls::admission::Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );
    assert!(server.fake().identities(&seat.room).is_empty());
}
