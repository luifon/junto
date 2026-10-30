use crate::{
    messages_support::{channel, send},
    stream_support::{Frame, Stream},
    support::TestServer,
};
use junto_protocol::{
    WorkspacePermission as W,
    events::{ClientMessage, Level, ServerMessage},
    typing::{Typing, TypingPlace},
};
use reqwest::StatusCode;
use serde_json::json;
use std::time::Duration;

async fn watch(stream: &mut Stream, places: Vec<TypingPlace>) {
    stream
        .send(&ClientMessage::TypingWatch {
            places: places.clone(),
        })
        .await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match stream.next().await {
                Frame::Message(ServerMessage::TypingWatching { places: accepted }) => {
                    assert_eq!(accepted, places);
                    return;
                }
                Frame::Message(_) => {}
                other => panic!("watch closed: {other:?}"),
            }
        }
    })
    .await
    .expect("the watch was not acknowledged");
}
async fn activity(stream: &mut Stream) -> Typing {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match stream.next().await {
                Frame::Message(ServerMessage::Typing { activity }) => return activity,
                Frame::Message(_) => {}
                other => panic!("activity closed: {other:?}"),
            }
        }
    })
    .await
    .expect("activity was not delivered")
}
async fn change(stream: &mut Stream, place: TypingPlace, lifetime_ms: u32) {
    stream
        .send(&ClientMessage::Typing { place, lifetime_ms })
        .await;
}
async fn no_activity(stream: &mut Stream) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(frame) = stream.within(Duration::from_millis(300)).await {
            assert!(
                !matches!(frame, Frame::Message(ServerMessage::Typing { .. })),
                "{frame:?}"
            );
        }
    })
    .await
    .expect("the stream never became quiet");
}
#[tokio::test]
async fn activity_is_volatile_and_rechecks_sender_and_reader_permissions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let alice = server.join(&owner, "Luca").await;
    let bob = server.join(&owner, "Marta").await;
    let other = server.join(&owner, "Cara").await;
    let channel = channel(&server, &owner, "text").await;
    let place = TypingPlace::Channel {
        channel_id: channel.id,
    };
    let mut writer = server.subscribed(&alice, Level::Full).await;
    let mut reader = server.subscribed(&bob, Level::Full).await;
    let mut third = server.subscribed(&other, Level::Full).await;
    watch(&mut reader, vec![place]).await;
    watch(&mut third, vec![place]).await;
    let before = server.log().await.len();
    change(&mut writer, place, 8000).await;
    let first = activity(&mut reader).await;
    assert_eq!(first.account_id, alice.account_id);
    assert_eq!(first.name, "Luca");
    assert!(first.active && first.expires_in_ms <= 8000);
    activity(&mut third).await;
    change(&mut writer, place, 0).await;
    assert!(!activity(&mut reader).await.active);
    activity(&mut third).await;
    assert_eq!(server.log().await.len(), before);
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)").bind(channel.id).bind(alice.account_id).bind(channel.workspace_id).bind(1_i64 << W::SendMessages as u8).execute(&server.db).await.unwrap();
    change(&mut writer, place, 8000).await;
    watch(&mut writer, Vec::new()).await;
    no_activity(&mut reader).await;
    sqlx::query("delete from channel_person_overrides where channel_id=$1 and account_id=$2")
        .bind(channel.id)
        .bind(alice.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update channels set archived_at=now() where id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    change(&mut writer, place, 8000).await;
    watch(&mut writer, Vec::new()).await;
    no_activity(&mut reader).await;
    sqlx::query("update channels set archived_at=null where id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)").bind(channel.id).bind(bob.account_id).bind(channel.workspace_id).bind(1_i64 << W::ViewChannel as u8).execute(&server.db).await.unwrap();
    change(&mut writer, place, 8000).await;
    assert!(activity(&mut third).await.active);
    no_activity(&mut reader).await;
    assert_eq!(server.log().await.len(), before);
}
#[tokio::test]
async fn private_conversations_threads_and_connection_closes_keep_their_scope() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let alice = server.join(&owner, "Luca").await;
    let bob = server.join(&owner, "Marta").await;
    let outsider = server.join(&owner, "Cara").await;
    let conversation: junto_protocol::message::Conversation = server
        .post(
            &alice,
            "/api/v1/conversations",
            &json!({"account_id": bob.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let direct = TypingPlace::Direct {
        conversation_id: conversation.id,
    };
    let mut writer = server.subscribed(&alice, Level::Full).await;
    let mut reader = server.subscribed(&bob, Level::Full).await;
    let mut third = server.subscribed(&outsider, Level::Full).await;
    watch(&mut reader, vec![direct]).await;
    watch(&mut third, vec![direct]).await;
    change(&mut writer, direct, 8000).await;
    assert!(activity(&mut reader).await.active);
    no_activity(&mut third).await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "a thread").await;
    let thread = TypingPlace::Thread {
        channel_id: channel.id,
        root_id: root.id,
    };
    watch(&mut reader, vec![thread]).await;
    watch(
        &mut third,
        vec![TypingPlace::Channel {
            channel_id: channel.id,
        }],
    )
    .await;
    change(&mut writer, thread, 8000).await;
    assert_eq!(activity(&mut reader).await.place, thread);
    no_activity(&mut third).await;
    writer
        .send_frame(tokio_tungstenite::tungstenite::Message::Close(None))
        .await
        .unwrap();
    assert!(!activity(&mut reader).await.active);
}
#[tokio::test]
async fn thread_activity_obeys_the_roots_live_history_boundary() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let bob = server.join(&owner, "Marta").await;
    let channel = channel(&server, &owner, "text").await;
    let old = send(&server, &owner, &channel, "before the connection").await;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)").bind(channel.id).bind(bob.account_id).bind(channel.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&server.db).await.unwrap();
    let mut writer = server.subscribed(&owner, Level::Full).await;
    let mut reader = server.subscribed(&bob, Level::Full).await;
    let old = TypingPlace::Thread {
        channel_id: channel.id,
        root_id: old.id,
    };
    watch(&mut reader, vec![old]).await;
    change(&mut writer, old, 8000).await;
    watch(&mut writer, Vec::new()).await;
    no_activity(&mut reader).await;
    server.clock.advance(time::Duration::seconds(1));
    let new = send(&server, &owner, &channel, "after the connection").await;
    let new = TypingPlace::Thread {
        channel_id: channel.id,
        root_id: new.id,
    };
    watch(&mut reader, vec![new]).await;
    change(&mut writer, new, 8000).await;
    assert_eq!(activity(&mut reader).await.place, new);
}

#[tokio::test]
async fn replacing_a_watch_delivers_only_the_latest_places() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let bob = server.join(&owner, "Marta").await;
    let first = channel(&server, &owner, "text").await;
    let second: junto_protocol::structure::Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", first.workspace_id),
            &json!({ "name": "other-messages", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let first = TypingPlace::Channel {
        channel_id: first.id,
    };
    let second = TypingPlace::Channel {
        channel_id: second.id,
    };
    let mut writer = server.subscribed(&owner, Level::Full).await;
    let mut reader = server.subscribed(&bob, Level::Full).await;
    reader
        .send(&ClientMessage::TypingWatch {
            places: vec![first],
        })
        .await;
    reader
        .send(&ClientMessage::TypingWatch {
            places: vec![second],
        })
        .await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match reader.next().await {
                Frame::Message(ServerMessage::TypingWatching { places }) if places == [second] => {
                    break;
                }
                Frame::Message(_) => {}
                other => panic!("watch closed: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    change(&mut writer, first, 8000).await;
    change(&mut writer, second, 8000).await;
    assert_eq!(activity(&mut reader).await.place, second);
}
