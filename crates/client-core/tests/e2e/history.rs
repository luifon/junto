//! A channel whose history the member may not read, against the real server (ADR 0304):
//! the timeline holds the messages of the connections, a reconnect keeps them, and the
//! server's withdrawal of older messages or a loss of the history removes the older ones.

use junto_client_core::ServerHandle;
use junto_client_core::live::StreamStatus;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::ChannelId;
use junto_client_core::protocol::message::{Message, SendMessage};
use junto_client_core::protocol::structure::SetChannelOverride;

use crate::messages::{both_open, document, message_when, texts, timeline_when};
use crate::stream::{Pair, live, model_when, pair, text_channel, when};
use crate::support::{FaultProxy, TestServer};

async fn send(owner: &ServerHandle, place: PlaceId, text: &str) -> Message {
    let message = SendMessage {
        document: document(text),
        reply_to: None,
        attachments: Vec::new(),
    };
    owner.api().send_message(place, &message).await.unwrap()
}

/// Denies `read_message_history` to the everyone role in the channel, and waits until the
/// member's state says so.
async fn deny_history(scene: &Pair, channel_id: ChannelId) {
    let everyone = scene.owner.live().view().model.unwrap().workspaces[0]
        .roles
        .iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    let deny = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ReadMessageHistory],
    };
    scene
        .owner
        .api()
        .set_channel_override(channel_id, everyone, &deny)
        .await
        .unwrap();
    model_when(&scene.member, "the history denied", |model| {
        model.channel(channel_id).is_some()
            && !model.holds_in(channel_id, WorkspacePermission::ReadMessageHistory)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_history_a_reconnect_keeps_the_messages_and_a_withdrawal_drops_the_older_ones() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let scene = pair(server, &proxy.address()).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    send(owner, place, "before").await;
    deny_history(&scene, general.id).await;
    both_open(&scene, place).await;
    assert!(texts(&member.live().view().model.unwrap(), place).is_empty());

    let first = send(owner, place, "first connection").await;
    message_when(member, place, "first connection").await;

    proxy.block_streams(true);
    proxy.cut_streams();
    when(member, "the loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    proxy.block_streams(false);
    when(member, "the next connection", |view| {
        view.status == StreamStatus::Live
    })
    .await;
    assert_eq!(
        texts(&member.live().view().model.unwrap(), place),
        ["first connection"]
    );

    send(owner, place, "second connection").await;
    message_when(member, place, "second connection").await;
    owner.api().delete_message(first.id).await.unwrap();
    let model = timeline_when(member, place, "the withdrawal", |timeline, _| {
        timeline.message(first.id).is_none()
    })
    .await;
    assert_eq!(texts(&model, place), ["second connection"]);
    assert!(!model.timeline(place).unwrap().stale);
}

#[tokio::test(flavor = "multi_thread")]
async fn losing_the_history_removes_the_older_messages_and_new_ones_still_arrive() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    both_open(&scene, place).await;
    send(owner, place, "with history").await;
    message_when(member, place, "with history").await;

    deny_history(&scene, general.id).await;
    let model = timeline_when(member, place, "the reload", |timeline, _| {
        timeline.loaded && !timeline.stale
    })
    .await;
    assert!(texts(&model, place).is_empty());

    send(owner, place, "without history").await;
    let model = model_when(member, "the new message", |model| {
        texts(model, place) == ["without history"]
    })
    .await;
    assert!(model.timeline(place).unwrap().failed.is_none());
}
