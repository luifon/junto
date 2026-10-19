//! The screens of the messages (ADR 0740): the list of a text channel with every kind of
//! row, the states of the composer, the chat of a voice channel, and the chat of a call.

use std::collections::HashMap;
use std::sync::Arc;

use junto_client::compose::{self, Mention, MentionTarget};
use junto_client::shell::call::CallTab;
use junto_client::shell::{EmojiTarget, Open, Pane, Shell};
use junto_client::state::feed::Zone;
use junto_client::state::view::{DraftFile, ServerSnapshot};
use junto_client_core::model::{
    PendingFile, PendingMessage, PlaceId, ReadFailure, Reading, SendFailure, SendState,
    ServerModel, Timeline,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{AttachmentId, ChannelId, MessageId, ServerId};
use junto_client_core::protocol::message::{
    Attachment, Document, Message, MessagePlace, MessageState, Reaction, SystemMessage,
};
use junto_client_core::transfers::{
    DownloadProgress, DownloadStage, TransferFailure, UploadProgress, UploadStage,
};
use time::{Duration, OffsetDateTime, UtcOffset};

use super::{
    Phase, Screen, Studio, acme, acme_with_luca, harbor, id, in_call, person, select_channel,
};

pub(crate) const GENERAL: u32 = 30;
const DAILY: u32 = 34;

/// Thursday, 8 October 2026 at 09:00 in the time zone of the screens.
pub(crate) fn morning() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_450_000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

pub(crate) fn place(channel: u32) -> MessagePlace {
    MessagePlace::Channel {
        workspace_id: id(10),
        channel_id: id(channel),
    }
}

/// The document of a text of the composer. Each `@Name` of a person of the design is a
/// mention.
pub(crate) fn document(text: &str) -> Document {
    let mut mentions = Vec::new();
    for name in super::PEOPLE {
        let sign = format!("@{name}");
        for (start, _) in text.match_indices(&sign) {
            mentions.push(Mention {
                range: start..start + sign.len(),
                target: MentionTarget::Account(person(name)),
            });
        }
    }
    compose::parse(text, &mentions)
}

pub(crate) struct Made {
    pub(crate) channel: u32,
    pub(crate) position: u64,
    pub(crate) messages: Vec<Arc<Message>>,
}

impl Made {
    pub(crate) fn new(channel: u32) -> Self {
        Self {
            channel,
            position: 0,
            messages: Vec::new(),
        }
    }

    pub(crate) fn message(&mut self, author: &str, minutes: i64, text: &str) -> &mut Message {
        self.position += 1;
        self.messages.push(Arc::new(Message {
            id: id(5000 + self.channel * 100 + self.position as u32),
            place: place(self.channel),
            position: self.position,
            author_id: Some(person(author)),
            author_name: Some(author.to_owned()),
            created_at: morning() + Duration::minutes(minutes),
            edited_at: None,
            document: Some(document(text)),
            system: None,
            reply_to: None,
            permanent: false,
            deleted: false,
            attachments: Vec::new(),
            reactions: Vec::new(),
            thread_root: None,
            reply_count: 0,
            last_reply: None,
            reply_authors: Vec::new(),
            pinned: false,
            files_removed_by_retention: false,
        }));
        let last = self.messages.len() - 1;
        Arc::make_mut(&mut self.messages[last])
    }

    pub(crate) fn system(
        &mut self,
        minutes: i64,
        action: &str,
        actor: Option<&str>,
        subject: Option<&str>,
    ) {
        let message = self.message("Luca", minutes, "");
        message.author_id = None;
        message.author_name = None;
        message.document = None;
        message.system = Some(SystemMessage {
            action: action.to_owned(),
            subject_id: subject.map(person),
            actor_id: actor.map(person),
            recording_id: None,
            duration_seconds: None,
        });
    }

    pub(crate) fn timeline(self) -> Timeline {
        Timeline {
            place: PlaceId::Channel(id(self.channel)),
            messages: self.messages,
            more_before: false,
            more_after: false,
            reading: Reading::default(),
            loaded: true,
            failed: None,
            stale: false,
            unread_after: None,
            root: None,
            forks: Vec::new(),
            anchor: None,
        }
    }
}

pub(crate) fn reaction(emoji: &str, count: u32, own: bool) -> Reaction {
    Reaction {
        emoji: emoji.to_owned(),
        count,
        own,
    }
}

fn file(number: u32, name: &str, bytes: u64, picture: Option<(u32, u32)>) -> Attachment {
    Attachment {
        id: id(7000 + number),
        name: name.to_owned(),
        bytes,
        sha256: String::new(),
        media_type: if picture.is_some() {
            "image/png"
        } else {
            "application/octet-stream"
        }
        .to_owned(),
        width: picture.map(|size| size.0),
        height: picture.map(|size| size.1),
    }
}

/// The messages of `#general` of the design (view A).
pub(crate) fn general() -> Made {
    let mut made = Made::new(GENERAL);
    made.system(-30, "channel_created", Some("Ana"), None);
    let ana = made.message(
        "Ana",
        2,
        "The **v0.9** checklist is up. *Please* tick your items. Build: `cargo build --release`",
    );
    ana.reactions = vec![
        reaction("\u{1f44d}", 4, true),
        reaction("\u{2705}", 2, false),
    ];
    let bruno = made.message(
        "Bruno",
        10,
        "The export worker ran out of memory on large rooms. Now it drops muted tracks:\n\n\
         ```Rust\nfn mix_tracks(tracks: &[Track]) -> Mixdown {\n    \
         tracks.iter().filter(|t| !t.muted)   // skip silent tracks\n        \
         .map(|t| t.resample(48_000)).collect()\n}\n```",
    );
    let bruno_id = bruno.id;
    let carla = made.message(
        "Carla",
        14,
        "Ran it on the 40-person room: the mixdown went from 96 s to 11 s. Timeline below.",
    );
    carla.reply_to = Some(bruno_id);
    carla.edited_at = Some(morning() + Duration::minutes(15));
    carla.attachments = vec![file(1, "mixdown-timeline.png", 1_200_000, Some((584, 144)))];
    carla.reactions = vec![
        reaction("\u{1f389}", 3, false),
        reaction("\u{1f440}", 2, false),
    ];
    made.message(
        "Diego",
        31,
        "Should recordings keep one track per speaker, or only the mixdown? Storage is the \
         tradeoff.",
    );
    let luca = made.message(
        "Luca",
        48,
        "@Bruno can you review the retention settings before lunch? The spec is attached.",
    );
    luca.attachments = vec![file(2, "recording-retention-spec.pdf", 284_000, None)];
    made.message(
        "Luca",
        49,
        "See also https://example.test/specs/retention for the draft.",
    );
    let bruno = made.message(
        "Bruno",
        91,
        "Reviewed, @Luca. The default is **30 days**, with a per-channel override. I merge \
         after lunch.",
    );
    bruno.edited_at = Some(morning() + Duration::minutes(93));
    made
}

/// Messages with every kind of block, a deleted message and a reply to it.
fn blocks() -> Made {
    let mut made = Made::new(GENERAL);
    let first = made.message(
        "Ana",
        2,
        "Action items for the release:\n\n- [x] Bruno merges the export fix\n- [ ] Diego \
         writes the retention spec\n- [ ] Luca sends Marta the demo invite",
    );
    first.reactions = vec![reaction("\u{2705}", 3, true)];
    let deleted = made.message("Bruno", 5, "gone");
    deleted.deleted = true;
    deleted.document = None;
    deleted.author_id = None;
    deleted.author_name = None;
    let deleted_id = deleted.id;
    let reply = made.message("Carla", 6, "I had the same question.");
    reply.reply_to = Some(deleted_id);
    made.message(
        "Diego",
        20,
        "The order of the work:\n\n1. Read the tracks\n2. Resample to 48 kHz\n3. Write the \
         mixdown\n\n> Storage is the tradeoff.\n\nAnd ~~not~~ with *both* kinds of \
         **emphasis**, a [link](https://example.test/a) and `inline code`.",
    );
    made.message(
        "Luca",
        22,
        "- one item\n- a second item with `code`\n- a third item for @Ana",
    );
    made.message(
        "Luca",
        23,
        "A second message of the same author within five minutes.",
    );
    made.message("Luca", 24, "And a third one.");
    made
}

/// The chat of the voice channel Daily after a call (view G), as far as this version
/// draws it.
fn daily_chat() -> Made {
    let mut made = Made::new(DAILY);
    made.system(38, "call_started", Some("Ana"), None);
    made.system(41, "recording_started", Some("Ana"), None);
    made.system(44, "joined_by_invitation", Some("Marta"), None);
    made.message(
        "Bruno",
        47,
        "Staging is back up. Deploy `v0.9.0-rc2` went out at 09:31.",
    );
    made.system(52, "screen_share_started", Some("Carla"), None);
    made.system(57, "call_microphone_blocked", Some("Ana"), Some("Luca"));
    let notes = made.message("Carla", 65, "Notes from the Alpha review:");
    notes.attachments = vec![file(3, "alpha-review-notes.pdf", 96_000, None)];
    let bruno = made.message(
        "Bruno",
        76,
        "Two more minutes on the screen share, then we close.",
    );
    bruno.edited_at = Some(morning() + Duration::minutes(77));
    made.system(80, "recording_stopped", None, None);
    made.system(81, "call_ended", None, None);
    made.system(84, "recording_ready", None, None);
    made
}

pub(crate) fn state(channel: u32, unread: u64, mentions: u64) -> MessageState {
    MessageState {
        place: place(channel),
        last_read: 0,
        latest: 9,
        unread,
        mentions,
        unread_sign: true,
        notification_mentions: mentions,
        muted: false,
    }
}

/// "Acme Studio" with these messages loaded, and the unread signs of the design: mentions
/// in `client-alpha` and new messages in `deploys`.
pub(crate) fn acme_with(change: impl FnOnce(&mut ServerModel)) -> ServerSnapshot {
    let mut server = acme();
    if let Some(model) = &mut server.model {
        let model = Arc::make_mut(model);
        model.message_states = vec![state(31, 5, 2), state(32, 3, 0)];
        change(model);
        // The change can give the person a role or a permission: the lists that the
        // window reads are stated again (ADR 0310).
        junto_client::state::fixtures::restate(model);
    }
    server
}

fn with_timeline(timeline: Timeline) -> Vec<ServerSnapshot> {
    vec![
        acme_with(|model| model.timelines = vec![Arc::new(timeline)]),
        harbor(),
    ]
}

/// A picture as the server would give it: bars of a timeline on a dark ground.
fn timeline_picture() -> Vec<u8> {
    let (width, height) = (584u32, 144u32);
    let mut picture = image::RgbaImage::from_pixel(width, height, image::Rgba([59, 74, 82, 255]));
    let bars: [(u32, u32, u32, [u8; 3]); 8] = [
        (20, 14, 240, [217, 138, 100]),
        (300, 14, 192, [217, 138, 100]),
        (68, 42, 140, [199, 154, 69]),
        (240, 42, 300, [199, 154, 69]),
        (20, 70, 88, [169, 188, 98]),
        (176, 70, 120, [169, 188, 98]),
        (392, 70, 164, [169, 188, 98]),
        (120, 98, 352, [143, 177, 191]),
    ];
    for (left, top, length, colour) in bars {
        for x in left..(left + length).min(width) {
            for y in top..(top + 18).min(height) {
                picture.put_pixel(x, y, image::Rgba([colour[0], colour[1], colour[2], 255]));
            }
        }
    }
    let mut bytes = Vec::new();
    let encoded = image::DynamicImage::ImageRgba8(picture).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    );
    match encoded {
        Ok(()) => bytes,
        Err(_) => Vec::new(),
    }
}

