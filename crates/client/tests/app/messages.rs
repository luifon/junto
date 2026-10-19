//! Messages in the window against the real server (ADR 0740): a person writes, answers,
//! changes and deletes messages, reacts, sends and saves a file, and the unread signs
//! follow what another account writes.

use gpui::TestAppContext;
use junto_client::shell::{EmojiTarget, Pane};
use junto_client::state::feed::{Feed, FeedRow, MessageRow, QuoteText, plain_text};
use junto_client::state::view::ServerStatus;
use junto_client::text;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{AccountId, AttachmentId, ChannelId, MessageId};
use junto_client_core::protocol::message::{Block, Document, Inline, Message, SendMessage};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::transfers::{DownloadStage, UploadSource, UploadStage};

use crate::harness::{Admin, Window, World, member};
use crate::support::FaultProxy;

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
fn files_attached_before_a_send_are_in_the_message_in_the_order_of_their_drops(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let files = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..6).map(|number| format!("part-{number}.txt")).collect();
    let paths: Vec<std::path::PathBuf> = names
        .iter()
        .map(|name| {
            let path = files.path().join(name);
            std::fs::write(&path, name.as_bytes()).unwrap();
            path
        })
        .collect();

    // Six drops, one file each, and Enter before the sizes of the files are read: the
    // message is not sent without them.
    let problem = window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "The parts", window, cx);
        for path in &paths {
            shell.attach_paths(Pane::Main, vec![path.clone()], cx);
        }
        shell.submit_message(Pane::Main, window, cx);
        shell.composer_problem(Pane::Main).map(str::to_owned)
    });
    assert_eq!(problem.as_deref(), Some(text::FILES_STILL_ATTACHING));

    // The files are in the draft in the order of the drops, and the message is sent with
    // them.
    window.until("the files are in the draft", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.draft.len() == names.len())
    });
    let drafted: Vec<String> = window
        .view()
        .workspace()
        .unwrap()
        .draft
        .iter()
        .map(|file| file.name.clone())
        .collect();
    assert_eq!(drafted, names);
    assert_eq!(
        window.in_shell(|shell, _, _| shell.composer_problem(Pane::Main).map(str::to_owned)),
        None
    );
    window.in_shell(|shell, window, cx| shell.submit_message(Pane::Main, window, cx));
    let sent = window.until_message("The parts");
    let attached: Vec<String> = sent
        .message
        .attachments
        .iter()
        .map(|attachment| attachment.name.clone())
        .collect();
    assert_eq!(attached, names);
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

    // A saved file of a message is not opened (ADR 0740), as a saved recording is. In a
    // test the application keeps the path of a file that it opens, and an opening through
    // the system panics on the test platform.
    window.settle();
    let opened = window
        .state
        .read_with(&window.cx, |state, _| state.kept_opened().to_vec());
    assert!(opened.is_empty(), "{opened:?}");
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
fn a_new_account_can_mark_positions_seen_by_the_previous_account(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let first = window.own_account();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    let unread = |window: &mut Window| {
        window
            .channels()
            .into_iter()
            .find(|row| row.id == voice.id)
            .map_or(0, |row| row.unread.messages)
    };
    for number in 0..3 {
        admin.send(&world, voice.id, document(&format!("message {number}")));
    }
    // The first account reads the chat of the voice channel to its end.
    window.until("the unread sign", |window| unread(window) == 3);
    window.click_channel(&voice.name);
    window.until("the chat is read by the first account", |window| {
        if window.shows("mark-as-read") {
            window.click("mark-as-read");
        }
        unread(window) == 0
    });

    // The account is removed, and the person joins again. For the new account the same
    // messages are unread, and it marks them as read: the read position that the first
    // account reached does not stop the request of the new one.
    world.run(admin.handle.api().remove_account(first)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    assert_ne!(window.own_account(), first);
    window.until("the unread sign of the new account", |window| {
        unread(window) == 3
    });
    window.click_channel(&voice.name);
    window.until("the chat is read by the new account", |window| {
        if window.shows("mark-as-read") {
            window.click("mark-as-read");
        }
        unread(window) == 0
    });
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
        .snapshot
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

impl Window {
    /// How many requests of the window for a change of a message have no answer yet.
    fn message_asks_flying(&mut self) -> usize {
        self.state
            .read_with(&self.cx, |state, _| state.message_asks_flying())
    }

    fn holds_message(&mut self, message_id: MessageId) -> bool {
        self.messages()
            .iter()
            .any(|row| row.message.id == message_id)
    }

    /// The person writes a message and changes it. The server stores the change, and its
    /// answer to the request waits at the proxy. The window shows the change, which the
    /// event told. Then the administrator deletes the message, and the window shows that.
    fn edit_with_a_held_answer_and_delete(
        &mut self,
        world: &World,
        proxy: &FaultProxy,
        admin: &Admin,
        text: &str,
    ) -> MessageId {
        self.write(text);
        let message_id = self.until_message(text).message.id;
        proxy.hold_answer_ending("PUT", &format!("/messages/{message_id}"));
        self.in_shell(|shell, window, cx| shell.edit_message(Pane::Main, message_id, window, cx));
        let changed = format!("{text}, changed");
        self.write(&changed);
        self.until("the answer to the edit waits at the proxy", |_| {
            proxy.answers_waiting() == 1
        });
        assert_eq!(self.until_message(&changed).message.id, message_id);
        world
            .run(admin.handle.api().delete_message(message_id))
            .unwrap();
        self.until("the deleted message is gone", |window| {
            !window.holds_message(message_id)
        });
        assert_eq!(self.message_asks_flying(), 1);
        message_id
    }
}

#[gpui::test]
fn a_late_message_refusal_is_not_shown_at_another_composer(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let first = window.feed().unwrap().place;
    window.write("one");
    let message_id = window.until_message("one").message.id;
    // A new member may not pin: the server refuses the pin, and its answer waits.
    proxy.hold_answer_ending("PUT", &format!("/messages/{message_id}/pin"));
    let server_id = window.view().selected.unwrap().id;
    window.in_shell(|shell, _, cx| {
        shell.state().update(cx, |state, cx| {
            state.set_pinned(server_id, message_id, true, cx);
        });
    });
    window.until("the refusal waits at the proxy", |_| {
        proxy.answers_waiting() == 1
    });

    // The person selects another channel before the refusal arrives: its composer does
    // not tell the refusal, and neither does the composer of the first channel later.
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    window.click_channel(&voice.name);
    window.until("the chat of the voice channel", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Channel(voice.id))
    });
    proxy.release();
    window.until("the refusal has arrived", |window| {
        window.message_asks_flying() == 0
    });
    assert!(!window.shows("compose-problem"));
    window.until("the first channel again", |window| {
        if window.feed().is_some_and(|feed| feed.place != first) {
            window.click_channel("general");
        }
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == first)
    });
    assert!(!window.shows("compose-problem"));

    // A refusal in the place that the person stays in is told there.
    window.in_shell(|shell, _, cx| {
        shell.state().update(cx, |state, cx| {
            state.set_pinned(server_id, message_id, true, cx);
        });
    });
    window.until("the refusal is told", |window| {
        window.shows("compose-problem")
    });
}

