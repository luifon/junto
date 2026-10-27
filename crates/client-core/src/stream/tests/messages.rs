//! The messages in the stream client, against routes that the test plays: what an event
//! of a message makes the client read, the bound on those reads, the read position, a
//! reload, and a message whose answer is lost.

use junto_protocol::discovery::{Notification, NotificationPage};
use junto_protocol::ids::{ChannelId, MessageId};
use junto_protocol::message::{
    AttachmentLimits, MarkRead, Message, MessagePage, MessageState, SendMessage, ThreadPage,
    ThreadState, UnreadThreads,
};

use super::*;
use crate::api::Method;
use crate::model::message_tests::{document, message};
use crate::model::{PlaceId, SendState};

/// The messages of the text channel of the scene as the played server holds them.
#[derive(Default)]
pub(super) struct Chat {
    pub(super) messages: Vec<Message>,
    pub(super) last_read: u64,
    /// The read positions that the client sent, in their order.
    pub(super) marked: Vec<u64>,
    /// The documents of the messages that the client sent, in their order.
    pub(super) posted: Vec<String>,
    /// The next sends are stored, and their answers are lost.
    pub(super) lose_answers: usize,
    /// The account that the client writes as.
    pub(super) own: Option<AccountId>,
    /// The answer to the next read position that the client sends: a summary that the
    /// server made before a later change.
    pub(super) mark_answer: Option<MessageState>,
    /// The replies of the threads of the text channel.
    pub(super) replies: Vec<Message>,
    /// The threads that hold something unread for the account.
    pub(super) threads: Vec<ThreadState>,
    /// The channels that the threads were forked into.
    pub(super) forks: Vec<ChannelId>,
    pub(super) notices: Vec<Notification>,
    /// The server holds notices after the listed ones.
    pub(super) notices_more: bool,
    /// The notices that the server refuses as the cursor of a page.
    pub(super) refused_cursors: Vec<uuid::Uuid>,
    /// The read positions of threads that the client sent, in their order.
    pub(super) thread_marked: Vec<(MessageId, u64)>,
    /// The answer to the next read position of a thread: a state that the server made
    /// before a later change.
    pub(super) thread_mark_answer: Option<ThreadState>,
    /// A request for one of these messages is answered with another message.
    pub(super) answered_wrongly: Vec<MessageId>,
    /// The summary of the channel says that the account muted it.
    pub(super) muted: bool,
    /// The receipt of the answer to the next send (ADR 0214).
    pub(super) send_receipt: crate::api::Receipt,
}

pub(super) fn json<T: serde::Serialize>(
    status: u16,
    body: &T,
) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse {
        status,
        retry_after: None,
        body: serde_json::to_vec(body).unwrap().into(),
        receipt: crate::api::Receipt::NotStated,
    })
}

pub(super) fn not_found() -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse {
        status: 404,
        retry_after: None,
        body: br#"{"error":{"code":"not_found","message":""}}"#.to_vec().into(),
        receipt: crate::api::Receipt::NotStated,
    })
}

impl Chat {
    fn state(&self) -> Option<MessageState> {
        let own = self.own?;
        let place = self.messages.first()?.place;
        let unread = self
            .messages
            .iter()
            .filter(|held| held.position > self.last_read && held.author_id != Some(own))
            .count() as u64;
        Some(MessageState {
            place,
            last_read: self.last_read,
            latest: self.messages.last().map_or(0, |held| held.position),
            unread,
            mentions: 0,
            unread_sign: unread > 0 && !self.muted,
            notification_mentions: 0,
            muted: self.muted,
        })
    }

    fn page(&self, request: &HttpRequest) -> MessagePage {
        Self::page_of(&self.messages, request)
    }

