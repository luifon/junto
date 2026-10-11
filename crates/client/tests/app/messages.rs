//! Messages in the window against the real server (ADR 0740): a person writes, answers,
//! changes and deletes messages, reacts, sends and saves a file, and the unread signs
//! follow what another account writes.

use gpui::TestAppContext;
use junto_client::shell::{EmojiTarget, Pane};
use junto_client::state::feed::{Feed, FeedRow, MessageRow, QuoteText, plain_text};
use junto_client::text;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{AccountId, ChannelId, MessageId};
use junto_client_core::protocol::message::{Block, Document, Inline, Message, SendMessage};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::transfers::{DownloadStage, UploadStage};

use crate::harness::{Admin, Window, World, member};

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

impl Admin {
    pub fn send(&self, world: &World, channel_id: ChannelId, document: Document) -> Message {
        let message = SendMessage {
            document,
            reply_to: None,
            attachments: Vec::new(),
        };
        world
            .run(
                self.handle
                    .api()
                    .send_message(PlaceId::Channel(channel_id), &message),
            )
            .unwrap()
    }
}

impl Window {
    pub fn feed(&mut self) -> Option<Feed> {
        self.view().workspace()?.feed.clone()
    }

    /// The messages of people that the window draws, in their order.
    pub fn messages(&mut self) -> Vec<MessageRow> {
        self.feed()
            .map(|feed| {
                feed.rows
                    .iter()
                    .filter_map(|row| match row {
                        FeedRow::Message(row) => Some(row.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn texts(&mut self) -> Vec<String> {
        let Some(feed) = self.feed() else {
            return Vec::new();
        };
        feed.rows
            .iter()
            .filter_map(|row| match row {
                FeedRow::Message(row) => row
                    .message
                    .document
                    .as_ref()
                    .map(|document| plain_text(document, &feed.names, 200)),
                _ => None,
            })
            .collect()
    }

    fn waiting(&mut self) -> usize {
        self.feed().map_or(0, |feed| {
            feed.rows
                .iter()
                .filter(|row| matches!(row, FeedRow::Waiting(_)))
                .count()
        })
    }

    #[track_caller]
    pub fn until_message(&mut self, text: &str) -> MessageRow {
        self.until(&format!("the message {text:?} is drawn"), |window| {
            window.waiting() == 0 && window.texts().iter().any(|held| held == text)
        });
        let feed = self.feed().unwrap();
        self.messages()
            .into_iter()
            .find(|row| {
                row.message
                    .document
                    .as_ref()
                    .is_some_and(|document| plain_text(document, &feed.names, 200) == text)
            })
            .unwrap()
    }

    /// Types a text into the composer and sends it with Enter.
    fn write(&mut self, text: &str) {
        self.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, text, window, cx);
            shell.focus_composer(Pane::Main, window, cx);
        });
        self.press("enter");
    }

    pub fn selected_channel(&mut self) -> ChannelId {
        self.view()
            .workspace()
            .and_then(|workspace| workspace.channel.as_ref().map(|channel| channel.id))
            .unwrap()
    }

    pub fn own_account(&mut self) -> AccountId {
        self.view().person.account_id.unwrap()
    }
}

#[gpui::test]
fn a_person_writes_answers_changes_and_deletes_messages(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel_id = window.selected_channel();
    assert!(window.shows("composer") && window.shows("send-message"));
    assert!(!window.shows("messages-state"));

    // Enter sends the text of the composer, and the marks of the text become formatting.
    window.write("Hello **world**, see `main.rs`");
    let hello = window.until_message("Hello world, see main.rs");
    assert!(hello.own && hello.can_edit && hello.can_delete);
    assert_eq!(hello.author, "Luca");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "",
        "the composer is empty after a send"
    );
    assert!(window.shows_named(format!("message:{}", hello.message.id)));

    // Shift and Enter adds a line and sends nothing.
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "first line", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.press("shift-enter");
    assert_eq!(window.waiting(), 0);
    assert!(
        window
            .in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx))
            .starts_with("first line\n")
    );
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "", window, cx));

    // A text that the server would refuse is not sent, and the composer says why.
    window.write("   ");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.composer_problem(Pane::Main).map(str::to_owned)),
        Some(text::compose_problem(junto_client::compose::Problem::Empty))
    );
    window.write(&"word ".repeat(4000));
    assert!(window.shows("compose-problem"));
    assert_eq!(window.messages().len(), 1);
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "", window, cx));

    // The administrator writes with a mention of the person: the message arrives with
    // the name of the person in it, and is marked.
    let own = window.own_account();
    let mention = Document {
        blocks: vec![Block::Paragraph {
            content: vec![
                Inline::Text {
                    text: "Welcome, ".to_owned(),
                    marks: Vec::new(),
                },
                Inline::Account { account_id: own },
            ],
        }],
    };
    let welcome = admin.send(&world, channel_id, mention).id;
    let arrived = window.until_message("Welcome, @Luca");
    assert!(arrived.mentions_own && !arrived.own && !arrived.can_edit);
    assert_eq!(arrived.author, "Ada");

    // A reply names its message, and shows the start of it.
    window.in_shell(|shell, window, cx| shell.reply_to_message(Pane::Main, welcome, window, cx));
    assert!(window.shows("compose-state"));
    window.write("Thank you");
    let reply = window.until_message("Thank you");
    let quote = reply.reply.unwrap();
    assert_eq!(quote.message_id, welcome);
    assert_eq!(quote.author.as_deref(), Some("Ada"));
    assert_eq!(quote.text, QuoteText::Text("Welcome, @Luca".to_owned()));
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, None)
    );

    // The arrow up in an empty composer edits the last own message; the composer holds
    // its text with the marks, and Enter saves.
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("up");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, Some(reply.message.id))
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "Thank you"
    );
    window.press("escape");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, None)
    );
    window
        .in_shell(|shell, window, cx| shell.edit_message(Pane::Main, hello.message.id, window, cx));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "Hello **world**, see `main.rs`"
    );
    window.write("Hello *everyone*");
    let edited = window.until_message("Hello everyone");
    assert_eq!(edited.message.id, hello.message.id);
    assert!(edited.message.edited_at.is_some());

    // A reaction is added through the picker and taken away at its sign.
    window.in_shell(|shell, window, cx| {
        shell.open_emoji_picker(EmojiTarget::React(welcome), window, cx);
        shell.find_emoji("thumbs up", window, cx);
    });
    assert!(window.shows("emoji-picker"));
    window.click_named("emoji:\u{1f44d}".to_owned());
    assert!(!window.shows("emoji-picker"));
    let reactions = |window: &mut Window| {
        window
            .messages()
            .into_iter()
            .find(|row| row.message.id == welcome)
            .map(|row| row.message.reactions.clone())
            .unwrap_or_default()
    };
    window.until("the reaction is drawn", |window| {
        let held = reactions(window);
        held.len() == 1 && held[0].emoji == "\u{1f44d}" && held[0].own && held[0].count == 1
    });
    window.click_named(format!("reaction:{welcome}:0"));
    window.until("the reaction is gone", |window| {
        reactions(window).is_empty()
    });

    // The menu of a message has its actions for the keyboard, and a deletion asks first.
    window.in_shell(|shell, window, cx| {
        shell.open_message_menu(Pane::Main, reply.message.id, window, cx)
    });
    assert!(window.shows("message-menu") && window.shows("message-edit"));
    window.click("message-delete");
    assert!(window.shows("confirm-delete-message"));
    window.click("cancel-delete-message");
    assert!(!window.shows("confirm-delete-message"));
    assert_eq!(window.messages().len(), 3);
    window.in_shell(|shell, window, cx| {
        shell.ask_to_delete_message(reply.message.id, window, cx);
    });
    window.click("delete-message");
    window.until("the message is gone", |window| {
        window
            .messages()
            .iter()
            .all(|row| row.message.id != reply.message.id)
    });
    // The menu of a message of another person has no edit and no delete for a member.
    window.in_shell(|shell, window, cx| shell.open_message_menu(Pane::Main, welcome, window, cx));
    assert!(window.shows("message-reply"));
    assert!(!window.shows("message-edit") && !window.shows("message-delete"));
    window.press("escape");
    assert!(!window.shows("message-menu"));

    // A message that the administrator deletes leaves the list without an action of the
    // person.
    world
        .run(admin.handle.api().delete_message(welcome))
        .unwrap();
    window.until("the deleted message is gone", |window| {
        window
            .messages()
            .iter()
            .all(|row| row.message.id != welcome)
    });
    assert_eq!(window.texts(), ["Hello everyone"]);
}