pub(crate) type Fixture = (
    HashMap<(ServerId, PlaceId), Vec<DraftFile>>,
    HashMap<(ServerId, u64), UploadProgress>,
    HashMap<(ServerId, AttachmentId), DownloadProgress>,
    Option<String>,
);

/// Shows the times in UTC, the pictures of the design, and these states of files.
pub(crate) fn prepare(shell: &mut Shell, fixture: Fixture, cx: &mut gpui::Context<Shell>) {
    shell.show_picture(id(7001), &timeline_picture());
    let (drafts, uploads, downloads, refused) = fixture;
    shell.state().clone().update(cx, |state, cx| {
        state.show_messages_fixture(
            Zone::Fixed(UtcOffset::UTC),
            drafts,
            uploads,
            downloads,
            refused,
            cx,
        );
    });
}

pub(crate) fn none() -> Fixture {
    (HashMap::new(), HashMap::new(), HashMap::new(), None)
}

fn upload(stage: UploadStage, sent: u64, bytes: u64) -> UploadProgress {
    UploadProgress {
        stage,
        sent,
        bytes,
        interrupted: false,
    }
}

fn waiting(
    number: u64,
    text: &str,
    state: SendState,
    files: Vec<PendingFile>,
) -> Arc<PendingMessage> {
    let mut pending = PendingMessage::new(
        number,
        PlaceId::Channel(id(GENERAL)),
        document(text),
        None,
        files,
        morning() + Duration::minutes(95 + number as i64),
    );
    pending.state = state;
    Arc::new(pending)
}

