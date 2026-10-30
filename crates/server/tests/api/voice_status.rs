use crate::calls_support::Scene;
use junto_protocol::{WorkspacePermission as W, error::ErrorCode};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn deny(scene: &Scene, permission: W) {
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,$4) on conflict(channel_id,account_id) do update set deny=excluded.deny")
        .bind(scene.voice.id).bind(scene.workspace_id).bind(scene.ann.account_id)
        .bind(1_i64 << permission as u8).execute(&scene.server.db).await.unwrap();
}

#[tokio::test]
async fn voice_status_requires_a_confirmed_participant_or_channel_management_and_the_status_permission()
 {
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/channels/{}/voice-status", scene.voice.id);
    server
        .put(&scene.ann, &path, &json!({"status":"Outside"}))
        .await
        .error(ErrorCode::Forbidden);
    let pending = server.join_call(&scene.ann, scene.voice.id).await;
    server
        .put(&scene.ann, &path, &json!({"status":"Not connected yet"}))
        .await
        .error(ErrorCode::Forbidden);
    server.connect(pending);
    server.check().await;
    let changed: Value = server
        .put(&scene.ann, &path, &json!({"status":" Planning 🚀 "}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed["voice_status"], "Planning 🚀");
    let read: Value = server
        .get(&scene.ben, &format!("/api/v1/channels/{}", scene.voice.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(read["voice_status"], "Planning 🚀");
    deny(&scene, W::SetVoiceChannelStatus).await;
    server
        .put(&scene.ann, &path, &json!({"status":"Denied inside"}))
        .await
        .error(ErrorCode::Forbidden);
    let manager: Value = server
        .put(&scene.owner, &path, &json!({"status":"Manager outside"}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(manager["voice_status"], "Manager outside");
    deny(&scene, W::ViewChannel).await;
    server
        .put(&scene.ann, &path, &json!({"status":"Hidden"}))
        .await
        .error(ErrorCode::NotFound);
    let cleared: Value = server
        .put(&scene.owner, &path, &json!({"status":""}))
        .await
        .ok(StatusCode::OK);
    assert!(cleared.get("voice_status").is_none());
}

#[tokio::test]
async fn voice_status_is_bounded_plain_text_and_cannot_change_a_text_or_archived_channel() {
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/channels/{}/voice-status", scene.voice.id);
    for status in [
        "é".repeat(501),
        "bad\nline".to_owned(),
        "bad\0text".to_owned(),
    ] {
        server
            .put(&scene.owner, &path, &json!({"status":status}))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let status = "é".repeat(500);
    let changed: Value = server
        .put(&scene.owner, &path, &json!({"status":status}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed["voice_status"], status);
    let exported: Value = server
        .get(&scene.owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    assert!(!exported.to_string().contains("voice_status"));
    let text = crate::messages_support::channel(server, &scene.owner, "text").await;
    server
        .put(
            &scene.owner,
            &format!("/api/v1/channels/{}/voice-status", text.id),
            &json!({"status":"Wrong kind"}),
        )
        .await
        .error(ErrorCode::Conflict);
    crate::archive::archive(server, &scene.owner, scene.voice.id)
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .put(&scene.owner, &path, &json!({"status":"Archived"}))
        .await
        .error(ErrorCode::ChannelArchived);
}