#[gpui::test]
fn a_file_is_sent_with_a_message_and_saved_from_one(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let files = tempfile::tempdir().unwrap();
    let content: Vec<u8> = (0..40_000u32).map(|number| (number % 249) as u8).collect();
    let path = files.path().join("timings.csv");
    std::fs::write(&path, &content).unwrap();

    // The file is attached before the send: its upload starts, and the composer shows it.
    window.in_shell(|shell, _, cx| shell.attach_paths(Pane::Main, vec![path.clone()], cx));
    window.until("the upload is complete", |window| {
        window.view().workspace().is_some_and(|workspace| {
            workspace.draft.len() == 1
                && workspace.draft[0].name == "timings.csv"
                && matches!(workspace.draft[0].progress.stage, UploadStage::Complete(_))
        })
    });
    assert!(window.shows_named("draft-file:0".to_owned()));
    // A file that does not exist is not attached, and the composer says so.
    window.in_shell(|shell, _, cx| {
        shell.attach_paths(Pane::Main, vec![files.path().join("missing.bin")], cx);
    });
    assert!(window.shows("compose-problem"));
    assert_eq!(window.view().workspace().unwrap().draft.len(), 1);

    // The message is sent with its file.
    window.write("The timings");
    let sent = window.until_message("The timings");
    assert_eq!(sent.message.attachments.len(), 1);
    let attachment = sent.message.attachments[0].clone();
    assert_eq!(
        (attachment.name.as_str(), attachment.bytes),
        ("timings.csv", 40_000)
    );
    assert!(window.view().workspace().unwrap().draft.is_empty());
    assert!(window.shows_named("file:0".to_owned()));

    // The file is saved to the download directory under a name that no file has.
    let downloads = tempfile::tempdir().unwrap();
    std::fs::write(downloads.path().join("timings.csv"), b"mine").unwrap();
    let server_id = window.view().selected.unwrap().id;
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell.state().clone().update(cx, |state, cx| {
            state.set_download_directory(target);
            state.download(server_id, &attachment, cx);
        });
    });
    let saved = downloads.path().join("timings (1).csv");
    window.until("the file is saved", |window| {
        window
            .view()
            .downloads
            .values()
            .any(|progress| matches!(&progress.stage, DownloadStage::Saved(path) if *path == saved))
    });
    assert_eq!(std::fs::read(&saved).unwrap(), content);
    assert_eq!(
        std::fs::read(downloads.path().join("timings.csv")).unwrap(),
        b"mine"
    );
}