fn general_id() -> ChannelId {
    id(GENERAL)
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    let server_id: ServerId = id(1);
    let general_place = PlaceId::Channel(general_id());

    // View A of the design: a text channel with its messages.
    studio.draw(
        Screen::new("messages-text-channel", with_timeline(general().timeline())),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    studio.draw(
        Screen::new(
            "messages-text-channel-150",
            with_timeline(general().timeline()),
        )
        .scale(150),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    studio.draw(
        Screen::new(
            "messages-text-channel-members",
            with_timeline(general().timeline()),
        ),
        |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.open(Open::Members, window, cx);
        },
    )?;

    // A message whose files the retention of its workspace removed (ADR 0409).
    let mut removed = general();
    removed
        .message("Ana", 100, "The recording of the review is attached.")
        .files_removed_by_retention = true;
    studio.draw(
        Screen::new("messages-files-removed", with_timeline(removed.timeline())),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;

    // Every kind of block, a deleted message, a reply to it, and a group of one author.
    studio.draw(
        Screen::new("messages-blocks", with_timeline(blocks().timeline())),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;

    // The line of the first unread message, and the history that has more before it.
    let mut unread = general().timeline();
    unread.unread_after = Some(4);
    unread.more_before = true;
    studio.draw(
        Screen::new(
            "messages-unread-line",
            vec![
                acme_with(|model| {
                    model.message_states.push(MessageState {
                        place: place(GENERAL),
                        last_read: 4,
                        latest: 8,
                        unread: 3,
                        mentions: 1,
                        unread_sign: true,
                        notification_mentions: 1,
                        muted: false,
                    });
                    model.timelines = vec![Arc::new(unread)];
                }),
                harbor(),
            ],
        ),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;

    // The messages that wait, the files of the next message, and a reply.
    let waits = general();
    let quoted = waits.messages[4].id;
    let waits = waits.timeline();
    let downloads: HashMap<(ServerId, AttachmentId), DownloadProgress> = HashMap::from([
        (
            (server_id, id(7001)),
            DownloadProgress {
                stage: DownloadStage::Receiving,
                received: 420_000,
                bytes: 1_200_000,
                interrupted: false,
            },
        ),
        (
            (server_id, id(7002)),
            DownloadProgress {
                stage: DownloadStage::Saved(std::path::PathBuf::from(
                    "recording-retention-spec.pdf",
                )),
                received: 284_000,
                bytes: 284_000,
                interrupted: false,
            },
        ),
    ]);
    let drafts = HashMap::from([(
        (server_id, general_place),
        vec![
            DraftFile {
                upload: 1,
                name: "export-timings.csv".to_owned(),
                progress: upload(UploadStage::Sending, 2_100_000, 6_400_000),
            },
            DraftFile {
                upload: 2,
                name: "notes.txt".to_owned(),
                progress: upload(UploadStage::Complete(id(7100)), 1_800, 1_800),
            },
            DraftFile {
                upload: 3,
                name: "huge-recording.mov".to_owned(),
                progress: upload(
                    UploadStage::Failed(TransferFailure::Refused {
                        code: ErrorCode::StorageLow,
                        status: 507,
                    }),
                    0,
                    90_000_000,
                ),
            },
        ],
    )]);
    let uploads = HashMap::from([(
        (server_id, 9u64),
        upload(UploadStage::Sending, 800_000, 3_000_000),
    )]);
    studio.draw(
        Screen::new(
            "messages-waiting",
            vec![
                acme_with(|model| {
                    model.timelines = vec![Arc::new(waits)];
                    model.outbox = vec![
                        waiting(1, "This one is on its way.", SendState::Sending, Vec::new()),
                        waiting(
                            2,
                            "This one waits for its file.",
                            SendState::WaitingForFiles,
                            vec![PendingFile {
                                upload: 9,
                                name: "mixdown.flac".to_owned(),
                                bytes: 3_000_000,
                                attachment_id: None,
                            }],
                        ),
                        waiting(
                            3,
                            "The answer to this one was lost.",
                            SendState::Unconfirmed,
                            Vec::new(),
                        ),
                        waiting(
                            4,
                            "The server refused this one.",
                            SendState::Failed(SendFailure::Refused {
                                code: ErrorCode::RateLimited,
                                status: 429,
                            }),
                            Vec::new(),
                        ),
                    ];
                }),
                harbor(),
            ],
        ),
        move |shell, window, cx| {
            prepare(shell, (drafts, uploads, downloads, None), cx);
            shell.reply_to_message(Pane::Main, quoted, window, cx);
            shell.type_message(
                Pane::Main,
                "Yes: **30 days**, then only the mixdown. @",
                window,
                cx,
            );
        },
    )?;

    // A text that the server would refuse: the composer says why, and nothing is sent.
    studio.draw(
        Screen::new("messages-too-long", with_timeline(general().timeline())),
        |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.type_message(Pane::Main, &"word ".repeat(4000), window, cx);
            shell.submit_message(Pane::Main, window, cx);
        },
    )?;

    // The emoji picker, the menu of a message, and the question before a deletion.
    let made = general();
    let (own, other) = (made.messages[5].id, made.messages[1].id);
    studio.draw(
        Screen::new("messages-emoji-picker", with_timeline(general().timeline())),
        move |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.open_emoji_picker(EmojiTarget::React(other), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("messages-menu", with_timeline(general().timeline())),
        move |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.open_message_menu(Pane::Main, own, window, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "messages-confirm-delete",
            with_timeline(general().timeline()),
        ),
        move |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.ask_to_delete_message(own, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("messages-editing", with_timeline(general().timeline())),
        move |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.edit_message(Pane::Main, own, window, cx);
        },
    )?;

    // The states of a list without rows.
    studio.draw(
        Screen::new("messages-loading", vec![acme_with(|_| {}), harbor()]),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    let mut empty = Made::new(GENERAL).timeline();
    studio.draw(
        Screen::new("messages-empty", with_timeline(empty.clone())),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    empty.loaded = false;
    empty.failed = Some(ReadFailure::NoAnswer);
    studio.draw(
        Screen::new("messages-failed", with_timeline(empty)),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    let mut stale = general().timeline();
    stale.stale = true;
    studio.draw(
        Screen::new("messages-stale", with_timeline(stale)),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;

    // The chat of a voice channel that the person is not in (view G), and the chat of the
    // call of the person, in its panel (view B).
    studio.draw(
        Screen::new(
            "messages-voice-channel",
            with_timeline(daily_chat().timeline()),
        ),
        |shell, _, cx| {
            prepare(shell, none(), cx);
            select_channel(shell, "Daily", cx);
        },
    )?;
    let mut in_daily = acme_with_luca(|_| {});
    if let Some(model) = &mut in_daily.model {
        Arc::make_mut(model).timelines = vec![Arc::new(daily_chat().timeline())];
    }
    studio.draw(
        Screen::call("messages-call-chat", in_call(Phase::Connected))
            .servers(vec![in_daily.clone(), harbor()]),
        |shell, window, cx| {
            prepare(shell, none(), cx);
            select_channel(shell, "Daily", cx);
            shell.open(Open::CallTab(CallTab::Chat), window, cx);
        },
    )?;
    studio.draw(
        Screen::call("messages-call-chat-150", in_call(Phase::Connected))
            .servers(vec![in_daily, harbor()])
            .scale(150),
        |shell, window, cx| {
            prepare(shell, none(), cx);
            select_channel(shell, "Daily", cx);
            shell.open(Open::CallTab(CallTab::Chat), window, cx);
        },
    )?;
    let _: MessageId = id(1);
    Ok(())
}