    fn page_of(messages: &[Message], request: &HttpRequest) -> MessagePage {
        let value = |name: &str| {
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let position_of = |name: &str| {
            value(name)
                .and_then(|id| id.parse::<MessageId>().ok())
                .and_then(|id| messages.iter().find(|held| held.id == id))
                .map(|held| held.position)
        };
        let limit: usize = value("limit").map_or(50, |limit| limit.parse().unwrap());
        let after = value("after")
            .map(|after| after.parse::<u64>().unwrap())
            .or_else(|| position_of("after_message"));
        let before = value("before")
            .map(|before| before.parse::<u64>().unwrap())
            .or_else(|| position_of("before_message"));
        let mut found: Vec<Message> = messages
            .iter()
            .filter(|held| {
                after.is_none_or(|after| held.position > after)
                    && before.is_none_or(|before| held.position < before)
            })
            .cloned()
            .collect();
        let has_more = found.len() > limit;
        if after.is_some() {
            found.truncate(limit);
        } else {
            found = found.split_off(found.len().saturating_sub(limit));
        }
        MessagePage {
            messages: found,
            has_more,
        }
    }

    /// The answer to a request of a message route. `None` for another route.
    pub(super) fn answer(
        &mut self,
        request: &HttpRequest,
    ) -> Option<Result<HttpResponse, TransportError>> {
        let path = request.url.path();
        if path.ends_with("/attachment-limits") {
            return Some(json(
                200,
                &AttachmentLimits {
                    bytes_per_file: 1_000,
                    files_per_message: 2,
                },
            ));
        }
        if path.ends_with("/messages/read") {
            if request.method == Method::Put {
                let body: MarkRead =
                    serde_json::from_slice(request.body.as_ref()?.as_slice()).unwrap();
                self.marked.push(body.position);
                self.last_read = self.last_read.max(body.position);
                if let Some(answer) = self.mark_answer.take() {
                    return Some(json(200, &answer));
                }
            }
            return Some(match self.state() {
                Some(state) => json(200, &state),
                None => not_found(),
            });
        }
        if path.ends_with("/threads/unread") {
            return Some(json(
                200,
                &UnreadThreads {
                    threads: self.threads.clone(),
                    more: false,
                    next: None,
                },
            ));
        }
        if path.ends_with("/notifications") {
            let refused = request.url.query_pairs().any(|(name, value)| {
                name == "before"
                    && self
                        .refused_cursors
                        .iter()
                        .any(|cursor| cursor.to_string() == value)
            });
            if refused {
                return Some(Ok(HttpResponse {
                    status: 400,
                    retry_after: None,
                    body: br#"{"error":{"code":"invalid_request","message":"invalid notification cursor"}}"#
                        .to_vec()
                        .into(),
                    receipt: crate::api::Receipt::NotStated,
                }));
            }
            return Some(json(
                200,
                &NotificationPage {
                    notifications: self.notices.clone(),
                    has_more: self.notices_more,
                },
            ));
        }
        if path.ends_with("/pins") {
            let pins: Vec<&Message> = self
                .messages
                .iter()
                .chain(&self.replies)
                .filter(|held| held.pinned)
                .collect();
            return Some(json(200, &pins));
        }
        if let Some(rest) = path.strip_prefix("/api/v1/messages/") {
            let (id, route) = rest.split_once('/').unwrap_or((rest, ""));
            let id: MessageId = id.parse().unwrap();
            if route.is_empty() && self.answered_wrongly.contains(&id) {
                return Some(json(200, &self.messages[0]));
            }
            return Some(match route {
                "" => match self
                    .messages
                    .iter()
                    .chain(&self.replies)
                    .find(|held| held.id == id)
                {
                    Some(held) => json(200, held),
                    None => not_found(),
                },
                "thread/read" => self.thread_read(id, request),
                "thread" => self.thread(id, request),
                other => panic!("the stream client asked for the route {other} of a message"),
            });
        }
        if !path.ends_with("/messages") {
            return None;
        }
        if request.method == Method::Get {
            return Some(json(200, &self.page(request)));
        }
        let body: SendMessage = serde_json::from_slice(request.body.as_ref()?.as_slice()).unwrap();
        let place = self.messages.first()?.place;
        let position = self.messages.last().map_or(0, |held| held.position) + 1;
        let mut sent = message(place, position, self.own?, "");
        sent.document = Some(body.document);
        sent.reply_to = body.reply_to;
        self.posted
            .push(serde_json::to_string(&sent.document).unwrap());
        self.messages.push(sent.clone());
        if self.lose_answers > 0 {
            self.lose_answers -= 1;
            return Some(Err(TransportError::new(
                TransportErrorKind::Interrupted,
                "the answer was lost",
            )));
        }
        let mut answer = json(201, &sent);
        if let Ok(answer) = &mut answer {
            answer.receipt = std::mem::take(&mut self.send_receipt);
        }
        Some(answer)
    }
}

impl Chat {
    fn thread_read(
        &mut self,
        root_id: MessageId,
        request: &HttpRequest,
    ) -> Result<HttpResponse, TransportError> {
        if request.method == Method::Put {
            let body: MarkRead =
                serde_json::from_slice(request.body.as_ref().unwrap().as_slice()).unwrap();
            self.thread_marked.push((root_id, body.position));
            if let Some(answer) = self.thread_mark_answer.take() {
                return json(200, &answer);
            }
            if let Some(held) = self.threads.iter_mut().find(|held| held.root_id == root_id) {
                held.last_read = held.last_read.max(body.position);
                if held.last_read >= held.latest {
                    held.unread = 0;
                    held.mentions = 0;
                }
            }
        }
        match self.threads.iter().find(|held| held.root_id == root_id) {
            Some(held) => json(200, held),
            // The account does not follow the thread (ADR 0409).
            None => not_found(),
        }
    }