#[gpui::test]
fn a_late_edit_answer_cannot_restore_deleted_content(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let message_id = window.edit_with_a_held_answer_and_delete(&world, &proxy, &admin, "one");

    // The answer to the edit arrives after the deletion. It holds the message as it was
    // before the deletion, and nothing of it enters the list.
    proxy.release();
    window.until("the answer to the edit has arrived", |window| {
        window.message_asks_flying() == 0
    });
    assert!(!window.holds_message(message_id));
    assert!(window.texts().iter().all(|text| text != "one, changed"));
    // The list follows the place as before.
    window.write("two");
    window.until_message("two");
    assert!(!window.holds_message(message_id));
}

#[gpui::test]
fn a_late_edit_answer_cannot_restore_deleted_content_for_the_next_account(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let first = window.own_account();
    let message_id = window.edit_with_a_held_answer_and_delete(&world, &proxy, &admin, "one");

    // The account is removed, and the person joins again: the entry of the server has a
    // new account, which reads the place.
    world.run(admin.handle.api().remove_account(first)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    assert_ne!(window.own_account(), first);
    window.until("the messages are read for the new account", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    assert!(!window.holds_message(message_id));

    // The answer to the edit of the account before arrives now.
    proxy.release();
    window.until("the answer to the edit has arrived", |window| {
        window.message_asks_flying() == 0
    });
    assert!(!window.holds_message(message_id));
    assert!(window.texts().iter().all(|text| text != "one, changed"));
    window.write("of the new account");
    window.until_message("of the new account");
    assert!(!window.holds_message(message_id));
}

#[gpui::test]
fn a_pending_file_picker_cannot_retarget_an_upload(cx: &mut TestAppContext) {
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&second));
    let first_admin = world.owner(&first);
    let second_admin = world.owner(&second);
    let (_directory, mut window) = member(cx, &world, &first.address(), &first_admin, "Luca");
    let code = second_admin.invite(&world, 1);
    window.find_server(&proxy.address());
    window.join_with_code(code.expose());
    window.until("the second server is shown", |window| {
        let view = window.view();
        view.servers.len() == 2
            && view.servers[1].selected
            && view.servers[1].status == ServerStatus::Ready
            && view.workspace().is_some()
    });
    let shows = |window: &mut Window, name: &str| {
        let view = window.view();
        view.selected
            .as_ref()
            .is_some_and(|server| server.name == name)
            && window.feed().is_some_and(|feed| feed.loaded)
            && window.shows("composer")
    };
    window.click("server-0");
    window.until("the first server is shown", |window| {
        shows(window, "Acme Studio")
    });

    // The person opens the file dialog for a message on the first server.
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("for the first server.txt");
    std::fs::write(&path, b"what the person wrote for the first server").unwrap();
    assert!(!window.cx.did_prompt_for_paths());
    window.click("attach-file");
    assert!(window.cx.did_prompt_for_paths());

    // The dialog does not stop the window. The person selects the second server, and
    // chooses the file in the dialog afterwards.
    window.click("server-1");
    window.until("the second server is shown", |window| {
        shows(window, "Harbor")
    });
    let chosen = path.clone();
    window
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![chosen]));
    window.cx.run_until_parked();

    // Nothing is attached and nothing is uploaded, and the composer says why.
    assert_eq!(
        window.in_shell(|shell, _, _| shell.composer_problem(Pane::Main).map(str::to_owned)),
        Some(text::FILES_OF_ANOTHER_PLACE.to_owned())
    );
    assert!(window.view().workspace().unwrap().draft.is_empty());
    window.write("a message on the second server");
    let sent = window.until_message("a message on the second server");
    assert!(sent.message.attachments.is_empty());
    let uploads = |proxy: &FaultProxy| {
        proxy
            .all()
            .iter()
            .filter(|seen| seen.path.starts_with("/api/v1/attachments"))
            .count()
    };
    assert_eq!(uploads(&proxy), 0, "the second server got no file");
    window.click("server-0");
    window.until("the first server is shown again", |window| {
        shows(window, "Acme Studio")
    });
    assert!(window.view().workspace().unwrap().draft.is_empty());

    // The same holds for another place of one server: the dialog is opened in the text
    // channel and answered in the chat of the voice channel.
    let text_channel = window.selected_channel();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    window.click("attach-file");
    window.click_channel(&voice.name);
    window.until("the chat of the voice channel is shown", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Channel(voice.id))
            && window.shows("composer")
    });
    let chosen = path.clone();
    window
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![chosen]));
    window.cx.run_until_parked();
    assert_eq!(
        window.in_shell(|shell, _, _| shell.composer_problem(Pane::Main).map(str::to_owned)),
        Some(text::FILES_OF_ANOTHER_PLACE.to_owned())
    );
    assert!(window.view().workspace().unwrap().draft.is_empty());
    let text_row = window
        .channels()
        .into_iter()
        .find(|row| row.id == text_channel)
        .unwrap();
    window.click_channel(&text_row.name);
    window.until("the text channel is shown again", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Channel(text_channel))
            && window.shows("composer")
    });
    assert!(window.view().workspace().unwrap().draft.is_empty());

    // A dialog that is answered while the composer still shows its place attaches.
    window.click("attach-file");
    let chosen = path.clone();
    window
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![chosen]));
    window.until("the upload to the first server is complete", |window| {
        window.view().workspace().is_some_and(|workspace| {
            workspace.draft.len() == 1
                && matches!(workspace.draft[0].progress.stage, UploadStage::Complete(_))
        })
    });
    assert_eq!(uploads(&proxy), 0);
}

