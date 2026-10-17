//! Existing call notes and channel audit rows are converted by migration 0007.
use crate::calls_support::Scene;
use junto_protocol::{ids::MessageId, message::MessagePage};
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn migration_backfills_notes_and_audit_actions_in_order_without_private_details() {
    let scene = Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    server.clock.advance(time::Duration::seconds(1));
    server.enter_call(&scene.ann, scene.voice.id).await;
    server.enter_call(&scene.ben, scene.voice.id).await;
    server.clock.advance(time::Duration::seconds(1));
    server.leave_call(&scene.ben).await;
    server.clock.advance(time::Duration::seconds(1));
    server.leave_call(&scene.ann).await;
    server.clock.advance(time::Duration::seconds(1));
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":false}),
        )
        .await;
    let old_notes: Vec<(MessageId, uuid::Uuid, String)> = sqlx::query_as(
        "select id,call_id,kind from call_notes where channel_id=$1 order by occurred_at,id",
    )
    .bind(scene.voice.id)
    .fetch_all(&server.db)
    .await
    .unwrap();
    let expected: Vec<MessageId> = sqlx::query_scalar(
        "select id from (
            select id,occurred_at from call_notes where channel_id=$1
            union all select id,occurred_at from audit_entries where channel_id=$1
                and action not in ('recording_started','recording_stopped')
         ) source order by occurred_at,id",
    )
    .bind(scene.voice.id)
    .fetch_all(&server.db)
    .await
    .unwrap();
    let count:i64=sqlx::query_scalar("select (select count(*) from call_notes where channel_id=$1)+(select count(*) from audit_entries where channel_id=$1 and action not in ('recording_started','recording_stopped'))").bind(scene.voice.id).fetch_one(&server.db).await.unwrap();
    // Rebuild the old schema, assert its backfill, then upgrade it before using current APIs.
    sqlx::raw_sql(
        "drop table thread_forks,message_pins,notification_settings,workspace_notification_defaults,
             notifications,workspace_file_retention,direct_message_retention,notification_receivers,
             thread_reads,workspace_recording_retention,notification_trim_accounts cascade;
         drop function message_search_text(jsonb),notification_policy(uuid,uuid,timestamptz),
             notification_mentioned(uuid,uuid,boolean,boolean),notification_quiet(uuid,timestamptz),
             update_thread_summary(),initialize_thread_read(),refresh_mention_head(),queue_notification_trim(),
             transcript_search_original(),transcript_search_correction() cascade;
         alter table transcript_lines drop column search_text cascade;
         drop index recordings_retention;",
    )
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::raw_sql("drop table message_mention_accounts,message_mention_receivers,attachment_limits,attachment_chunks,attachments,message_mentions_notified,message_mentions,message_reactions,message_reads,messages,message_places,conversations,call_message_members,call_message_privacy cascade; drop function remember_call_message_member(),refresh_call_message_privacy(uuid) cascade;")
        .execute(&server.db).await.unwrap();
    sqlx::raw_sql(include_str!("../../migrations/0007_messages.sql"))
        .execute(&server.db)
        .await
        .unwrap();
    let converted: Vec<(MessageId, i64, Option<uuid::Uuid>, serde_json::Value)> = sqlx::query_as(
        "select id,position,call_id,system from messages where place_id=$1 order by position,id",
    )
    .bind(scene.voice.id)
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(converted.len() as i64, count);
    assert_eq!(
        converted.iter().map(|row| row.0).collect::<Vec<_>>(),
        expected
    );
    assert!(converted.windows(2).all(|rows| rows[0].1 <= rows[1].1));
    for (_, _, _, system) in &converted {
        assert!(system.get("details").is_none());
    }
    for (id, call_id, kind) in old_notes {
        let row = converted
            .iter()
            .find(|(message, _, _, _)| *message == id)
            .unwrap();
        assert_eq!(row.2, Some(call_id));
        assert_eq!(row.3["action"], kind);
    }
    sqlx::raw_sql(include_str!(
        "../../migrations/0008_threads_notifications_search.sql"
    ))
    .execute(&server.db)
    .await
    .unwrap();
    // The current API reads the position of each attached file.
    sqlx::raw_sql(include_str!(
        "../../migrations/0025_attachment_positions.sql"
    ))
    .execute(&server.db)
    .await
    .unwrap();
    let outside: MessagePage = server
        .get(
            &scene.owner,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(outside.messages.iter().all(|message| {
        message
            .system
            .as_ref()
            .is_none_or(|line| !matches!(line.action.as_str(), "call_started" | "call_ended"))
    }));
    let inside: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let ended = inside
        .messages
        .iter()
        .find(|message| {
            message
                .system
                .as_ref()
                .is_some_and(|line| line.action == "call_ended")
        })
        .unwrap();
    server
        .get(&scene.ben, &crate::messages_support::message(ended.id))
        .await
        .ok::<junto_protocol::message::Message>(StatusCode::OK);
    assert!(inside.messages.iter().any(|message| {
        message
            .system
            .as_ref()
            .is_some_and(|line| line.action == "call_started")
    }));
}
