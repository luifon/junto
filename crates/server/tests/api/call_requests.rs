use crate::calls_review2::move_to;
use crate::calls_support::Scene;
use crate::discovery_support::events;
use junto_protocol::call::CallJoin;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn the_call_setting_and_admission_state_are_explicit() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let settings = format!("/api/v1/channels/{}/call/settings", scene.voice.id);
    server
        .patch(owner, &settings, &json!({"hides_occupants":true}))
        .await
        .ok::<Value>(StatusCode::OK);
    let before = server.snapshot(ann).await.cursor;
    let join = server.join_call(ann, scene.voice.id).await;
    let own = server.own_call(ann).await.call.unwrap();
    assert_eq!(own.own.hides_occupants, Some(true));
    assert_eq!(own.inside, Some(false));
    assert!(own.call.participants.is_empty());
    assert_eq!(
        server.snapshot(ann).await.own_call.unwrap().hides_occupants,
        Some(true)
    );
    server
        .put_empty(
            ann,
            &format!("/api/v1/channels/{}/call/lock", scene.voice.id),
        )
        .await
        .error(ErrorCode::NotInCall);
    server
        .patch(owner, &settings, &json!({"hides_occupants":false}))
        .await
        .ok::<Value>(StatusCode::OK);
    server.connect(join);
    server.check().await;
    let own = server.own_call(ann).await.call.unwrap();
    assert_eq!(own.inside, Some(true));
    assert_eq!(own.own.hides_occupants, Some(true));
    let received = events(server, ann, before, Level::Full).await;
    assert!(received.iter().any(|e| matches!(
        e,
        Event::CallStarted {
            hides_occupants: Some(true),
            ..
        }
    )));
    assert!(
        received.iter().any(
            |e| matches!(e, Event::OwnCallChanged { call:Some(own), .. } if own.hides_occupants == Some(true))
        )
    );
    server
        .put_empty(
            ann,
            &format!("/api/v1/channels/{}/call/lock", scene.voice.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_move_announces_the_own_call_before_the_join() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.enter_call(ann, scene.voice.id).await;
    let destination = server.voice(owner, scene.workspace_id, "destination").await;
    let movement = move_to(
        server,
        owner,
        scene.voice.id,
        ann.account_id,
        destination.id,
    )
    .await;
    let join: CallJoin = server
        .post(
            ann,
            &format!("/api/v1/call/moves/{}/join", movement.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    let call_id = join.call.id;
    let before = server.snapshot(ann).await.cursor;
    server.connect(join);
    server.check().await;
    let received = events(server, ann, before, Level::Full).await;
    let own = received
        .iter()
        .position(
            |e| matches!(e, Event::OwnCallChanged {call:Some(own), ..} if own.call_id==call_id),
        )
        .unwrap();
    let joined = received.iter().position(|e| matches!(e, Event::CallParticipantJoined {call_id:id,participant,..} if *id==call_id && participant.account_id==ann.account_id)).unwrap();
    assert!(own < joined, "{received:?}");
}

#[tokio::test]
async fn review2_upgrade_renews_the_epoch_without_rewriting_events() {
    let scene = Scene::start().await;
    let server = &scene.server;
    server
        .patch(
            &scene.owner,
            &format!("/api/v1/channels/{}/call/settings", scene.voice.id),
            &json!({"hides_occupants":true}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server.enter_call(&scene.ann, scene.voice.id).await;
    server.leave_call(&scene.ann).await;
    let direct: CallJoin = crate::calls_review2::ring(server, &scene.ann, &scene.ben)
        .await
        .ok(StatusCode::OK);
    server.connect(direct);
    server.check().await;
    let before: Vec<Value>=sqlx::query_scalar("select payload from events where kind in ('call_started','own_call_changed') order by position").fetch_all(&server.db).await.unwrap();
    assert!(
        before
            .iter()
            .any(|event| event["type"] == "own_call_changed" && event["call"].is_null())
    );
    assert!(
        before
            .iter()
            .any(|event| event["type"] == "call_started" && event["hides_occupants"] == false)
    );
    sqlx::raw_sql("alter table events disable trigger events_no_change; update events set payload=payload-'hides_occupants' where kind='call_started'; update events set payload=jsonb_set(payload,'{call}',(payload->'call')-'hides_occupants') where kind='own_call_changed' and payload->'call'<>'null'::jsonb; alter table events enable trigger events_no_change;")
        .execute(&server.db).await.unwrap();
    let original: Vec<Value> = sqlx::query_scalar("select payload from events where kind in ('call_started','own_call_changed') order by position").fetch_all(&server.db).await.unwrap();
    let old_cursor = server.snapshot(&scene.ann).await.cursor;
    sqlx::raw_sql(include_str!(
        "../../migrations/0016_remaining_server_requests.sql"
    ))
    .execute(&server.db)
    .await
    .unwrap();
    let after: Vec<Value>=sqlx::query_scalar("select payload from events where kind in ('call_started','own_call_changed') order by position").fetch_all(&server.db).await.unwrap();
    assert_eq!(after, original);
    assert_ne!(
        server.snapshot(&scene.ann).await.cursor.epoch,
        old_cursor.epoch
    );
    let start = junto_server::events::delivery::subscribe(
        &server.state,
        &crate::stream_review::opened(&scene.ann),
        Level::Full,
        Some(old_cursor),
    )
    .await
    .unwrap();
    assert!(matches!(
        start,
        junto_server::events::delivery::Start::Resync(
            junto_protocol::events::ResyncReason::UnknownCursor
        )
    ));
}

#[tokio::test]
async fn missing_call_hide_fields_mean_unknown_to_a_reader() {
    let scene = Scene::start().await;
    let join = scene.server.join_call(&scene.ann, scene.voice.id).await;
    let own = scene.server.snapshot(&scene.ann).await.own_call.unwrap();
    let mut value = serde_json::to_value(own).unwrap();
    value.as_object_mut().unwrap().remove("hides_occupants");
    let decoded: junto_protocol::call::OwnCall = serde_json::from_value(value).unwrap();
    assert!(serde_json::to_value(decoded).unwrap()["hides_occupants"].is_null());
    let event = json!({"type":"call_started","call_id":join.call.id,"place":join.call.place,"started_at":scene.server.clock.now().format(&time::format_description::well_known::Rfc3339).unwrap()});
    let decoded: Event = serde_json::from_value(event).unwrap();
    assert!(serde_json::to_value(decoded).unwrap()["hides_occupants"].is_null());
}