/// A PNG image of one colour.
fn png(side: u32) -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(side, side, image::Rgba([200, 90, 40, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[gpui::test]
fn the_picture_of_a_message_is_read_again_for_the_next_account_of_the_server(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let first = window.own_account();
    let place = PlaceId::Channel(window.selected_channel());

    // The administrator sends a message with a picture.
    let attachment_id: AttachmentId = world.run(async {
        let source = UploadSource::Bytes {
            name: "chart.png".to_owned(),
            bytes: std::sync::Arc::new(png(32)),
        };
        let attachment_id = admin
            .handle
            .transfers()
            .upload(place, source)
            .finished()
            .await
            .unwrap();
        let message = SendMessage {
            document: document("A chart"),
            reply_to: None,
            attachments: vec![attachment_id],
        };
        admin
            .handle
            .api()
            .send_message(place, &message)
            .await
            .unwrap();
        attachment_id
    });
    window.until_message("A chart");
    let owners =
        |window: &mut Window| window.in_shell(|shell, _, _| shell.owners_of_picture(attachment_id));
    let file = format!("/api/v1/attachments/{attachment_id}/file");
    window.until("the picture is read", |window| owners(window).len() == 1);
    assert_eq!(owners(&mut window)[0].account_id, first);
    assert_eq!(proxy.seen(&file).len(), 1);
    // The person also saves the file.
    let downloads = tempfile::tempdir().unwrap();
    let server_id = window.view().selected.unwrap().id;
    let saved_file = window.until_message("A chart").message.attachments[0].clone();
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell.state().clone().update(cx, |state, cx| {
            state.set_download_directory(target);
            state.download(server_id, &saved_file, cx);
        });
    });
    window.until("the file is saved", |window| {
        window
            .view()
            .downloads
            .values()
            .any(|progress| matches!(progress.stage, DownloadStage::Saved(_)))
    });
    assert_eq!(proxy.seen(&file).len(), 2);

    // The account is removed, and the person joins again. The window holds no picture of
    // the account before, and the new account reads the picture from the server.
    world.run(admin.handle.api().remove_account(first)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    assert!(owners(&mut window).is_empty());
    assert!(
        window.view().downloads.is_empty(),
        "the window shows no download of the account before"
    );
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    let second = window.own_account();
    assert_ne!(second, first);
    window.until_message("A chart");
    window.until("the picture is read for the new account", |window| {
        owners(window).len() == 1
    });
    assert_eq!(owners(&mut window)[0].account_id, second);
    assert_eq!(proxy.seen(&file).len(), 3);
    assert!(window.view().downloads.is_empty());
}

/// The shell states the place where it drew the message list, and the wheel of the pointer
/// over that place scrolls the list. The measurement of the modes (ADR 0914) scrolls a
/// build without debug assertions, where gpui keeps no names of elements, at this place.
#[gpui::test]
fn the_wheel_over_the_stated_place_of_the_message_list_scrolls_it(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    for number in 0..40 {
        admin.send(&world, general, document(&format!("Line {number}")));
    }
    window.until_message("Line 39");
    window.settle();
    let stated = window
        .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
        .expect("the list was drawn");
    assert_eq!(Some(stated), window.cx.debug_bounds("message-list"));
    assert!(window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main)));
    window.dispatch(ScrollWheelEvent {
        position: stated.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(2000.0))),
        ..Default::default()
    });
    window.settle();
    assert!(!window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main)));
}

