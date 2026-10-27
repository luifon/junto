//! The messages of the client core against the real server: the timelines of two clients,
//! the waiting messages, the files, and the unread summaries (ADR 0721).

use std::sync::Arc;
use std::time::Duration;

use junto_client_core::ServerHandle;
use junto_client_core::api::{ErrorCategory, PageQuery};
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::flow::approve_link::ApproveLinkAction;
use junto_client_core::live::StreamStatus;
use junto_client_core::model::{PlaceId, SendFailure, SendState, ServerModel, Timeline};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{AccountId, AttachmentId, MessageId};
use junto_client_core::protocol::message::{Block, Document, Inline, Message};
use junto_client_core::protocol::structure::RuleSubject;
use junto_client_core::protocol::structure::{ChannelKind, SetChannelOverride};
use junto_client_core::transfers::{DownloadStage, UploadSource, UploadStage};

use crate::stream::{PATIENCE, Pair, channel, live, model_when, pair, text_channel, when};
use crate::support::{Device, Fault, FaultProxy, TestServer, logged_in};

pub fn document(text: &str) -> Document {
    Document {
        blocks: vec![Block::Paragraph {
            content: vec![Inline::Text {
                text: text.to_owned(),
                marks: Vec::new(),
            }],
        }],
    }
}

pub fn text_of(message: &Message) -> Option<String> {
    let document = message.document.as_ref()?;
    Some(
        document
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph { content } => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|node| match node {
                Inline::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
    )
}

/// The texts of the messages of people that a timeline holds, in its order.
pub fn texts(model: &ServerModel, place: PlaceId) -> Vec<String> {
    model
        .timeline(place)
        .map(|timeline| {
            timeline
                .messages
                .iter()
                .filter_map(|m| text_of(m))
                .collect()
        })
        .unwrap_or_default()
}

pub fn found(model: &ServerModel, place: PlaceId, text: &str) -> Option<Arc<Message>> {
    model
        .timeline(place)?
        .messages
        .iter()
        .find(|message| text_of(message).as_deref() == Some(text))
        .cloned()
}

pub async fn timeline_when(
    handle: &ServerHandle,
    place: PlaceId,
    what: &str,
    mut holds: impl FnMut(&Timeline, &ServerModel) -> bool,
) -> Arc<ServerModel> {
    model_when(handle, what, |model| {
        model
            .timeline(place)
            .is_some_and(|timeline| holds(timeline, model))
    })
    .await
}

pub async fn message_when(handle: &ServerHandle, place: PlaceId, text: &str) -> Arc<Message> {
    let model = model_when(handle, &format!("the message {text:?}"), |model| {
        found(model, place, text).is_some()
    })
    .await;
    found(&model, place, text).unwrap()
}

/// Waits until the timeline of `place` holds a message with each of `texts`, the messages
/// that this client sent.
///
/// The client sends the messages of one place one at a time: each request starts after the
/// answer to the request before it, so that the messages keep their order. The time that
/// many messages take therefore grows with their number and with the time of one request.
/// A wait of [`PATIENCE`] for the last of them bounds the sum of all the requests, and it
/// fails on a slow machine while every message arrives. Each step of this wait is bounded
/// instead: a step holds when the timeline holds one more of the texts than before, and a
/// step that sees none arrive within [`PATIENCE`] fails the test.
pub async fn messages_when(handle: &ServerHandle, place: PlaceId, texts: &[String]) {
    let held = |model: &ServerModel| {
        texts
            .iter()
            .filter(|text| found(model, place, text).is_some())
            .count()
    };
    let mut count = handle.live().view().model.as_deref().map_or(0, held);
    while count < texts.len() {
        let what = format!("{} of the {} messages", count + 1, texts.len());
        let model = model_when(handle, &what, |model| held(model) > count).await;
        count = held(&model);
    }
}

/// Every message of a place as the server gives it to this account.
async fn history(handle: &ServerHandle, place: PlaceId) -> Vec<Message> {
    let mut all = Vec::new();
    let mut query = PageQuery {
        after: Some(0),
        limit: Some(200),
        ..PageQuery::default()
    };
    loop {
        let page = handle.api().messages(place, &query).await.unwrap();
        let last = page.messages.last().map(|message| message.id);
        all.extend(page.messages);
        match last {
            Some(last) if page.has_more => {
                query = PageQuery {
                    after_message: Some(last),
                    limit: Some(200),
                    ..PageQuery::default()
                };
            }
            _ => return all,
        }
    }
}

/// The timeline of a client holds what the server gives now, message for message.
async fn agrees_with_the_history(handle: &ServerHandle, place: PlaceId) {
    let started = tokio::time::Instant::now();
    loop {
        let fresh = history(handle, place).await;
        let model = handle.live().view().model.unwrap();
        let held: Vec<Message> = model
            .timeline(place)
            .unwrap()
            .messages
            .iter()
            .map(|message| message.as_ref().clone())
            .collect();
        if held == fresh {
            return;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "the timeline differs from the history:\n{held:#?}\n{fresh:#?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Two clients with their event streams, and the place open on both.
pub async fn both_open(scene: &Pair, place: PlaceId) {
    for handle in [&scene.owner, &scene.member] {
        // The event of a new channel reaches the client that created it after the answer
        // to its request.
        model_when(handle, "the place", |model| model.place(place).is_some()).await;
        handle.live().open_messages(place).await.unwrap();
        timeline_when(handle, place, "the first page", |timeline, _| {
            timeline.loaded
        })
        .await;
    }
}

/// What two people do with messages in one place, as each client sees it.
async fn exchange(scene: &Pair, place: PlaceId) {
    let (owner, member) = (&scene.owner, &scene.member);
    both_open(scene, place).await;

    // A message of one client arrives at the other, once on both.
    owner
        .live()
        .send_message(place, document("hello"), None)
        .unwrap();
    let hello = message_when(member, place, "hello").await;
    assert_eq!(hello.author_id, Some(logged_in(owner)));
    let model = model_when(owner, "the message is confirmed", |model| {
        found(model, place, "hello").is_some() && model.outbox.is_empty()
    })
    .await;
    assert_eq!(texts(&model, place), ["hello"]);

    // A reply names its message.
    member
        .live()
        .send_message(place, document("reply"), Some(hello.id))
        .unwrap();
    let reply = message_when(owner, place, "reply").await;
    assert_eq!(reply.reply_to, Some(hello.id));

    // An edit replaces the text on both clients. Each learns it from the event of the
    // change; the answer to the request is not put into the state.
    owner
        .api()
        .edit_message(hello.id, document("hello again"))
        .await
        .unwrap();
    for handle in [owner, member] {
        let seen = message_when(handle, place, "hello again").await;
        assert!(seen.edited_at.is_some());
        assert_eq!(seen.id, hello.id);
    }

    // A reaction is counted for everyone and marked for the account that gave it.
    let thumbs_up = "\u{1f44d}";
    member
        .api()
        .add_reaction(hello.id, thumbs_up)
        .await
        .unwrap();
    for (handle, own) in [(owner, false), (member, true)] {
        timeline_when(handle, place, "the reaction", |timeline, _| {
            timeline.message(hello.id).is_some_and(|message| {
                message.reactions.len() == 1
                    && message.reactions[0].emoji == thumbs_up
                    && message.reactions[0].count == 1
                    && message.reactions[0].own == own
            })
        })
        .await;
    }
    member
        .api()
        .remove_reaction(hello.id, thumbs_up)
        .await
        .unwrap();
    for handle in [owner, member] {
        timeline_when(handle, place, "no reaction", |timeline, _| {
            timeline
                .message(hello.id)
                .is_some_and(|message| message.reactions.is_empty())
        })
        .await;
    }

    // A file is uploaded, sent with a message, and downloaded by the other client.
    let content: Vec<u8> = (0..70_000u32).map(|number| (number % 251) as u8).collect();
    let transfers = owner.transfers();
    let upload = transfers.upload(
        place,
        UploadSource::Bytes {
            name: "notes.bin".to_owned(),
            bytes: Arc::new(content.clone()),
        },
    );
    transfers
        .send(
            place,
            document("with a file"),
            None,
            std::slice::from_ref(&upload),
        )
        .unwrap();
    let with_file = message_when(member, place, "with a file").await;
    assert!(matches!(upload.progress().stage, UploadStage::Complete(_)));
    assert_eq!(with_file.attachments.len(), 1);
    let attachment = &with_file.attachments[0];
    assert_eq!(
        (attachment.name.as_str(), attachment.bytes),
        ("notes.bin", 70_000)
    );
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("notes.bin"), b"a file of the person").unwrap();
    let download = member
        .transfers()
        .download(attachment, target.path().to_path_buf());
    let saved = download.finished().await.unwrap();
    assert_eq!(saved, target.path().join("notes (1).bin"));
    assert_eq!(std::fs::read(&saved).unwrap(), content);
    assert_eq!(
        std::fs::read(target.path().join("notes.bin")).unwrap(),
        b"a file of the person"
    );
    assert_eq!(download.progress().stage, DownloadStage::Saved(saved));
    assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 2);
    let picture = member
        .transfers()
        .picture(member.owner().unwrap(), attachment)
        .await
        .unwrap();
    assert_eq!(picture, content);

    // A deleted message with a reply stays as a marker without content. A deleted message
    // without a reply is gone.
    owner.api().delete_message(hello.id).await.unwrap();
    for handle in [owner, member] {
        timeline_when(handle, place, "the marker", |timeline, _| {
            timeline
                .message(hello.id)
                .is_some_and(|message| message.deleted && message.document.is_none())
        })
        .await;
    }
    member.api().delete_message(reply.id).await.unwrap();
    for handle in [owner, member] {
        timeline_when(handle, place, "the reply is gone", |timeline, _| {
            timeline.message(reply.id).is_none()
        })
        .await;
    }
    for handle in [owner, member] {
        agrees_with_the_history(handle, place).await;
    }

    // The message with the file is unread for the member until its position is read.
    let latest = with_file.position;
    let unread = model_when(member, "the unread message", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.latest == latest && state.unread == 1)
    })
    .await;
    assert!(unread.message_state(place).unwrap().last_read < latest);
    member.live().mark_read(place, latest);
    model_when(member, "everything is read", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.unread == 0 && state.last_read == latest)
    })
    .await;
    for handle in [owner, member] {
        handle.live().release_messages(place);
        model_when(handle, "the timeline is dropped", |model| {
            model.timeline(place).is_none()
        })
        .await;
    }
}