    fn thread(
        &mut self,
        root_id: MessageId,
        request: &HttpRequest,
    ) -> Result<HttpResponse, TransportError> {
        let Some(mut root) = self
            .messages
            .iter()
            .find(|held| held.id == root_id)
            .cloned()
        else {
            return not_found();
        };
        let mut replies: Vec<Message> = self
            .replies
            .iter()
            .filter(|held| held.thread_root == Some(root_id))
            .cloned()
            .collect();
        if request.method == Method::Get {
            root.reply_count = replies.len() as u64;
            root.last_reply = replies.last().map(|held| held.id);
            return json(
                200,
                &ThreadPage {
                    root,
                    replies: Self::page_of(&replies, request),
                    forks: self.forks.clone(),
                },
            );
        }
        let body: SendMessage =
            serde_json::from_slice(request.body.as_ref().unwrap().as_slice()).unwrap();
        let head = self
            .messages
            .iter()
            .chain(&self.replies)
            .map(|held| held.position)
            .max()
            .unwrap_or(0);
        let mut sent = message(root.place, head + 1, self.own.unwrap(), "");
        sent.document = Some(body.document);
        sent.reply_to = body.reply_to;
        sent.thread_root = Some(root_id);
        self.posted
            .push(serde_json::to_string(&sent.document).unwrap());
        replies.push(sent.clone());
        self.replies.push(sent.clone());
        json(201, &sent)
    }
}

pub(super) struct Room {
    pub(super) rig: Rig,
    pub(super) peer: Peer,
    pub(super) place: PlaceId,
    pub(super) other: AccountId,
}

impl Room {
    pub(super) fn chat<R>(&self, act: impl FnOnce(&mut Chat) -> R) -> R {
        act(&mut self.rig.http.routes.lock().unwrap().chat)
    }

    pub(super) fn held(&self, position: u64, text: &str) -> Message {
        message(self.rig.scene.text_place(), position, self.other, text)
    }

    pub(super) async fn changed(&self, id: u64, held: &Message, deleted: bool) {
        self.peer
            .event(
                id,
                Event::MessageChanged {
                    place: held.place,
                    message_id: held.id,
                    position: held.position,
                    deleted,
                    thread_root: None,
                    thread: None,
                },
            )
            .await;
    }

    /// How many requests the client has sent to paths that start with a text.
    pub(super) fn sent_to(&self, start: &str) -> usize {
        let routes = self.rig.http.routes.lock().unwrap();
        routes
            .seen
            .iter()
            .filter(|(path, _)| path.starts_with(start))
            .count()
    }

    pub(super) async fn until_sent_to(&self, start: &str, count: usize) {
        soon("the requests", async {
            while self.sent_to(start) < count {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }
}

pub(super) fn positions(model: &ServerModel, place: PlaceId) -> Vec<u64> {
    model
        .timeline(place)
        .map(|timeline| timeline.messages.iter().map(|held| held.position).collect())
        .unwrap_or_default()
}

/// A served connection with three messages in the text channel and its timeline open.
pub(super) async fn room() -> Room {
    room_at(junto_protocol::events::EVENT_REVISION).await
}

/// A room on a server that states this event revision in its `hello`.
pub(super) async fn room_at(revision: u32) -> Room {
    let mut rig = rig();
    let place = PlaceId::Channel(rig.scene.general);
    let other = AccountId::generate();
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = (1..=3)
            .map(|position| message(rig.scene.text_place(), position, other, "text"))
            .collect();
    }
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served_at(revision).await;
    soon("the first page", rig.live.open_messages(place))
        .await
        .unwrap();
    let model = rig.model();
    assert_eq!(positions(&model, place), [1, 2, 3]);
    rig.model_when("the limits", |model| model.attachment_limits.is_some())
        .await;
    Room {
        rig,
        peer,
        place,
        other,
    }
}

#[tokio::test(start_paused = true)]
async fn a_changed_message_is_read_and_a_change_during_the_read_reads_it_again() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let fourth = room.held(4, "four");
    room.chat(|chat| chat.messages.push(fourth.clone()));
    let single = format!("/api/v1/messages/{}", fourth.id);

    // The event carries no content: the client reads the message and the summary.
    let gate = rig.http.hold("/api/v1/messages/");
    room.changed(41, &fourth, false).await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    room.until_sent_to(&single, 1).await;
    // The message changes again while its read is on its way: no second read starts.
    let mut edited = fourth.clone();
    edited.document = Some(document("four, edited"));
    room.chat(|chat| chat.messages[3] = edited.clone());
    room.changed(42, &fourth, false).await;
    rig.model_when("the second event", |model| model.cursor.position == 42)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(room.sent_to(&single), 1);
    assert_eq!(positions(&rig.model(), place), [1, 2, 3]);
    // After the answer the message is read again, because an event named it meanwhile.
    gate.add_permits(64);
    room.until_sent_to(&single, 2).await;
    let model = rig
        .model_when("the message as it is now", |model| {
            model
                .timeline(place)
                .and_then(|timeline| timeline.message(fourth.id))
                .is_some_and(|held| held.document == edited.document)
        })
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3, 4]);
    rig.model_when("the summary", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.latest == 4 && state.unread == 4)
    })
    .await;

    // A deleted message that the server no longer gives leaves the timeline.
    let second = rig.model().timeline(place).unwrap().messages[1].clone();
    room.chat(|chat| chat.messages.remove(1));
    room.changed(43, &second, true).await;
    let model = rig
        .model_when("the deletion", |model| positions(model, place) == [1, 3, 4])
        .await;
    assert!(!model.timeline(place).unwrap().stale);

    // A message of a place that no reader holds is not read.
    rig.live.release_messages(place);
    rig.model_when("released", |model| model.timeline(place).is_none())
        .await;
    let fifth = room.held(5, "five");
    room.chat(|chat| chat.messages.push(fifth.clone()));
    room.changed(44, &fifth, false).await;
    rig.model_when("the summary of the closed place", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.latest == 5)
    })
    .await;
    assert_eq!(room.sent_to(&format!("/api/v1/messages/{}", fifth.id)), 0);
}