/// Only the message under the pointer carries the bar of its actions: the other rows build
/// and lay out no hidden buttons at each frame (ADR 0914). A scroll of the list takes the
/// bar away until the pointer moves again.
#[gpui::test]
fn only_the_message_under_the_pointer_has_the_bar_of_actions(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, point, px};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let sent: Vec<MessageId> = (0..30)
        .map(|number| {
            admin
                .send(&world, general, document(&format!("Line {number}")))
                .id
        })
        .collect();
    window.until_message("Line 29");
    window.settle();
    // The pointer leaves the list for the column of the channels, wherever the steps
    // before left it.
    let list = window
        .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
        .expect("the list was drawn");
    window.cx.simulate_mouse_move(
        point(list.left() - px(20.0), list.top() + px(20.0)),
        None::<MouseButton>,
        Modifiers::none(),
    );
    window.settle();
    let bars = |window: &mut Window| {
        sent.iter()
            .filter(|id| {
                window
                    .cx
                    .debug_bounds(crate::harness::selector(format!("message-actions:{id}")))
                    .is_some()
            })
            .copied()
            .collect::<Vec<_>>()
    };
    assert_eq!(bars(&mut window), Vec::<MessageId>::new());

    let last = *sent.last().unwrap();
    let row = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{last}")))
        .expect("the newest message is drawn");
    window
        .cx
        .simulate_mouse_move(row.center(), None::<MouseButton>, Modifiers::none());
    window.settle();
    assert_eq!(bars(&mut window), vec![last]);

    window.dispatch(ScrollWheelEvent {
        position: row.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(300.0))),
        ..Default::default()
    });
    window.settle();
    assert_eq!(bars(&mut window), Vec::<MessageId>::new());
}