async fn live_pair(server: TestServer, member_at: &str) -> Pair {
    let scene = pair(server, member_at).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    scene
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_exchange_messages_in_a_text_channel() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = live_pair(server, &address).await;
    let general = text_channel(&scene.owner, scene.workspace_id, "general").await;
    model_when(&scene.member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    exchange(&scene, PlaceId::Channel(general.id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_exchange_messages_in_the_chat_of_a_voice_channel() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = live_pair(server, &address).await;
    let daily = channel(
        &scene.owner,
        scene.workspace_id,
        "Daily",
        ChannelKind::Voice,
    )
    .await;
    model_when(&scene.member, "the channel", |model| {
        model.channel(daily.id).is_some()
    })
    .await;
    exchange(&scene, PlaceId::Channel(daily.id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_exchange_messages_in_a_direct_conversation() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = live_pair(server, &address).await;
    let member_id = logged_in(&scene.member);
    let conversation = scene
        .owner
        .api()
        .open_conversation(member_id)
        .await
        .unwrap();
    // The event tells both clients, and a second open gives the same conversation.
    for (handle, other) in [
        (&scene.owner, member_id),
        (&scene.member, logged_in(&scene.owner)),
    ] {
        let model = model_when(handle, "the conversation", |model| {
            model.conversation_with(other).is_some()
        })
        .await;
        assert_eq!(model.conversation_with(other).unwrap().id, conversation.id);
    }
    let again = scene
        .member
        .api()
        .open_conversation(logged_in(&scene.owner))
        .await
        .unwrap();
    assert_eq!(again, conversation);
    assert_eq!(
        scene.member.api().conversations().await.unwrap(),
        std::slice::from_ref(&conversation)
    );
    exchange(&scene, PlaceId::Direct(conversation.id)).await;
    // An account cannot open a conversation with itself, and not with an unknown account.
    let api = scene.owner.api();
    let own = api.open_conversation(logged_in(&scene.owner)).await;
    assert!(own.is_err());
    let unknown = api.open_conversation(AccountId::generate()).await;
    assert!(unknown.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_was_offline_gets_the_missed_changes_without_a_duplicate_and_without_a_hole()
{
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let scene = live_pair(server, &proxy.address()).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    model_when(member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    both_open(&scene, place).await;
    let send = |text: &'static str| async move {
        let message = junto_client_core::protocol::message::SendMessage {
            document: document(text),
            reply_to: None,
            attachments: Vec::new(),
        };
        owner.api().send_message(place, &message).await.unwrap()
    };
    let first = send("one").await;
    let second = send("two").await;
    message_when(member, place, "two").await;

    // While the member has no connection: new messages, an edit and a deletion of
    // messages that the member holds.
    proxy.block_streams(true);
    proxy.cut_streams();
    when(member, "the loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    for text in ["three", "four", "five"] {
        send(text).await;
    }
    owner
        .api()
        .edit_message(first.id, document("one, edited"))
        .await
        .unwrap();
    owner.api().delete_message(second.id).await.unwrap();
    let snapshots = proxy.seen("/api/v1/snapshot").len();

    // The events are replayed: the member reads what they name.
    proxy.block_streams(false);
    timeline_when(member, place, "the replay", |timeline, _| {
        timeline.message(second.id).is_none()
            && timeline
                .messages
                .iter()
                .any(|message| text_of(message).as_deref() == Some("five"))
    })
    .await;
    agrees_with_the_history(member, place).await;
    let model = member.live().view().model.unwrap();
    assert_eq!(
        texts(&model, place),
        ["one, edited", "three", "four", "five"]
    );
    assert!(!model.timeline(place).unwrap().stale);
    assert_eq!(proxy.seen("/api/v1/snapshot").len(), snapshots);

    // While the member has no connection, its permissions change: the state is replaced
    // by a snapshot, and the open timeline is read again as one page.
    proxy.block_streams(true);
    proxy.cut_streams();
    when(member, "the second loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    let hidden = text_channel(owner, scene.workspace_id, "plans").await;
    let everyone = owner.live().view().model.unwrap().workspaces[0]
        .roles
        .iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    owner
        .api()
        .set_channel_rule(hidden.id, RuleSubject::Role { role_id: everyone }, &hide)
        .await
        .unwrap();
    for text in ["six", "seven"] {
        send(text).await;
    }
    owner
        .api()
        .edit_message(first.id, document("one, edited twice"))
        .await
        .unwrap();
    proxy.block_streams(false);
    timeline_when(member, place, "the page after the reload", |timeline, _| {
        !timeline.stale
            && timeline
                .messages
                .iter()
                .any(|message| text_of(message).as_deref() == Some("seven"))
    })
    .await;
    assert_eq!(proxy.seen("/api/v1/snapshot").len(), snapshots + 1);
    agrees_with_the_history(member, place).await;
    let model = member.live().view().model.unwrap();
    assert_eq!(
        texts(&model, place),
        ["one, edited twice", "three", "four", "five", "six", "seven"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_history_is_read_by_page_in_both_directions() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = live_pair(server, &address).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    model_when(member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    for number in 1..=90 {
        let message = junto_client_core::protocol::message::SendMessage {
            document: document(&format!("m{number}")),
            reply_to: None,
            attachments: Vec::new(),
        };
        owner.api().send_message(place, &message).await.unwrap();
    }
    let all = history(member, place).await;
    let position_of = |text: &str| {
        all.iter()
            .find(|message| text_of(message).as_deref() == Some(text))
            .unwrap()
            .position
    };
    // The member has read up to the message 40.
    let read = position_of("m40");
    member.api().mark_read(place, read).await.unwrap();
    model_when(member, "the read position", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.last_read == read && state.unread == 50)
    })
    .await;

    // A place with unread messages opens at the read position.
    member.live().open_messages(place).await.unwrap();
    let model = member.live().view().model.unwrap();
    let timeline = model.timeline(place).unwrap();
    assert_eq!(timeline.unread_after, Some(read));
    assert!(timeline.more_before && !timeline.more_after);
    let held = texts(&model, place);
    assert_eq!(held.first().map(String::as_str), Some("m11"));
    assert_eq!(held.last().map(String::as_str), Some("m90"));
    assert_eq!(held.len(), 80);

    // The page before the first loaded message is added without a second copy.
    member.live().read_older_messages(place);
    let model = timeline_when(member, place, "the older page", |timeline, _| {
        !timeline.more_before && !timeline.reading.older
    })
    .await;
    let held: Vec<Message> = model
        .timeline(place)
        .unwrap()
        .messages
        .iter()
        .map(|message| message.as_ref().clone())
        .collect();
    assert_eq!(held, all);

    // The reader names a position; after a reload the page starts there.
    let anchor = position_of("m60");
    member.live().set_message_anchor(place, Some(anchor));
    let hidden = text_channel(owner, scene.workspace_id, "plans").await;
    model_when(member, "the other channel", |model| {
        model.channel(hidden.id).is_some()
    })
    .await;
    let everyone = model.workspaces[0]
        .roles
        .iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    owner
        .api()
        .set_channel_rule(hidden.id, RuleSubject::Role { role_id: everyone }, &hide)
        .await
        .unwrap();
    let model = timeline_when(
        member,
        place,
        "the page from the anchor",
        |timeline, model| {
            model.channel(hidden.id).is_none() && !timeline.stale && !timeline.reading.open
        },
    )
    .await;
    let timeline = model.timeline(place).unwrap();
    assert_eq!(timeline.messages.first().unwrap().position, anchor);
    assert!(timeline.more_before && !timeline.more_after);
    assert_eq!(texts(&model, place).len(), 31);

    // The end of the history replaces what is loaded.
    member.live().read_latest_messages(place);
    let model = timeline_when(member, place, "the latest page", |timeline, model| {
        !timeline.reading.open && texts(model, place).len() == 50
    })
    .await;
    let held = texts(&model, place);
    assert_eq!(held.first().map(String::as_str), Some("m41"));
    assert_eq!(held.last().map(String::as_str), Some("m90"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_whose_answer_was_lost_is_not_sent_a_second_time() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let scene = live_pair(server, &proxy.address()).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    model_when(member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    both_open(&scene, place).await;
    let path = format!("/api/v1/channels/{}/messages", general.id);
    let sent = |text: &str| {
        let text = text.to_owned();
        async move {
            history(owner, place)
                .await
                .iter()
                .filter(|message| text_of(message).as_deref() == Some(text.as_str()))
                .count()
        }
    };

    // The answer is lost and the event of the message arrives: the message is confirmed.
    proxy.fail(&path, [Fault::LoseAnswer]);
    member
        .live()
        .send_message(place, document("lost answer"), None)
        .unwrap();
    model_when(member, "the message is confirmed by its copy", |model| {
        found(model, place, "lost answer").is_some() && model.outbox.is_empty()
    })
    .await;
    assert_eq!(sent("lost answer").await, 1);

    // The answer is lost and no event arrives: the message stays without a confirmation.
    proxy.block_streams(true);
    proxy.cut_streams();
    when(member, "the loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    proxy.fail(&path, [Fault::LoseAnswer]);
    let id = member
        .live()
        .send_message(place, document("no event"), None)
        .unwrap();
    let model = model_when(member, "the message without an answer", |model| {
        model
            .outbox
            .iter()
            .any(|pending| pending.id == id && pending.state == SendState::Unconfirmed)
    })
    .await;
    assert!(found(&model, place, "no event").is_none());
    assert_eq!(sent("no event").await, 1);
    // "Send again" finds the message in the history and sends nothing.
    let requests = proxy.seen(&path).len();
    member.live().send_again(id);
    model_when(member, "the message is found in the history", |model| {
        model.outbox.is_empty()
    })
    .await;
    assert_eq!(sent("no event").await, 1);
    let posts = |from: usize| {
        proxy.seen(&path)[from..]
            .iter()
            .filter(|seen| seen.method == "POST")
            .count()
    };
    assert_eq!(posts(requests), 0);

    // A message that the server refused is sent again at once, and then exists once.
    proxy.fail(&path, [Fault::Busy; 5]);
    let id = member
        .live()
        .send_message(place, document("refused"), None)
        .unwrap();
    model_when(member, "the refusal", |model| {
        model.outbox.iter().any(|pending| {
            pending.id == id
                && matches!(
                    pending.state,
                    SendState::Failed(SendFailure::Refused { .. })
                )
        })
    })
    .await;
    assert_eq!(sent("refused").await, 0);
    member.live().send_again(id);
    model_when(member, "the message is sent", |model| {
        model.outbox.is_empty()
    })
    .await;
    assert_eq!(sent("refused").await, 1);

    // A discarded message leaves the state.
    proxy.fail(&path, [Fault::Busy; 5]);
    let id = member
        .live()
        .send_message(place, document("discarded"), None)
        .unwrap();
    model_when(member, "the second refusal", |model| {
        model
            .outbox
            .iter()
            .any(|pending| pending.id == id && matches!(pending.state, SendState::Failed(_)))
    })
    .await;
    member.live().discard_message(id);
    model_when(member, "nothing waits", |model| model.outbox.is_empty()).await;
    assert_eq!(sent("discarded").await, 0);

    // The messages of one place arrive in the order in which the person sent them.
    proxy.block_streams(false);
    when(member, "the stream", |view| {
        view.status == StreamStatus::Live
    })
    .await;
    let expected: Vec<String> = (0..12).map(|number| format!("ordered {number}")).collect();
    for text in &expected {
        member
            .live()
            .send_message(place, document(text), None)
            .unwrap();
    }
    messages_when(member, place, &expected).await;
    model_when(member, "all are sent", |model| model.outbox.is_empty()).await;
    let ordered: Vec<String> = history(owner, place)
        .await
        .iter()
        .filter_map(text_of)
        .filter(|text| text.starts_with("ordered"))
        .collect();
    assert_eq!(ordered, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_continues_after_a_cut_connection() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let scene = live_pair(server, &proxy.address()).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    model_when(member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    both_open(&scene, place).await;

    // Three parts and a rest. The answer to the second part is lost after the server
    // stored it, and the third part does not reach the server at first.
    let megabyte = 1024 * 1024;
    let content: Vec<u8> = (0..3 * megabyte + 1234)
        .map(|number: usize| (number % 253) as u8)
        .collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &content).unwrap();
    proxy.fail_ending(&format!("/chunks/{megabyte}"), [Fault::LoseAnswer]);
    proxy.fail_ending(
        &format!("/chunks/{}", 2 * megabyte),
        [Fault::Cut, Fault::Busy],
    );
    // The read of the status that follows the lost answer waits at the proxy: the
    // progress tells the interruption until the test has seen it.
    proxy.hold_ending("/upload");
    let upload = member
        .transfers()
        .upload(place, UploadSource::File(file.path().to_path_buf()));
    let mut progress = upload.watch();
    tokio::time::timeout(PATIENCE, progress.wait_for(|now| now.interrupted))
        .await
        .expect("the progress told no interruption")
        .unwrap();
    proxy.release();
    let attachment_id = loop {
        let now = *progress.borrow_and_update();
        match now.stage {
            UploadStage::Complete(attachment_id) => break attachment_id,
            UploadStage::Failed(failure) => panic!("the upload failed: {failure:?}"),
            _ => {}
        }
        tokio::time::timeout(PATIENCE, progress.changed())
            .await
            .expect("the upload did not end")
            .unwrap();
    };
    assert_eq!(upload.progress().sent, content.len() as u64);
    // The part whose answer was lost is not sent again: the status told that the server
    // holds it. The part that did not arrive is sent until the server has it.
    let sent = |offset: usize| {
        let end = format!("/chunks/{offset}");
        proxy
            .all()
            .iter()
            .filter(|seen| seen.path.ends_with(&end))
            .count()
    };
    assert_eq!(
        [
            sent(0),
            sent(megabyte),
            sent(2 * megabyte),
            sent(3 * megabyte)
        ],
        [1, 1, 3, 1]
    );

    member
        .transfers()
        .send(place, document("a large file"), None, &[upload])
        .unwrap();
    let message = message_when(owner, place, "a large file").await;
    assert_eq!(message.attachments[0].id, attachment_id);
    let target = tempfile::tempdir().unwrap();
    let saved = owner
        .transfers()
        .download(&message.attachments[0], target.path().to_path_buf())
        .finished()
        .await
        .unwrap();
    assert_eq!(std::fs::read(saved).unwrap(), content);

    // A cancelled upload is deleted on the server, and a message with it is not sent.
    proxy.hold_ending("/chunks/0");
    let cancelled = member
        .transfers()
        .upload(place, UploadSource::File(file.path().to_path_buf()));
    let id = member
        .transfers()
        .send(
            place,
            document("never sent"),
            None,
            std::slice::from_ref(&cancelled),
        )
        .unwrap();
    model_when(member, "the message waits for its file", |model| {
        model
            .outbox
            .iter()
            .any(|pending| pending.id == id && pending.state == SendState::WaitingForFiles)
    })
    .await;
    // The server answered the start of the upload, and the first part waits at the proxy.
    let part = tokio::time::timeout(PATIENCE, async {
        loop {
            let parts: Vec<String> = proxy
                .all()
                .into_iter()
                .filter(|seen| seen.path.ends_with("/chunks/0"))
                .map(|seen| seen.path)
                .collect();
            if parts.len() == 2 {
                break parts[1].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the first part was not sent");
    let held: AttachmentId = part
        .trim_end_matches("/chunks/0")
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(member.api().upload_status(held).await.is_ok());
    cancelled.cancel();
    proxy.release();
    model_when(member, "the message has no file", |model| {
        model.outbox.iter().any(|pending| {
            pending.id == id && pending.state == SendState::Failed(SendFailure::File)
        })
    })
    .await;
    assert_eq!(cancelled.progress().stage, UploadStage::Cancelled);
    upload_gone(member, held).await;
    member.live().discard_message(id);
    model_when(member, "nothing waits", |model| model.outbox.is_empty()).await;
    assert!(found(&member.live().view().model.unwrap(), place, "never sent").is_none());
}

/// Waits until the server holds no upload with this identifier for the account.
async fn upload_gone(handle: &ServerHandle, attachment_id: AttachmentId) {
    let api = handle.api();
    tokio::time::timeout(PATIENCE, async {
        loop {
            match api.upload_status(attachment_id).await {
                Err(error) if error.category() == Some(ErrorCategory::NotFound) => return,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .expect("the server still holds the upload");
}

/// Two clients with the text channel `general` open, the member behind the proxy.
async fn pair_in_general(
    proxy_of: impl AsyncFnOnce(&TestServer) -> FaultProxy,
) -> (Pair, FaultProxy, PlaceId) {
    let server = TestServer::start("Acme").await;
    let proxy = proxy_of(&server).await;
    let scene = live_pair(server, &proxy.address()).await;
    let general = text_channel(&scene.owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    model_when(&scene.member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    both_open(&scene, place).await;
    (scene, proxy, place)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_complete_upload_that_is_cancelled_is_deleted_on_the_server() {
    let (scene, _proxy, place) = pair_in_general(FaultProxy::start).await;
    let member = &scene.member;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"some bytes").unwrap();
    let source = UploadSource::File(file.path().to_path_buf());

    // A complete upload counts as an upload of the account until a message has it. The
    // person removes the file from the message: the server deletes it.
    let removed = member.transfers().upload(place, source.clone());
    let attachment_id = tokio::time::timeout(PATIENCE, removed.finished())
        .await
        .expect("the upload did not end")
        .unwrap();
    assert!(
        member
            .api()
            .upload_status(attachment_id)
            .await
            .unwrap()
            .complete
    );
    removed.cancel();
    upload_gone(member, attachment_id).await;
    let mut progress = removed.watch();
    tokio::time::timeout(
        PATIENCE,
        progress.wait_for(|now| now.stage == UploadStage::Cancelled),
    )
    .await
    .expect("the upload was not told as cancelled")
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_that_failed_ends_the_wait_of_its_message() {
    let (scene, proxy, place) = pair_in_general(FaultProxy::start).await;
    let member = &scene.member;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"some bytes").unwrap();
    let source = UploadSource::File(file.path().to_path_buf());

    // The first file of the message is still on its way when the second one has failed.
    proxy.hold_ending("/chunks/0");
    let slow = member.transfers().upload(place, source);
    let empty = tempfile::NamedTempFile::new().unwrap();
    let failed = member
        .transfers()
        .upload(place, UploadSource::File(empty.path().to_path_buf()));
    let id = member
        .transfers()
        .send(place, document("two files"), None, &[slow.clone(), failed])
        .unwrap();
    model_when(member, "the message has no file", |model| {
        model.outbox.iter().any(|pending| {
            pending.id == id && pending.state == SendState::Failed(SendFailure::File)
        })
    })
    .await;
    assert!(!matches!(
        slow.progress().stage,
        UploadStage::Complete(_) | UploadStage::Failed(_) | UploadStage::Cancelled
    ));
    slow.cancel();
    proxy.release();
    member.live().discard_message(id);
    model_when(member, "nothing waits", |model| model.outbox.is_empty()).await;
}

/// A second device of the account of `first`.
async fn second_device(server_address: &str, first: &ServerHandle) -> (Device, ServerHandle) {
    let device = Device::new();
    let newcomer = device.add(server_address).await;
    newcomer
        .dispatch(AccessAction::RequestLink {
            device_name: "Second".to_owned(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForLink { code, .. } = newcomer.access().state else {
        panic!("not waiting for a link");
    };
    let approval = first.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
    newcomer.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(logged_in(&newcomer), logged_in(first));
    (device, newcomer)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_unread_counts_are_the_same_on_two_devices_of_one_account() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let address = proxy.address();
    let scene = live_pair(server, &address).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let (other_device, member_too) = second_device(&address, member).await;
    live(&other_device, &member_too).await;
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    let member_id = logged_in(member);
    let devices = [member, &member_too];
    let counts_when = |unread: u64, mentions: u64, last_read: Option<u64>| async move {
        for handle in devices {
            model_when(handle, "the unread counts", |model| {
                model.message_state(place).is_some_and(|state| {
                    state.unread == unread
                        && state.mentions == mentions
                        && last_read.is_none_or(|position| state.last_read == position)
                })
            })
            .await;
        }
    };

    let mut positions = Vec::new();
    for text in ["one", "two"] {
        let message = junto_client_core::protocol::message::SendMessage {
            document: document(text),
            reply_to: None,
            attachments: Vec::new(),
        };
        positions.push(
            owner
                .api()
                .send_message(place, &message)
                .await
                .unwrap()
                .position,
        );
    }
    let mention = junto_client_core::protocol::message::SendMessage {
        document: Document {
            blocks: vec![Block::Paragraph {
                content: vec![Inline::Account {
                    account_id: member_id,
                }],
            }],
        },
        reply_to: None,
        attachments: Vec::new(),
    };
    let mentioned: Message = owner.api().send_message(place, &mention).await.unwrap();
    counts_when(3, 1, Some(0)).await;

    // One device reads a part: the other device has the same counts.
    member.live().mark_read(place, positions[1]);
    counts_when(1, 1, Some(positions[1])).await;
    // A position below the read one changes nothing.
    member_too.live().mark_read(place, positions[0]);
    member_too.live().mark_read(place, mentioned.position);
    counts_when(0, 0, Some(mentioned.position)).await;

    // A message of the account itself is not unread, on either device. The server moves
    // the read position of the account to the message: the devices show the message as
    // the latest one before the summary that its change asks for arrives.
    proxy.hold_answer_ending("GET", "/messages/read");
    let reply = member_too
        .live()
        .send_message(place, document("mine"), None);
    assert!(reply.is_some());
    let message_id: MessageId = loop {
        if let Some(message) = history(owner, place)
            .await
            .iter()
            .find(|message| text_of(message).as_deref() == Some("mine"))
        {
            break message.id;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let own = owner.api().message(message_id).await.unwrap();
    counts_when(0, 0, Some(own.position)).await;
    for handle in devices {
        let model = handle.live().view().model.unwrap();
        let state = model.message_state(place).unwrap();
        assert_eq!(
            (state.last_read, state.latest),
            (own.position, own.position)
        );
    }
    proxy.release();
    for handle in [member, &member_too] {
        let state = handle.api().message_state(place).await.unwrap();
        assert_eq!(
            handle
                .live()
                .view()
                .model
                .unwrap()
                .message_state(place)
                .unwrap(),
            &state
        );
    }
}
