use crate::calls_support::{Scene, claims};
use junto_protocol::{WorkspacePermission as W, error::ErrorCode};
use junto_server::{
    calls::admission::Outcome,
    media::{TrackKind, TrackSource},
};
use serde_json::json;

async fn deny(scene: &Scene, permissions: &[W]) {
    let bits = permissions.iter().fold(0_i64, |bits, permission| {
        bits | (1_i64 << *permission as u8)
    });
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,$4) on conflict(channel_id,account_id) do update set deny=excluded.deny")
        .bind(scene.voice.id).bind(scene.workspace_id).bind(scene.ann.account_id).bind(bits).execute(&scene.server.db).await.unwrap();
}

#[tokio::test]
async fn a_visible_channel_without_connect_refuses_a_join() {
    let scene = Scene::start().await;
    deny(&scene, &[W::Connect]).await;
    scene.server.channel_call(&scene.ann, scene.voice.id).await;
    scene
        .server
        .call_join_reply(&scene.ann, scene.voice.id)
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn losing_connect_removes_an_existing_media_participant() {
    let scene = Scene::start().await;
    let seat = scene.server.enter_call(&scene.ann, scene.voice.id).await;
    deny(&scene, &[W::Connect]).await;
    assert_eq!(
        scene.server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );
    assert!(
        !scene
            .server
            .fake()
            .identities(&seat.room)
            .contains(&seat.identity)
    );
}

#[tokio::test]
async fn track_permissions_restrict_the_join_token_sources() {
    for (denied, expected) in [
        (vec![W::Speak], json!(["camera", "screen_share"])),
        (
            vec![W::Video],
            json!(["microphone", "screen_share", "screen_share_audio"]),
        ),
        (vec![W::ShareScreen], json!(["camera", "microphone"])),
        (vec![W::Speak, W::Video, W::ShareScreen], json!([])),
    ] {
        let scene = Scene::start().await;
        deny(&scene, &denied).await;
        let join = scene.server.join_call(&scene.ann, scene.voice.id).await;
        let token = claims(&join);
        assert_eq!(token["video"]["canPublishSources"], expected, "{denied:?}");
        assert_eq!(
            token["video"]["canPublish"],
            !expected.as_array().unwrap().is_empty()
        );
    }
}

#[tokio::test]
async fn hidden_occupancy_join_tokens_still_obey_channel_track_permissions() {
    let scene = Scene::start().await;
    scene
        .server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    deny(&scene, &[W::Video, W::ShareScreen]).await;
    let join = scene.server.join_call(&scene.ann, scene.voice.id).await;
    assert_eq!(claims(&join)["video"]["canPublish"], false);
    assert_eq!(claims(&join)["video"]["canPublishSources"], json!([]));
}

#[tokio::test]
async fn losing_speak_restricts_existing_tracks_and_restoring_it_restores_publication() {
    let scene = Scene::start().await;
    let seat = scene.server.enter_call(&scene.ann, scene.voice.id).await;
    let fake = scene.server.fake();
    assert!(fake.publish(
        &seat.room,
        &seat.identity,
        TrackKind::Audio,
        TrackSource::Microphone
    ));
    deny(&scene, &[W::Speak]).await;
    assert_eq!(
        scene.server.check().await,
        Outcome::Checked {
            removed: 0,
            updated: 1
        }
    );
    assert!(!fake.publish(
        &seat.room,
        &seat.identity,
        TrackKind::Audio,
        TrackSource::Microphone
    ));
    assert!(!fake.publish(
        &seat.room,
        &seat.identity,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio
    ));
    assert!(fake.publish(
        &seat.room,
        &seat.identity,
        TrackKind::Video,
        TrackSource::Camera
    ));
    sqlx::query("delete from channel_person_overrides where channel_id=$1 and account_id=$2")
        .bind(scene.voice.id)
        .bind(scene.ann.account_id)
        .execute(&scene.server.db)
        .await
        .unwrap();
    assert_eq!(
        scene.server.check().await,
        Outcome::Checked {
            removed: 0,
            updated: 1
        }
    );
    assert!(fake.publish(
        &seat.room,
        &seat.identity,
        TrackKind::Audio,
        TrackSource::Microphone
    ));
}

#[tokio::test]
async fn asking_to_enter_requires_connect_before_reading_call_state() {
    let scene = Scene::start().await;
    deny(&scene, &[W::Connect]).await;
    scene
        .server
        .post(
            &scene.ann,
            &format!("/api/v1/channels/{}/call/entry-requests", scene.voice.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn admission_removes_tracks_that_bypass_the_source_permission() {
    for (denied, kind, source) in [
        (W::Video, TrackKind::Video, TrackSource::Camera),
        (W::ShareScreen, TrackKind::Video, TrackSource::ScreenShare),
        (W::Speak, TrackKind::Audio, TrackSource::Camera),
    ] {
        let scene = Scene::start().await;
        deny(&scene, &[denied]).await;
        let seat = scene.server.enter_call(&scene.ann, scene.voice.id).await;
        scene
            .server
            .fake()
            .force_track(&seat.room, &seat.identity, kind, source);
        assert_eq!(
            scene.server.check().await,
            Outcome::Checked {
                removed: 1,
                updated: 0
            },
            "{denied:?}"
        );
    }
}

#[tokio::test]
async fn set_call_user_limit_changes_the_user_limit_without_manage_channel() {
    let scene = Scene::start().await;
    let path = format!("/api/v1/channels/{}/call/settings", scene.voice.id);
    scene
        .server
        .patch(&scene.ann, &path, &json!({ "user_limit": 5 }))
        .await
        .error(ErrorCode::Forbidden);
    let bits = 1_i64 << W::SetCallUserLimit as u8;
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,$4,0)")
        .bind(scene.voice.id).bind(scene.workspace_id).bind(scene.ann.account_id).bind(bits).execute(&scene.server.db).await.unwrap();
    let channel: junto_protocol::structure::Channel = scene
        .server
        .patch(&scene.ann, &path, &json!({ "user_limit": 5 }))
        .await
        .ok(reqwest::StatusCode::OK);
    assert_eq!(channel.user_limit, Some(5));
    // Hiding who is inside still takes "manage channel".
    scene
        .server
        .patch(
            &scene.ann,
            &path,
            &json!({ "user_limit": 6, "hides_occupants": true }),
        )
        .await
        .error(ErrorCode::Forbidden);
}