#[gpui::test]
fn an_administrator_deletes_one_permanent_message_of_another_person(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = crate::admin_pages::owner_window(cx, &server);
    let code = crate::admin_pages::invite(&world, &owner, Vec::new());
    let (_device, luca) = crate::admin_pages::joined(&world, &server, &code, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let change = junto_client_core::protocol::structure::UpdateChannel {
        permanent_messages: Some(true),
        ..Default::default()
    };
    world
        .run(owner.api().update_channel(general, &change))
        .unwrap();
    let send = |text: &str| {
        let message = SendMessage {
            document: document(text),
            reply_to: None,
            attachments: Vec::new(),
        };
        world
            .run(luca.api().send_message(PlaceId::Channel(general), &message))
            .unwrap()
    };
    let kept = send("The budget is approved.");
    let wrong = send("The budget is approved twice.");
    window.until_message("The budget is approved.");
    let row = window.until_message("The budget is approved twice.");
    assert!(row.message.permanent && !row.own);
    assert!(row.can_delete && !row.can_edit);

    // The bar of the message under the pointer has Delete, and the deletion asks first
    // with what stays of the message.
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{}", wrong.id)))
        .expect("the message is drawn");
    window
        .cx
        .simulate_mouse_move(bounds.center(), None::<MouseButton>, Modifiers::none());
    window.settle();
    window.click_named(format!("delete:{}", wrong.id));
    assert!(window.shows("confirm-delete-message") && window.shows("confirm-delete-permanent"));
    window.click("delete-message");
    window.until(
        "the message is gone and the channel states the deletion",
        |window| {
            let feed = window.feed().unwrap();
            let gone = feed
                .rows
                .iter()
                .all(|row| !matches!(row, FeedRow::Message(row) if row.message.id == wrong.id));
            let line = feed.rows.iter().any(|row| {
                matches!(row, FeedRow::System(line)
                if line.message.system.as_ref().is_some_and(|system| {
                    system.action == "permanent_message_deleted"
                }))
            });
            gone && line
        },
    );
    assert!(
        window
            .messages()
            .iter()
            .any(|row| row.message.id == kept.id)
    );
}

#[gpui::test]
fn a_message_whose_files_the_retention_removed_says_so(cx: &mut TestAppContext) {
    use sqlx::ConnectOptions;
    let world = World::new();
    let mut server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("draft.txt");
    std::fs::write(&path, b"the first draft").unwrap();
    window.in_shell(|shell, _, cx| shell.attach_paths(Pane::Main, vec![path.clone()], cx));
    window.until("the upload is complete", |window| {
        window.view().workspace().is_some_and(|workspace| {
            workspace.draft.len() == 1
                && matches!(workspace.draft[0].progress.stage, UploadStage::Complete(_))
        })
    });
    window.write("The draft");
    let sent = window.until_message("The draft");
    assert_eq!(sent.message.attachments.len(), 1);
    let removed = crate::harness::selector(format!("files-removed:{}", sent.message.id));
    assert!(!window.shows(removed));

    // The workspace keeps files for one day, and the file was attached two days ago. The
    // server applies the retention when it starts.
    let workspace_id = window.view().workspace().unwrap().id;
    world
        .run(admin.handle.api().set_file_retention(
            workspace_id,
            &junto_client_core::protocol::discovery::Retention { days: Some(1) },
        ))
        .unwrap();
    world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url.parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query("update attachments set retention_at = now() - interval '2 days'")
            .execute(&mut database)
            .await
            .unwrap();
    });
    world.run(server.restart());

    // The message stays with its text, without the file, and states why.
    window.until("the message states that its files were removed", |window| {
        window.shows(removed)
    });
    let row = window.until_message("The draft");
    assert!(row.message.files_removed_by_retention);
    assert!(row.message.attachments.is_empty());
    assert!(!window.shows_named("file:0".to_owned()));
}