#[tokio::test(start_paused = true)]
async fn the_reads_of_single_messages_are_bounded() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let new: Vec<Message> = (4..=15)
        .map(|position| room.held(position, "new"))
        .collect();
    room.chat(|chat| chat.messages.extend(new.clone()));
    let gate = rig.http.hold("/api/v1/messages/");
    for (index, held) in new.iter().enumerate() {
        room.changed(41 + index as u64, held, false).await;
    }
    rig.model_when("the events", |model| model.cursor.position == 52)
        .await;
    room.until_sent_to("/api/v1/messages/", 4).await;
    // The other eight wait until one of the four has its answer.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(room.sent_to("/api/v1/messages/"), 4);
    gate.add_permits(2);
    room.until_sent_to("/api/v1/messages/", 6).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(room.sent_to("/api/v1/messages/"), 6);
    gate.add_permits(64);
    let model = rig
        .model_when("all messages", |model| positions(model, place).len() == 15)
        .await;
    assert_eq!(positions(&model, place), (1..=15).collect::<Vec<u64>>());
    assert_eq!(room.sent_to("/api/v1/messages/"), 12);
}

#[tokio::test(start_paused = true)]
async fn http_message_bodies_count_toward_the_retained_budget() {
    use crate::model::message_tests::heavy;
    use crate::model::{MOST_LOADED_WEIGHT, MOST_MESSAGE_WEIGHT, ReadFailure, weight};

    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    // Small events name new messages. The answer to the read of each one is a message
    // close to the most that one message weighs, far below the bound of one answer.
    let each = MOST_MESSAGE_WEIGHT - 4096;
    let count = (MOST_LOADED_WEIGHT / each) as u64 + 8;
    for index in 0..count {
        let held = heavy(rig.scene.text_place(), 4 + index, each);
        room.chat(|chat| chat.messages.push(held.clone()));
        room.changed(41 + index, &held, false).await;
    }
    let last = room.held(4 + count, "an ordinary message");
    room.chat(|chat| chat.messages.push(last.clone()));
    room.changed(41 + count, &last, false).await;
    let model = rig
        .model_when("the last message", |model| {
            positions(model, place).last() == Some(&last.position)
                && positions(model, place).len() as u64 > count - 9
        })
        .await;
    // The timeline holds the weight that it may hold, and the oldest messages gave way:
    // what is held is the end of the history without a hole.
    let timeline = model.timeline(place).unwrap();
    let held: usize = timeline.messages.iter().map(|held| weight(held)).sum();
    assert!(held <= MOST_LOADED_WEIGHT, "{held}");
    assert!(timeline.more_before && !timeline.more_after);
    let shown = positions(&model, place);
    let first = last.position + 1 - shown.len() as u64;
    assert_eq!(shown, (first..=last.position).collect::<Vec<u64>>());
    assert!(first > 4, "the messages before the heavy ones gave way");

    // A message above the bound of one message is no answer that the client reads: it is
    // not held, the read is not sent again, and the timeline says that it is not current.
    let above = heavy(rig.scene.text_place(), last.position + 1, 3 * 1024 * 1024);
    room.chat(|chat| chat.messages.push(above.clone()));
    let asked = room.sent_to(&format!("/api/v1/messages/{}", above.id));
    room.changed(42 + count, &above, false).await;
    let model = rig
        .model_when("the failed read", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| timeline.failed == Some(ReadFailure::NoAnswer))
        })
        .await;
    assert!(model.timeline(place).unwrap().message(above.id).is_none());
    // A read that got no answer is sent again after 2 seconds.
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(
        room.sent_to(&format!("/api/v1/messages/{}", above.id)),
        asked + 1
    );

    // The messages after it are read as before.
    let next = room.held(last.position + 2, "the next message");
    room.chat(|chat| chat.messages.push(next.clone()));
    room.changed(43 + count, &next, false).await;
    rig.model_when("the next message", |model| {
        model
            .timeline(place)
            .is_some_and(|timeline| timeline.message(next.id).is_some())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn permission_invalidation_hides_content_before_a_snapshot_can_succeed() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let workspace_id = rig.scene.workspace_id;
    rig.http.routes.lock().unwrap().members = vec![member("Ana")];
    soon("the members", rig.live.load_members(workspace_id))
        .await
        .unwrap();
    let pins = soon("the pins", rig.live.open_pins(rig.scene.general));
    pins.await.unwrap();
    // A page is on its way when the change arrives, and the snapshots that follow fail.
    let gate = rig.http.hold("/messages");
    let before = rig.http.count("/messages");
    rig.live.read_latest_messages(place);
    soon("the page request", async {
        while rig.http.count("/messages") == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    for _ in 0..3 {
        rig.http.fail("/snapshot", 500, "internal");
    }
    room.peer
        .event(
            41,
            Event::PermissionsChanged {
                workspace_id: Some(workspace_id),
            },
        )
        .await;
    // Nothing of the workspace that the account may have lost is held any more.
    let model = rig
        .model_when("the change", |model| model.cursor.position == 41)
        .await;
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.messages.is_empty() && !timeline.loaded && timeline.stale);
    assert_eq!(
        model.workspace(workspace_id).unwrap().members,
        Loadable::Loading
    );
    let pinned = model.pin_list(rig.scene.general).unwrap();
    assert_eq!(pinned.messages, Loadable::Loading);
    // The answer to the page that was asked for before the change is not taken, while
    // the snapshots fail.
    gate.add_permits(64);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let model = rig.model();
    assert!(model.timeline(place).unwrap().messages.is_empty());
    assert!(rig.http.count("/snapshot") >= 2);
    // A snapshot that succeeds reads the place again.
    let model = rig
        .model_when("the place read again", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| timeline.loaded && !timeline.stale)
        })
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3]);
}