#[gpui::test]
fn the_unread_signs_follow_other_channels_and_end_when_the_messages_are_read(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let own = window.own_account();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    let unread = |window: &mut Window, channel_id: ChannelId| {
        window
            .channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .map(|row| (row.unread.messages, row.unread.mentions))
            .unwrap()
    };
    assert_eq!(unread(&mut window, voice.id), (0, 0));

    // The chat of the voice channel gets two messages, one with a mention: its row, its
    // workspace and its server show them.
    admin.send(&world, voice.id, document("Is anyone here?"));
    let mention = Document {
        blocks: vec![Block::Paragraph {
            content: vec![Inline::Account { account_id: own }],
        }],
    };
    let last: MessageId = admin.send(&world, voice.id, mention).id;
    window.until("the unread sign of the voice channel", |window| {
        unread(window, voice.id) == (2, 1)
    });
    let view = window.view();
    assert_eq!(view.servers[0].unread.mentions, 1);
    assert_eq!(view.servers[0].workspaces[0].unread.messages, 2);

    // The person opens the chat of the voice channel: it shows the line of the first
    // unread message, and the messages count as read when the person has seen them.
    window.click_channel(&voice.name);
    window.until("the chat of the voice channel is read", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Channel(voice.id))
    });
    assert!(window.texts().iter().any(|text| text == "Is anyone here?"));
    assert!(
        window
            .messages()
            .iter()
            .any(|row| row.message.id == last && row.mentions_own)
    );
    window.until("the messages are read", |window| {
        if window.shows("mark-as-read") {
            window.click("mark-as-read");
        }
        unread(window, voice.id) == (0, 0)
    });
    let view = window.view();
    assert!(!view.servers[0].unread.any());
    assert_eq!(
        world
            .run(admin.handle.api().message_state(PlaceId::Channel(voice.id)))
            .unwrap()
            .unread,
        0,
        "the own messages of the administrator are not unread"
    );
}

