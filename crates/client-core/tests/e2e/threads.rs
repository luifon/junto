//! Threads, forks, pins, notices, search and direct conversations of the client core
//! against the real server (ADR 0722).

use junto_client_core::ServerHandle;
use junto_client_core::api::{ErrorCategory, PageQuery, SearchQuery};
use junto_client_core::live::LoadError;
use junto_client_core::model::{Named, PlaceId, ServerModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::discovery::NotificationKind;
use junto_client_core::protocol::ids::{ChannelId, MessageId};
use junto_client_core::protocol::message::{Block, Document, Inline, Message};
use junto_client_core::protocol::structure::{ChannelKind, SetChannelOverride};

use crate::messages::{both_open, document, found, message_when, texts, timeline_when};
use crate::stream::{Pair, channel, live, model_when, pair, text_channel};
use crate::support::{TestServer, logged_in};

fn thread_of(channel_id: ChannelId, root: &Message) -> PlaceId {
    PlaceId::Thread {
        channel_id,
        root_id: root.id,
    }
}

/// A text that names an account.
fn mention(text: &str, account_id: junto_client_core::protocol::ids::AccountId) -> Document {
    Document {
        blocks: vec![Block::Paragraph {
            content: vec![
                Inline::Text {
                    text: text.to_owned(),
                    marks: Vec::new(),
                },
                Inline::Account { account_id },
            ],
        }],
    }
}

fn root_in(model: &ServerModel, place: PlaceId, root_id: MessageId) -> &Message {
    model.timeline(place).unwrap().message(root_id).unwrap()
}

fn texts_of(timeline: &junto_client_core::model::Timeline) -> Vec<String> {
    timeline
        .messages
        .iter()
        .filter_map(|message| crate::messages::text_of(message))
        .collect()
}

/// Two clients with a text channel open on both, and a message of the owner in it.
pub(crate) async fn scene_with_root(name: &str) -> (Pair, PlaceId, std::sync::Arc<Message>) {
    let server = TestServer::start(name).await;
    let address = server.address();
    let scene = pair(server, &address).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    let general = text_channel(&scene.owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    both_open(&scene, place).await;
    scene
        .owner
        .live()
        .send_message(place, document("root"), None)
        .unwrap();
    let root = message_when(&scene.member, place, "root").await;
    (scene, place, root)
}

pub(crate) async fn open(handle: &ServerHandle, place: PlaceId) {
    handle.live().open_messages(place).await.unwrap();
}

/// Stores a finished recording of a voice channel with one transcript line, as the server
/// holds it after the transcription.
pub async fn seed_transcript(scene: &Pair, voice: ChannelId, text: &str) -> uuid::Uuid {
    use sqlx::ConnectOptions;
    use std::str::FromStr;
    let options = sqlx::postgres::PgConnectOptions::from_str(scene.server.database_url()).unwrap();
    let mut connection = options.connect().await.unwrap();
    let recording = uuid::Uuid::now_v7();
    let segment = uuid::Uuid::now_v7();
    let owner = logged_in(&scene.owner);
    sqlx::query(
        "insert into recordings
             (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,
              requested_at, recording_since, stop_reason, stop_requested_at, ended_at,
              processed_at)
         values ($1, $2, $3, gen_random_uuid(), $4, 'ready', $5, 'auto', now(), now(),
                 'call_ended', now(), now(), now())",
    )
    .bind(recording)
    .bind(scene.workspace_id.as_uuid())
    .bind(voice.as_uuid())
    .bind(format!("jr-{}-{}", "0".repeat(12), recording.simple()))
    .bind(owner.as_uuid())
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into recording_segments
             (id, recording_id, source, state, job_id, job_started_at, requested_path,
              requested_at)
         values ($1, $2, 'composite', 'ended', $3, now(), 'seeded/composite.mp4', now())",
    )
    .bind(segment)
    .bind(recording)
    .bind(format!("seeded-{}", segment.simple()))
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into transcripts (recording_id, state, requested_at, finished_at, lines)
         values ($1, 'ready', now(), now(), 1)",
    )
    .bind(recording)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into transcript_lines
             (recording_id, line, start_ms, end_ms, segment_id, source, account_id, text, language)
         values ($1, 0, 0, 500, $2, 'microphone', $3, $4, 'en')",
    )
    .bind(recording)
    .bind(segment)
    .bind(owner.as_uuid())
    .bind(text)
    .execute(&mut connection)
    .await
    .unwrap();
    recording
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_use_a_thread_with_its_unread_state_a_mention_and_its_notice() {
    let (scene, place, root) = scene_with_root("Threads").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let PlaceId::Channel(channel_id) = place else {
        unreachable!()
    };
    let thread = thread_of(channel_id, &root);

    // The member opens the thread and writes the first reply.
    open(member, thread).await;
    let model = member.live().view().model.unwrap();
    assert_eq!(
        model.timeline(thread).unwrap().root.as_ref().unwrap().id,
        root.id
    );
    member
        .live()
        .send_message(thread, document("first reply"), None)
        .unwrap();
    let first = message_when(member, thread, "first reply").await;
    assert_eq!(first.thread_root, Some(root.id));

    // The root of both clients counts the reply, and the reply is in no channel timeline.
    for handle in [owner, member] {
        let model = timeline_when(handle, place, "the reply count", |timeline, _| {
            timeline
                .message(root.id)
                .is_some_and(|root| root.reply_count == 1)
        })
        .await;
        assert_eq!(root_in(&model, place, root.id).last_reply, Some(first.id));
        assert_eq!(texts(&model, place), ["root"]);
    }

    // The owner wrote the root: the thread is unread for the owner, with its root named.
    let model = model_when(owner, "the unread thread with its notice", |model| {
        model
            .thread_state(root.id)
            .is_some_and(|state| state.unread == 1)
            && matches!(model.named(root.id), Some(Named::Read(_)))
            && model.notices.iter().any(|notice| {
                notice.message_id == first.id && notice.kind == NotificationKind::Thread
            })
    })
    .await;
    // The reply does not count as an unread message of the channel.
    assert_eq!(model.message_state(place).unwrap().unread, 0);
    // Nothing is unread for the member, who wrote the reply.
    let model = member.live().view().model.unwrap();
    assert!(model.thread_state(root.id).is_none());

    // The owner opens the thread: it starts with the reply, and reading it empties the
    // list and marks the notice as read.
    open(owner, thread).await;
    let model = owner.live().view().model.unwrap();
    assert_eq!(texts(&model, thread), ["first reply"]);
    owner.live().mark_read(thread, first.position);
    model_when(owner, "the read thread", |model| {
        model.threads.is_empty()
            && model
                .notices
                .iter()
                .all(|notice| notice.message_id != first.id || notice.read_at.is_some())
    })
    .await;

    // The owner answers and names the member: a notice of a mention, an unread thread
    // with a mention, and a mention in the summary of the channel.
    let member_id = logged_in(member);
    owner
        .live()
        .send_message(thread, mention("for you ", member_id), Some(first.id))
        .unwrap();
    let model = model_when(member, "the mention", |model| {
        model
            .thread_state(root.id)
            .is_some_and(|state| state.unread == 1 && state.mentions == 1)
            && model
                .message_state(place)
                .is_some_and(|state| state.mentions == 1)
            && model.unread_notices().count() == 1
    })
    .await;
    let notice = model.unread_notices().next().unwrap().clone();
    assert_eq!(notice.kind, NotificationKind::Mention);
    let second = model_when(member, "the message of the notice", |model| {
        matches!(model.named(notice.message_id), Some(Named::Read(_)))
    })
    .await;
    let Some(Named::Read(named)) = second.named(notice.message_id) else {
        unreachable!()
    };
    assert_eq!(named.thread_root, Some(root.id));
    assert_eq!(named.reply_to, Some(first.id));
    assert_eq!(texts(&second, thread), ["first reply", "for you "]);

    // The member reads the thread: no mention is left anywhere.
    member.live().mark_read(thread, named.position);
    model_when(member, "nothing unread", |model| {
        model.threads.is_empty()
            && model
                .message_state(place)
                .is_some_and(|state| state.mentions == 0)
            && model.unread_notices().count() == 0
    })
    .await;

    // A second device state: the route of the list agrees with the state.
    let listed = member.api().unread_threads(None, None).await.unwrap();
    assert!(listed.threads.is_empty() && !listed.more);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_use_a_pin_a_fork_a_search_and_a_direct_conversation() {
    let (scene, place, root) = scene_with_root("Pins").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let PlaceId::Channel(channel_id) = place else {
        unreachable!()
    };
    let thread = thread_of(channel_id, &root);
    open(owner, thread).await;
    member
        .api()
        .reply_in_thread(
            root.id,
            &junto_client_core::protocol::message::SendMessage {
                document: document("quartz reply"),
                reply_to: None,
                attachments: Vec::new(),
            },
        )
        .await
        .unwrap();
    message_when(owner, thread, "quartz reply").await;

    // Pins: the owner may pin, the member may not. The open list follows the change.
    owner.live().open_pins(channel_id).await.unwrap();
    let pinned = |model: &ServerModel| -> Vec<MessageId> {
        model
            .pin_list(channel_id)
            .and_then(|list| list.messages.loaded())
            .map(|pins| pins.iter().map(|pin| pin.id).collect())
            .unwrap_or_default()
    };
    assert!(pinned(&owner.live().view().model.unwrap()).is_empty());
    let refused = member.api().pin_message(root.id).await.unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    owner.api().pin_message(root.id).await.unwrap();
    model_when(owner, "the pin in the list", |model| {
        pinned(model) == [root.id]
    })
    .await;
    for handle in [owner, member] {
        timeline_when(handle, place, "the pin on the message", |timeline, _| {
            timeline.message(root.id).is_some_and(|root| root.pinned)
        })
        .await;
    }
    assert_eq!(member.api().pins(channel_id).await.unwrap().len(), 1);
    owner.api().unpin_message(root.id).await.unwrap();
    model_when(owner, "no pin", |model| pinned(model).is_empty()).await;
    owner.live().release_pins(channel_id);

    // A fork: the member may not; the owner copies the thread into a channel, and a
    // second fork is refused.
    let refused = member
        .api()
        .fork_thread(root.id, "forked", ChannelKind::Text)
        .await
        .unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    let fork = owner
        .api()
        .fork_thread(root.id, "forked", ChannelKind::Text)
        .await
        .unwrap();
    for handle in [owner, member] {
        model_when(handle, "the fork channel", |model| {
            model.channel(fork.id).is_some()
        })
        .await;
    }
    timeline_when(owner, thread, "the fork of the thread", |timeline, _| {
        timeline.forks == [fork.id]
    })
    .await;
    let copies = owner
        .api()
        .messages(PlaceId::Channel(fork.id), &PageQuery::default())
        .await
        .unwrap();
    let copied: Vec<String> = copies
        .messages
        .iter()
        .filter_map(crate::messages::text_of)
        .collect();
    assert_eq!(copied, ["root", "quartz reply"]);
    let again = owner
        .api()
        .fork_thread(root.id, "forked again", ChannelKind::Text)
        .await
        .unwrap_err();
    assert_eq!(again.category(), Some(ErrorCategory::Conflict));

    // Search: a message, a reply of a thread, and a line of a transcript.
    let voice = channel(owner, scene.workspace_id, "Daily", ChannelKind::Voice).await;
    let recording = seed_transcript(&scene, voice.id, "the quartz was said aloud").await;
    let query = |text: &str| SearchQuery {
        text: text.to_owned(),
        ..SearchQuery::default()
    };
    let hits = member.api().search(&query("quartz")).await.unwrap();
    assert!(hits.next.is_none());
    let reply_hits: Vec<_> = hits
        .results
        .iter()
        .filter(|hit| hit.message_id.is_some())
        .collect();
    // The reply of the thread and its copy in the fork.
    assert_eq!(reply_hits.len(), 2);
    let spoken: Vec<_> = hits
        .results
        .iter()
        .filter(|hit| hit.recording_id.is_some())
        .collect();
    assert_eq!(spoken.len(), 1);
    assert_eq!(spoken[0].recording_id.unwrap().as_uuid(), recording);
    assert_eq!(spoken[0].line, Some(0));
    assert_eq!(spoken[0].place_id, voice.id.as_uuid());
    assert_eq!(spoken[0].text, "the quartz was said aloud");
    // The filters: one channel, one author, and a time that excludes everything.
    let in_channel = SearchQuery {
        channel_id: Some(channel_id),
        ..query("quartz")
    };
    let hits = member.api().search(&in_channel).await.unwrap();
    assert_eq!(hits.results.len(), 1);
    assert_eq!(hits.results[0].author_id, logged_in(member));
    let by_owner = SearchQuery {
        author_id: Some(logged_in(owner)),
        ..query("quartz")
    };
    let hits = member.api().search(&by_owner).await.unwrap();
    assert!(hits.results.iter().all(|hit| hit.recording_id.is_some()));
    let earlier = SearchQuery {
        until: Some(root.created_at - time::Duration::days(1)),
        ..query("quartz")
    };
    assert!(
        member
            .api()
            .search(&earlier)
            .await
            .unwrap()
            .results
            .is_empty()
    );
    // A result is shown in its place. The reply of the thread is found by its message.
    let hit = member.api().search(&in_channel).await.unwrap().results[0].clone();
    let found_reply = member.api().message(hit.message_id.unwrap()).await.unwrap();
    assert_eq!(PlaceId::of_message(&found_reply), thread);
    // A result that lies before the loaded page of its channel: the page around its
    // position replaces what is loaded.
    for number in 0..60 {
        let later = junto_client_core::protocol::message::SendMessage {
            document: document(&format!("later {number}")),
            reply_to: None,
            attachments: Vec::new(),
        };
        owner.api().send_message(place, &later).await.unwrap();
    }
    member.live().read_latest_messages(place);
    timeline_when(member, place, "the latest page", |timeline, _| {
        timeline.message(root.id).is_none()
            && timeline.more_before
            && texts_of(timeline).last().map(String::as_str) == Some("later 59")
    })
    .await;
    let found_root = member
        .api()
        .search(&SearchQuery {
            channel_id: Some(channel_id),
            ..query("root")
        })
        .await
        .unwrap()
        .results
        .remove(0);
    assert_eq!(found_root.message_id, Some(root.id));
    member.live().read_messages_around(place, root.position);
    let model = timeline_when(
        member,
        place,
        "the result with what follows it",
        |timeline, _| timeline.message(root.id).is_some(),
    )
    .await;
    let around = model.timeline(place).unwrap();
    assert!(around.more_after || around.messages.len() > 50);
    assert!(texts_of(around).iter().any(|text| text == "later 0"));

    // A direct conversation: the member opens one with the owner and writes.
    let conversation = member
        .api()
        .open_conversation(logged_in(owner))
        .await
        .unwrap();
    let direct = PlaceId::Direct(conversation.id);
    for handle in [owner, member] {
        model_when(handle, "the conversation", |model| {
            model.place(direct).is_some()
        })
        .await;
    }
    open(member, direct).await;
    member
        .live()
        .send_message(direct, document("hello, directly"), None)
        .unwrap();
    let model = model_when(owner, "the unread direct message", |model| {
        model
            .message_state(direct)
            .is_some_and(|state| state.unread == 1)
            && model
                .unread_notices()
                .any(|notice| notice.kind == NotificationKind::Direct)
    })
    .await;
    assert!(model.conversation_with(logged_in(member)).is_some());
    open(owner, direct).await;
    let hello = message_when(owner, direct, "hello, directly").await;
    // A direct conversation has no thread and no pin (ADR 0409).
    let no_thread = owner
        .api()
        .reply_in_thread(
            hello.id,
            &junto_client_core::protocol::message::SendMessage {
                document: document("no"),
                reply_to: None,
                attachments: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(no_thread.category(), Some(ErrorCategory::ClientFault));
    assert!(found(&owner.live().view().model.unwrap(), direct, "no").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_loses_a_channel_loses_its_threads_its_notices_and_its_results() {
    let (scene, place, root) = scene_with_root("Revoked").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let PlaceId::Channel(channel_id) = place else {
        unreachable!()
    };
    let thread = thread_of(channel_id, &root);
    let member_id = logged_in(member);
    // The member has the thread open, an unread mention in it, and a pin list.
    open(member, thread).await;
    owner
        .live()
        .send_message(thread, mention("onyx for ", member_id), None)
        .unwrap();
    owner.api().pin_message(root.id).await.unwrap();
    member.live().open_pins(channel_id).await.unwrap();
    model_when(member, "the unread mention", |model| {
        model.thread_state(root.id).is_some()
            && model.unread_notices().count() == 1
            && matches!(model.named(root.id), Some(Named::Read(_)))
    })
    .await;
    let query = SearchQuery {
        text: "onyx".to_owned(),
        ..SearchQuery::default()
    };
    assert_eq!(member.api().search(&query).await.unwrap().results.len(), 1);

    // The channel is hidden from the "everyone" role.
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
        .set_channel_override(channel_id, everyone, &hide)
        .await
        .unwrap();

    // The state of the member holds nothing of the channel any more.
    let model = model_when(member, "the state without the channel", |model| {
        model.channel(channel_id).is_none()
    })
    .await;
    assert!(model.timeline(thread).is_none() && model.timeline(place).is_none());
    assert!(model.threads.is_empty());
    assert!(model.notices.is_empty());
    assert!(model.named.is_empty());
    assert!(model.pin_list(channel_id).is_none());
    assert!(model.message_state(place).is_none());
    // The routes answer in the same way.
    assert!(
        member
            .api()
            .search(&query)
            .await
            .unwrap()
            .results
            .is_empty()
    );
    assert!(
        member
            .api()
            .notifications(None, None)
            .await
            .unwrap()
            .notifications
            .is_empty()
    );
    let pins = member.api().pins(channel_id).await.unwrap_err();
    assert_eq!(pins.category(), Some(ErrorCategory::NotFound));
    let reopened = member.live().open_messages(thread).await;
    assert!(matches!(reopened, Err(LoadError::NotInState)));
    // The owner still holds everything.
    let model = owner.live().view().model.unwrap();
    assert!(model.channel(channel_id).is_some());
    assert_eq!(owner.api().search(&query).await.unwrap().results.len(), 1);
}