#[tokio::test(start_paused = true)]
async fn a_bound_keeps_the_start_of_a_timeline_that_the_reader_shows() {
    use crate::model::message_tests::heavy;
    use crate::model::{MOST_LOADED_WEIGHT, MOST_MESSAGE_WEIGHT, weight};

    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    // The reader shows the first message of the place.
    rig.live.set_message_anchor(place, Some(1));
    rig.model_when("the anchor", |model| {
        model
            .timeline(place)
            .is_some_and(|timeline| timeline.anchor == Some(1))
    })
    .await;
    let each = MOST_MESSAGE_WEIGHT - 4096;
    let count = (MOST_LOADED_WEIGHT / each) as u64 + 8;
    let mut heavies = Vec::new();
    for index in 0..count {
        let held = heavy(rig.scene.text_place(), 4 + index, each);
        room.chat(|chat| chat.messages.push(held.clone()));
        room.changed(41 + index, &held, false).await;
        heavies.push(held);
    }
    rig.model_when("the last event", |model| {
        model.cursor.position == 40 + count
            && model
                .timeline(place)
                .is_some_and(|timeline| timeline.more_after)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let model = rig.model();
    let timeline = model.timeline(place).unwrap();
    let held: usize = timeline.messages.iter().map(|held| weight(held)).sum();
    assert!(held <= MOST_LOADED_WEIGHT, "{held}");
    // The messages that the reader shows stay; the end gave way.
    assert_eq!(positions(&model, place)[..3], [1, 2, 3]);
    assert!(timeline.more_after && !timeline.more_before);
    let last = heavies.last().unwrap();
    assert!(timeline.message(last.id).is_none());
}

#[tokio::test(start_paused = true)]
async fn the_read_position_is_sent_one_request_at_a_time_with_the_newest_position() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let gate = rig.http.hold("/messages/read");
    rig.live.mark_read(place, 1);
    soon("the first request", async {
        while rig.http.count("/messages/read") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Two further positions arrive while the first request is on its way.
    rig.live.mark_read(place, 2);
    rig.live.mark_read(place, 3);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(rig.http.count("/messages/read"), 1);
    gate.add_permits(64);
    rig.model_when("the newest position", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.last_read == 3 && state.unread == 0)
    })
    .await;
    assert_eq!(room.chat(|chat| chat.marked.clone()), [1, 3]);
    // A position that is read already is not sent.
    rig.live.mark_read(place, 2);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(room.chat(|chat| chat.marked.clone()), [1, 3]);
}

#[tokio::test(start_paused = true)]
async fn a_reload_keeps_the_open_timeline_until_its_page_has_arrived() {
    let mut room = room().await;
    let place = room.place;
    // The reader stands at the second message.
    room.rig.live.set_message_anchor(place, Some(2));
    room.chat(|chat| {
        chat.messages.remove(0);
        let mut edited = chat.messages[0].clone();
        edited.document = Some(document("two, edited"));
        chat.messages[0] = edited;
    });
    let fourth = room.held(4, "four");
    room.chat(|chat| chat.messages.push(fourth));
    room.rig.set_snapshot(90, |_| {});
    let gate = room.rig.http.hold("/messages");
    room.peer
        .send(&ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind,
        })
        .await;
    // The messages of before stay, marked, while the page is on its way.
    let model = room
        .rig
        .model_when("the snapshot", |model| {
            model.cursor.position == 90 && model.timeline(place).is_some()
        })
        .await;
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.stale && timeline.loaded);
    assert_eq!(positions(&model, place), [1, 2, 3]);
    gate.add_permits(64);
    let model = room
        .rig
        .model_when("the page from the anchor", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| !timeline.stale)
        })
        .await;
    let timeline = model.timeline(place).unwrap();
    assert_eq!(positions(&model, place), [2, 3, 4]);
    assert_eq!(timeline.messages[0].document, Some(document("two, edited")));
    assert!(timeline.more_before && !timeline.more_after);
    room.peer.serve().await;
}

