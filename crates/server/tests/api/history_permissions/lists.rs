use super::*;
use junto_protocol::{discovery::SearchPage, message::ThreadPage};
use serde_json::json;

#[tokio::test]
async fn pins_require_history_permission() {
    let (server, owner, reader, channel, root) = denied().await;
    server
        .put_empty(&owner, &format!("{}/pin", message(root.id)))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&reader, &format!("/api/v1/channels/{}/pins", channel.id))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn thread_history_requires_history_permission() {
    let (server, owner, reader, _, root) = denied().await;
    server
        .post(
            &owner,
            &format!("{}/thread", message(root.id)),
            &text("Reply"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    server
        .get(&owner, &format!("{}/thread", message(root.id)))
        .await
        .ok::<ThreadPage>(StatusCode::OK);
    server
        .get(&reader, &format!("{}/thread", message(root.id)))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn global_search_excludes_channels_without_history_permission() {
    let (server, owner, reader, _, _) = denied().await;
    let before: SearchPage = server
        .get(&owner, "/api/v1/search?q=connection")
        .await
        .ok(StatusCode::OK);
    assert!(!before.results.is_empty());
    let hidden: SearchPage = server
        .get(&reader, "/api/v1/search?q=connection")
        .await
        .ok(StatusCode::OK);
    assert!(hidden.results.is_empty());
}

#[tokio::test]
async fn channel_search_requires_history_permission() {
    let (server, _, reader, channel, _) = denied().await;
    server
        .get(
            &reader,
            &format!("/api/v1/search?q=connection&channel_id={}", channel.id),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn forking_a_thread_requires_history_permission_even_for_a_channel_manager() {
    let (server, owner, reader, channel, root) = denied().await;
    server
        .grant_workspace(&owner, &reader, channel.workspace_id, &[W::ManageChannel])
        .await;
    server
        .post(
            &reader,
            &format!("{}/fork", message(root.id)),
            &json!({"name":"Copy","kind":"text"}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn exporting_a_channel_requires_history_permission_even_for_an_exporter() {
    let directory = crate::support::Scratch::new();
    let path = directory.0.clone();
    let server = TestServer::start_with(move |config| config.exports_directory = Some(path)).await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Exporter").await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "Stored content").await;
    server
        .grant_workspace(&owner, &reader, channel.workspace_id, &[W::ExportChannel])
        .await;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(reader.account_id).bind(channel.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&server.db).await.unwrap();
    server
        .post(
            &reader,
            &format!("/api/v1/channels/{}/exports", channel.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn file_downloads_use_the_creation_time_of_their_message_and_current_connection() {
    let mut files = crate::messages_support::Files::open().await;
    files.owner = files.server.join(&files.owner, "Reader").await;
    let old = files.upload(b"old").await;
    files.attach(&[old.id]).await;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(files.channel.id).bind(files.owner.account_id).bind(files.channel.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&files.server.db).await.unwrap();
    let path = |id| format!("/api/v1/attachments/{id}/file");
    files
        .server
        .get(&files.owner, &path(old.id))
        .await
        .error(ErrorCode::Forbidden);
    let first = files.server.subscribed(&files.owner, Level::Full).await;
    files.server.clock.advance(Duration::seconds(1));
    let live = files.upload(b"live").await;
    files.attach(&[live.id]).await;
    let downloaded = files.server.get(&files.owner, &path(live.id)).await;
    assert_eq!(downloaded.status, StatusCode::OK);
    assert_eq!(downloaded.bytes(), b"live");
    files
        .server
        .get(&files.owner, &path(old.id))
        .await
        .error(ErrorCode::Forbidden);
    files.server.clock.advance(Duration::seconds(1));
    let second = files.server.subscribed(&files.owner, Level::Full).await;
    files
        .server
        .get(&files.owner, &path(live.id))
        .await
        .error(ErrorCode::Forbidden);
    drop((first, second));
}