#[gpui::test]
fn the_text_of_one_account_is_not_shown_to_the_next_account_of_the_server(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let first = window.own_account();
    window.until("the composer", |window| window.shows("composer"));
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "a text of the first account", window, cx)
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "a text of the first account"
    );

    // The account is removed, and the person joins again: the device has a new account.
    world.run(admin.handle.api().remove_account(first)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    assert_ne!(window.own_account(), first);
    window.until("the composer of the new account", |window| {
        window.shows("composer")
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
}

#[gpui::test]
fn a_quoted_message_that_is_not_loaded_follows_its_changes(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let channel_id = world
        .run(api.snapshot())
        .unwrap()
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.channels)
        .find(|channel| channel.kind == ChannelKind::Text)
        .unwrap()
        .id;
    // More messages than one page lie between a message and its answer: the window that
    // opens the channel afterwards loads the answer and not the message.
    let quoted = admin.send(&world, channel_id, document("the first words"));
    for number in 0..60 {
        admin.send(&world, channel_id, document(&format!("between {number}")));
    }
    let answer = SendMessage {
        document: document("an answer"),
        reply_to: Some(quoted.id),
        attachments: Vec::new(),
    };
    world
        .run(api.send_message(PlaceId::Channel(channel_id), &answer))
        .unwrap();

    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    assert_eq!(window.selected_channel(), channel_id);
    let quote_of_answer = |window: &mut Window| {
        window
            .messages()
            .into_iter()
            .find(|row| row.message.reply_to == Some(quoted.id))
            .and_then(|row| row.reply)
            .map(|quote| quote.text)
    };
    window.until("the quoted message is read", |window| {
        quote_of_answer(window) == Some(QuoteText::Text("the first words".to_owned()))
    });
    assert!(
        window
            .messages()
            .iter()
            .all(|row| row.message.id != quoted.id)
    );

    // The quoted message changes, and then it is deleted.
    world
        .run(api.edit_message(quoted.id, document("other words")))
        .unwrap();
    window.until("the quote shows the change", |window| {
        quote_of_answer(window) == Some(QuoteText::Text("other words".to_owned()))
    });
    world.run(api.delete_message(quoted.id)).unwrap();
    window.until("the quote shows the deletion", |window| {
        matches!(
            quote_of_answer(window),
            Some(QuoteText::Deleted | QuoteText::Unknown)
        )
    });
}