#[tokio::test(start_paused = true)]
async fn messages_are_sent_in_their_order_and_a_lost_answer_is_resolved_by_the_history() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let history = format!("/api/v1/channels/{}/messages", rig.scene.general);
    let requests = |room: &Room| {
        let routes = room.rig.http.routes.lock().unwrap();
        routes
            .seen
            .iter()
            .filter(|(path, _)| *path == history)
            .count()
    };
    // Three messages of one place: one request at a time, in the order of the person.
    let gate = rig.http.hold("/messages");
    for text in ["first", "second", "third"] {
        rig.live.send_message(place, document(text), None).unwrap();
    }
    let model = rig
        .model_when("the messages wait", |model| model.outbox.len() == 3)
        .await;
    assert!(
        model
            .outbox
            .iter()
            .all(|pending| pending.state == SendState::Sending)
    );
    soon("the first send", async {
        while requests(&room) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    // The first page was one request, and one send is on its way.
    assert_eq!(requests(&room), 2);
    gate.add_permits(64);
    let model = rig
        .model_when("all are sent", |model| model.outbox.is_empty())
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3, 4, 5, 6]);
    let expected: Vec<String> = ["first", "second", "third"]
        .iter()
        .map(|text| serde_json::to_string(&Some(document(text))).unwrap())
        .collect();
    assert_eq!(room.chat(|chat| chat.posted.clone()), expected);

    // The server stores a message and its answer is lost: the message is not confirmed,
    // and it is not sent again by itself.
    let unconfirmed = |id: u64| {
        move |model: &ServerModel| {
            model
                .outbox
                .iter()
                .any(|pending| pending.id == id && pending.state == SendState::Unconfirmed)
        }
    };
    room.chat(|chat| chat.lose_answers = 1);
    let id = rig
        .live
        .send_message(place, document("lost"), None)
        .unwrap();
    rig.model_when("no confirmation", unconfirmed(id)).await;
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(room.chat(|chat| chat.posted.len()), 4);
    // "Send again" reads the history, finds the message, and sends nothing.
    rig.live.send_again(id);
    let model = rig
        .model_when("found in the history", |model| model.outbox.is_empty())
        .await;
    assert_eq!(room.chat(|chat| chat.posted.len()), 4);
    assert_eq!(positions(&model, place), [1, 2, 3, 4, 5, 6, 7]);

    // A message without an answer that the history does not hold is sent again, and not
    // before the time in which the first request can still take effect.
    room.chat(|chat| chat.lose_answers = 1);
    let id = rig
        .live
        .send_message(place, document("lost twice"), None)
        .unwrap();
    rig.model_when("no confirmation", unconfirmed(id)).await;
    room.chat(|chat| {
        chat.messages.pop();
    });
    let started = Instant::now();
    rig.live.send_again(id);
    rig.model_when("sent again", |model| model.outbox.is_empty())
        .await;
    let waited = started.elapsed();
    let least = StreamPolicy::default().send_again_after - Duration::from_secs(1);
    assert!(
        waited >= least,
        "the message was sent again after {waited:?}"
    );
    assert_eq!(room.chat(|chat| chat.posted.len()), 6);
    assert_eq!(room.chat(|chat| chat.messages.len()), 8);

    // A discarded message leaves the state.
    room.chat(|chat| chat.lose_answers = 1);
    let id = rig
        .live
        .send_message(place, document("discarded"), None)
        .unwrap();
    rig.model_when("no confirmation", unconfirmed(id)).await;
    rig.live.discard_message(id);
    rig.model_when("nothing waits", |model| model.outbox.is_empty())
        .await;
}

#[tokio::test(start_paused = true)]
async fn the_answer_to_a_send_is_put_into_the_timeline_only_before_its_event() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let epoch = rig.epoch();
    let receipt =
        |position| crate::api::Receipt::At(junto_protocol::events::Cursor { epoch, position });

    // The server stores the message and tells its event and then its deletion before
    // the answer arrives. The answer states the position of the event of the message.
    let gate = rig.http.hold_answers("/messages");
    room.chat(|chat| chat.send_receipt = receipt(41));
    let id = rig
        .live
        .send_message(place, document("deleted at once"), None)
        .unwrap();
    rig.model_when("the message waits", |model| {
        model.outbox.iter().any(|pending| pending.id == id)
    })
    .await;
    room.until_sent_to(
        &format!("/api/v1/channels/{}/messages", rig.scene.general),
        2,
    )
    .await;
    let stored = room.chat(|chat| chat.messages.pop().unwrap());
    assert_eq!(stored.position, 4);
    room.changed(41, &stored, false).await;
    room.changed(42, &stored, true).await;
    rig.model_when("both events", |model| model.cursor.position == 42)
        .await;
    gate.add_permits(1);
    let model = rig
        .model_when("the answer", |model| model.outbox.is_empty())
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3]);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(positions(&rig.model(), place), [1, 2, 3]);

    // An answer that arrives before the stream reached the event of its message puts the
    // message into the timeline; the events that follow are applied after it.
    room.chat(|chat| chat.send_receipt = receipt(43));
    let id = rig
        .live
        .send_message(place, document("kept"), None)
        .unwrap();
    rig.model_when("the message waits", |model| {
        model.outbox.iter().any(|pending| pending.id == id)
    })
    .await;
    gate.add_permits(1);
    let model = rig
        .model_when("the answer", |model| model.outbox.is_empty())
        .await;
    assert_eq!(model.cursor.position, 42);
    assert_eq!(positions(&model, place), [1, 2, 3, 4]);
}

fn unconfirmed(id: u64) -> impl Fn(&ServerModel) -> bool {
    move |model: &ServerModel| {
        model
            .outbox
            .iter()
            .any(|pending| pending.id == id && pending.state == SendState::Unconfirmed)
    }
}

#[tokio::test(start_paused = true)]
async fn a_message_that_a_search_finds_is_not_put_back_after_a_later_event() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let history = format!("/api/v1/channels/{}/messages", rig.scene.general);

    // The server stores a message and its answer is lost.
    room.chat(|chat| chat.lose_answers = 1);
    let id = rig
        .live
        .send_message(place, document("deleted during the search"), None)
        .unwrap();
    rig.model_when("no confirmation", unconfirmed(id)).await;
    let stored = room.chat(|chat| chat.messages.last().cloned().unwrap());
    assert_eq!(stored.position, 4);

    // "Send again" searches the history. The answer holds the message, and the message is
    // deleted before the answer arrives.
    let before = room.sent_to(&history);
    let gate = rig.http.hold_answers("/messages");
    rig.live.send_again(id);
    // The search starts after the time in which the first request can still take effect.
    for _ in 0..1000 {
        if room.sent_to(&history) > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(room.sent_to(&history), before + 1);
    room.chat(|chat| {
        chat.messages.pop();
    });
    room.changed(41, &stored, true).await;
    rig.model_when("the deletion", |model| model.cursor.position == 41)
        .await;
    gate.add_permits(1);
    let model = rig
        .model_when("the answer", |model| model.outbox.is_empty())
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3]);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(positions(&rig.model(), place), [1, 2, 3]);
    assert_eq!(room.chat(|chat| chat.posted.len()), 1);
}

#[tokio::test(start_paused = true)]
async fn a_search_answer_from_before_a_reload_is_not_used() {
    let mut room = room().await;
    let place = room.place;
    let history = format!("/api/v1/channels/{}/messages", room.rig.scene.general);

    // The server stores a message and its answer is lost.
    room.chat(|chat| chat.lose_answers = 1);
    let id = room
        .rig
        .live
        .send_message(place, document("deleted during a reload"), None)
        .unwrap();
    room.rig
        .model_when("no confirmation", unconfirmed(id))
        .await;
    let stored = room.chat(|chat| chat.messages.last().cloned().unwrap());

    // "Send again" searches the history, and the answer holds the message.
    let before = room.sent_to(&history);
    let gate = room.rig.http.hold_answers("/messages");
    room.rig.live.send_again(id);
    for _ in 0..1000 {
        if room.sent_to(&history) > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(room.sent_to(&history), before + 1);
    // Another device deletes the message while the events are missed, and the state is
    // replaced by a snapshot before the answer arrives.
    room.chat(|chat| {
        chat.messages.pop();
    });
    room.rig.set_snapshot(90, |_| {});
    room.peer
        .send(&ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind,
        })
        .await;
    room.peer.serve().await;
    room.rig
        .model_when("the snapshot", |model| model.cursor.position == 90)
        .await;
    // The answer from before the reload is not used: the history is searched again, does
    // not hold the message, and the message is sent.
    gate.add_permits(64);
    let model = room
        .rig
        .model_when("the message is sent", |model| model.outbox.is_empty())
        .await;
    assert_eq!(room.chat(|chat| chat.posted.len()), 2);
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.messages.iter().all(|held| held.id != stored.id));
}

#[tokio::test(start_paused = true)]
async fn a_search_that_does_not_reach_the_end_of_the_history_sends_nothing() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let history = format!("/api/v1/channels/{}/messages", rig.scene.general);
    let requests = || {
        let routes = rig.http.routes.lock().unwrap();
        routes
            .seen
            .iter()
            .filter(|(path, _)| *path == history)
            .count()
    };
    // The server holds 1,100 messages that the client was not told of. The position
    // after which the client searches lies that far before the message.
    let many: Vec<Message> = (4..=1103)
        .map(|position| room.held(position, "other"))
        .collect();
    room.chat(|chat| {
        chat.messages.extend(many);
        chat.lose_answers = 1;
    });
    let id = rig
        .live
        .send_message(place, document("lost"), None)
        .unwrap();
    rig.model_when("no confirmation", unconfirmed(id)).await;
    let before = requests();
    rig.live.send_again(id);
    tokio::time::sleep(Duration::from_secs(60)).await;
    // The search ended before the end of the history: the server may hold the message.
    assert_eq!(room.chat(|chat| chat.posted.len()), 1);
    assert_eq!(requests(), before + 5);
    assert!(unconfirmed(id)(&rig.model()));
    // The next search goes on where this one ended, and finds the message.
    rig.live.send_again(id);
    rig.model_when("found in the history", |model| model.outbox.is_empty())
        .await;
    assert_eq!(room.chat(|chat| chat.posted.len()), 1);
}

#[tokio::test(start_paused = true)]
async fn the_answer_to_a_read_position_does_not_replace_a_newer_summary() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let gate = rig.http.hold("PUT /messages/read");
    let before = rig.http.count("/messages/read");
    rig.live.mark_read(place, 3);
    soon("the request", async {
        while rig.http.count("/messages/read") == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // The server made its answer before a fourth message arrived, and the answer is late.
    let fourth = room.held(4, "four");
    let old = MessageState {
        place: fourth.place,
        last_read: 3,
        latest: 3,
        unread: 0,
        mentions: 0,
        unread_sign: false,
        notification_mentions: 0,
        muted: false,
    };
    room.chat(|chat| {
        chat.mark_answer = Some(old);
        chat.messages.push(fourth.clone());
    });
    room.changed(41, &fourth, false).await;
    rig.model_when("the summary of the event", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.latest == 4)
    })
    .await;
    gate.add_permits(64);
    rig.model_when("the summary after the late answer", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.last_read == 3 && state.latest == 4 && state.unread == 1)
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_summary_is_read_until_the_server_answers() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let fourth = room.held(4, "four");
    room.chat(|chat| chat.messages.push(fourth.clone()));
    // No event tells the count a second time, and no reader can ask for it.
    for _ in 0..40 {
        rig.http.fail("/messages/read", 503, "unavailable");
    }
    room.changed(41, &fourth, false).await;
    let model = rig
        .model_when("the summary", |model| {
            model
                .message_state(place)
                .is_some_and(|state| state.latest == 4)
        })
        .await;
    assert!(!model.timeline(place).unwrap().stale);
    assert!(rig.http.routes.lock().unwrap().failures.is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_answer_to_a_read_position_does_not_replace_what_an_event_told_meanwhile() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let gate = rig.http.hold("PUT /messages/read");
    let before = rig.http.count("/messages/read");
    rig.live.mark_read(place, 2);
    soon("the request", async {
        while rig.http.count("/messages/read") == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Another device of the account reads to the latest message. The event moves the
    // read position without a read of the summary. The answer to the request of this
    // device was made before that.
    let first = room.held(1, "");
    let old = MessageState {
        place: first.place,
        last_read: 2,
        latest: 3,
        unread: 1,
        mentions: 0,
        unread_sign: true,
        notification_mentions: 0,
        muted: false,
    };
    room.chat(|chat| {
        chat.last_read = 3;
        chat.mark_answer = Some(old);
    });
    room.peer
        .event(
            41,
            Event::MessageRead {
                place: first.place,
                position: 3,
            },
        )
        .await;
    rig.model_when("the event", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.last_read == 3 && state.unread == 0)
    })
    .await;
    gate.add_permits(64);
    soon("the request is answered", async {
        while room.chat(|chat| chat.marked.is_empty()) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let state = rig.model().message_state(place).cloned().unwrap();
    assert_eq!((state.last_read, state.unread), (3, 0));
}

#[tokio::test(start_paused = true)]
async fn a_read_position_of_a_new_message_moves_the_latest_position_before_the_summary_answers() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let third = room.held(3, "text");
    room.changed(41, &third, false).await;
    rig.model_when("the summary", |model| {
        model
            .message_state(place)
            .is_some_and(|state| state.latest == 3 && state.unread == 3)
    })
    .await;
    // Another device of the account sends a fourth message. The server moves the read
    // position of the account to it in the same transaction, and tells the change of the
    // message before the read position. The summary that the change asks for is late.
    let gate = rig.http.hold("/messages/read");
    let before = rig.http.count("/messages/read");
    let mine = message(rig.scene.text_place(), 4, rig.scene.account_id, "mine");
    room.chat(|chat| {
        chat.messages.push(mine.clone());
        chat.last_read = 4;
    });
    room.changed(42, &mine, false).await;
    soon("the read of the summary", async {
        while rig.http.count("/messages/read") == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    room.peer
        .event(
            43,
            Event::MessageRead {
                place: mine.place,
                position: 4,
            },
        )
        .await;
    let model = rig
        .model_when("the read position", |model| {
            model
                .message_state(place)
                .is_some_and(|state| state.last_read == 4)
        })
        .await;
    let expected = room.chat(|chat| chat.state()).unwrap();
    assert_eq!(model.message_state(place), Some(&expected));
    gate.add_permits(64);
    rig.model_when("the summary of the server", |model| {
        model.message_state(place) == Some(&expected)
    })
    .await;
}
