//! Threads, pins and direct conversations in the window against the real server
//! (ADR 0741): a person answers in a thread, sees what is unread in it, asks for a fork,
//! pins a message, and writes to another person.

use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client::state::feed::{FeedRow, plain_text};
use junto_client::state::view::{ForkState, PinsState};
use junto_client::text;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, MessageId};
use junto_client_core::protocol::message::{Block, Document, Inline, SendMessage};
use junto_client_core::protocol::roles::{CreateWorkspaceRole, UpdateWorkspaceRole};
use junto_client_core::protocol::structure::ChannelKind;

use crate::harness::{Admin, Window, World, member};
use crate::messages::document;

impl Admin {
    pub(crate) fn reply(&self, world: &World, root_id: MessageId, document: Document) -> MessageId {
        let message = SendMessage {
            document,
            reply_to: None,
            attachments: Vec::new(),
        };
        world
            .run(self.handle.api().reply_in_thread(root_id, &message))
            .unwrap()
            .id
    }

    /// Gives every member of the first workspace these permissions too.
    pub(crate) fn allow_everyone(&self, world: &World, more: &[WorkspacePermission]) {
        let api = self.handle.api();
        let workspace_id = world.run(api.workspaces()).unwrap()[0].id;
        let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
        let everyone = roles.iter().find(|role| role.is_everyone).unwrap();
        let mut permissions = everyone.permissions.clone();
        permissions.extend_from_slice(more);
        let update = UpdateWorkspaceRole {
            name: None,
            permissions: Some(permissions),
        };
        world
            .run(api.update_workspace_role(workspace_id, everyone.id, &update))
            .unwrap();
    }
}

impl Window {
    /// The texts of the replies that the thread panel draws, in their order.
    pub(crate) fn replies(&mut self) -> Vec<String> {
        let view = self.view();
        let Some(feed) = view
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref())
        else {
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

    pub(crate) fn open_thread_of(&mut self, root_id: MessageId) {
        self.in_shell(|shell, window, cx| {
            shell.open_message_menu(Pane::Main, root_id, window, cx);
        });
        self.click("message-thread");
        self.until("the thread is read", |window| {
            window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| {
                    thread.root.is_some() && thread.feed.as_ref().is_some_and(|feed| feed.loaded)
                })
        });
    }

    pub(crate) fn write_reply(&mut self, text: &str) {
        self.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Thread, text, window, cx);
            shell.focus_composer(Pane::Thread, window, cx);
        });
        self.press("enter");
    }
}

/// A window of a member in the text channel of a new server, with one message of the
/// administrator in it.
pub(crate) fn with_root(
    cx: &mut TestAppContext,
    world: &World,
) -> (
    crate::support::TestServer,
    Admin,
    tempfile::TempDir,
    Window,
    ChannelId,
    MessageId,
) {
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel_id = window.selected_channel();
    let root = admin.send(world, channel_id, document("Tracks or mixdown?"));
    window.until_message("Tracks or mixdown?");
    (server, admin, directory, window, channel_id, root.id)
}

#[gpui::test]
fn a_person_answers_in_a_thread_and_sees_what_is_new_in_it(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _, root_id) = with_root(cx, &world);
    let own = window.own_account();

    // The menu of the message opens its thread in the side panel.
    assert!(!window.shows("thread-panel"));
    window.open_thread_of(root_id);
    assert!(window.shows("thread-panel") && window.shows("thread-root"));
    assert!(window.shows("thread-composer") && window.shows("composer"));
    assert!(window.shows("thread-no-messages"));
    // The channel column lists the open thread under its channel.
    assert!(window.shows_named(format!("thread-row:{root_id}")));

    // A reply is written in the composer of the panel. It is drawn in the panel and not
    // in the list of the channel, whose message counts it.
    window.write_reply("One per speaker.");
    window.until("the reply is drawn", |window| {
        window.replies() == ["One per speaker."]
    });
    assert_eq!(window.texts(), ["Tracks or mixdown?"]);
    window.until("the count of the replies", |window| {
        window.messages().iter().any(|row| {
            row.thread.as_ref().is_some_and(|mark| {
                mark.replies == 1
                    && mark.open
                    && mark.last.as_ref().is_some_and(|last| last.author == "Luca")
            })
        })
    });
    assert!(window.shows_named(format!("thread-chip:{root_id}")));
    assert!(window.shows("thread-replies"));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        ""
    );
    // The text of the channel composer is another text.
    window
        .in_shell(|shell, window, cx| shell.type_message(Pane::Main, "in the channel", window, cx));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        ""
    );

    // The panel is closed. Another person answers and names the person: the message of
    // the channel and the channel column show what is new in the thread.
    window.click("close-thread");
    assert!(!window.shows("thread-panel"));
    // The person wrote in the thread and follows it by the rule of the server: it stays
    // listed under its channel, without a sign (ADR 0796).
    window.until("the followed thread under its channel", |window| {
        window.shows_named(format!("thread-row:{root_id}"))
    });
    assert!(!window.shows_named(format!("thread-unread:{root_id}")));
    let mention = Document {
        blocks: vec![Block::Paragraph {
            content: vec![
                Inline::Text {
                    text: "Agreed, ".to_owned(),
                    marks: Vec::new(),
                },
                Inline::Account { account_id: own },
            ],
        }],
    };
    admin.reply(&world, root_id, mention);
    window.until("the unread thread", |window| {
        window.messages().iter().any(|row| {
            row.thread
                .as_ref()
                .is_some_and(|mark| mark.replies == 2 && mark.unread == 1 && mark.mentions == 1)
        })
    });
    window.until("the thread under its channel", |window| {
        window.channels().iter().any(|row| {
            row.threads.iter().any(|entry| {
                entry.root_id == root_id
                    && entry.mentions == 1
                    && entry.title.as_deref() == Some("Tracks or mixdown?")
            })
        })
    });
    assert!(window.shows_named(format!("thread-unread:{root_id}")));
    // The pictures of the two reply authors overlap each other and end before the count of
    // the replies starts.
    window.until("the two reply authors", |window| {
        window.messages().iter().any(|row| {
            row.thread
                .as_ref()
                .is_some_and(|mark| mark.authors.len() == 2)
        })
    });
    let bounds = |window: &mut Window, name: String| {
        let name: &'static str = Box::leak(name.into_boxed_str());
        window.cx.debug_bounds(name).unwrap()
    };
    let authors = bounds(&mut window, format!("thread-authors:{root_id}"));
    let count = bounds(&mut window, format!("thread-count:{root_id}"));
    assert!(authors.size.width.as_f32() > 15.0, "{authors:?}");
    assert!(authors.right() <= count.left(), "{authors:?} {count:?}");
    // The mention of the thread counts in the sign of its channel. The summary of the
    // channel arrives by itself, after the entry of the thread or before it.
    window.until("the mention in the sign of the channel", |window| {
        window.channels().iter().any(|row| row.unread.mentions == 1)
    });

    // The entry of the channel column opens the thread again, with both replies.
    window.click_named(format!("thread-row:{root_id}"));
    window.until("both replies", |window| window.replies().len() == 2);
    assert!(window.shows("thread-panel"));
}

#[gpui::test]
fn a_fork_tells_each_refusal_and_shows_the_new_channel(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _, root_id) = with_root(cx, &world);
    admin.reply(&world, root_id, document("One per speaker."));
    window.open_thread_of(root_id);
    let fork_state = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.fork.clone())
    };
    let may_fork = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| thread.may_fork)
    };

    // The roles of a new member hold no "manage channel": the controls say so, and the
    // server refuses the request.
    assert!(!may_fork(&mut window));
    window.in_shell(|shell, window, cx| {
        shell.open_fork_dialog(root_id, ChannelKind::Text, window, cx);
    });
    assert!(window.shows("fork-dialog"));
    window.in_shell(|shell, window, cx| {
        shell.type_fork_name("tracks", window, cx);
        shell.submit_fork(window, cx);
    });
    window.until("the refusal", |window| {
        fork_state(window) == Some(ForkState::Refused(text::FORK_NOT_ALLOWED.to_owned()))
    });
    assert!(window.shows("fork-refused") && window.shows("fork-dialog"));
    window.click("cancel-fork");
    assert!(!window.shows("fork-dialog"));
    assert_eq!(fork_state(&mut window), None);

    // With the permission the button opens the dialog, and the fork is made.
    admin.allow_everyone(&world, &[WorkspacePermission::ManageChannel]);
    window.until("the permission", may_fork);
    window.click("fork-text");
    assert!(window.shows("fork-dialog"));
    // A dialog without a name sends nothing.
    window.in_shell(|shell, window, cx| shell.submit_fork(window, cx));
    assert_eq!(fork_state(&mut window), None);
    // A name that the server does not take is told, and a changed name ends that.
    let long = "n".repeat(65);
    window.in_shell(|shell, window, cx| {
        shell.type_fork_name(&long, window, cx);
        shell.submit_fork(window, cx);
    });
    window.until("the refused name", |window| {
        fork_state(window) == Some(ForkState::Refused(text::FORK_NAME_REFUSED.to_owned()))
    });
    window.in_shell(|shell, window, cx| shell.type_fork_name("tracks", window, cx));
    assert_eq!(fork_state(&mut window), None);
    assert!(!window.shows("fork-refused"));
    window.click("create-fork");
    window.until("the fork channel", |window| {
        window.channel_names().iter().any(|name| name == "tracks")
    });
    window.until(
        "the dialog is closed and the thread names its fork",
        |window| {
            !window.shows("fork-dialog")
                && window
                    .view()
                    .workspace()
                    .and_then(|workspace| workspace.thread.as_ref())
                    .is_some_and(|thread| {
                        thread.forks.len() == 1
                            && thread.forks[0].1 == "tracks"
                            && thread.forks[0].2 == ChannelKind::Text
                    })
        },
    );
    assert!(window.shows("fork-link") && !window.shows("fork-text"));

    // A thread has one fork: the server refuses a second one.
    window.in_shell(|shell, window, cx| {
        shell.open_fork_dialog(root_id, ChannelKind::Voice, window, cx);
        shell.type_fork_name("tracks again", window, cx);
        shell.submit_fork(window, cx);
    });
    window.until("the second refusal", |window| {
        fork_state(window) == Some(ForkState::Refused(text::FORK_EXISTS.to_owned()))
    });
    window.click("cancel-fork");

    // The link of the thread shows the fork channel with the copies.
    window.click("fork-link");
    window.until("the copies in the fork channel", |window| {
        window.texts() == ["Tracks or mixdown?", "One per speaker."]
    });
    assert!(!window.shows("thread-panel"));
}

#[gpui::test]
fn a_person_pins_a_message_and_reads_the_pin_list(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, channel_id, root_id) = with_root(cx, &world);
    let pins = |window: &mut Window| -> Option<Vec<MessageId>> {
        let view = window.view();
        match &view.workspace()?.pins.as_ref()?.state {
            PinsState::Loaded(rows) => Some(rows.iter().map(|row| row.message.id).collect()),
            PinsState::Loading | PinsState::Failed => None,
        }
    };
    // A change of the permissions withdraws the messages of the workspace until the
    // place is read again (ADR 0748): for that time the message has no row.
    let held_row = |window: &mut Window| {
        window
            .messages()
            .into_iter()
            .find(|row| row.message.id == root_id)
    };
    let row_of = |window: &mut Window| held_row(window).unwrap();

    // The pin list of a channel without a pin.
    window.click("pins-button");
    window.until("the empty pin list", |window| {
        pins(window) == Some(Vec::new())
    });
    assert!(window.shows("pins-panel") && window.shows("no-pins"));

    // The roles of a new member hold no "manage messages": no pin is offered, and the
    // server refuses one.
    assert!(!row_of(&mut window).can_pin);
    let server_id = window.view().selected.unwrap().id;
    window.in_shell(|shell, _, cx| {
        shell.state().update(cx, |state, cx| {
            state.set_pinned(server_id, root_id, true, cx);
        });
    });
    window.until("the refusal of the pin", |window| {
        window.in_shell(|shell, _, cx| shell.state().read(cx).message_refusal().map(str::to_owned))
            == Some(text::PIN_NOT_ALLOWED.to_owned())
    });

    // Another person pins the message: the open list and the message show the pin.
    world.run(admin.handle.api().pin_message(root_id)).unwrap();
    window.until("the pin in the list", |window| {
        pins(window) == Some(vec![root_id])
    });
    assert!(row_of(&mut window).message.pinned);
    assert!(window.shows_named(format!("pin:{root_id}")));
    assert!(!window.shows("unpin"));

    // With the permission the person takes the pin away from the list.
    admin.allow_everyone(&world, &[WorkspacePermission::ManageMessages]);
    // The change of the roles reloads the state, and the pin list is read again: for
    // that moment the list is not drawn. The person presses "Unpin" when it is there.
    window.until("the permission", |window| {
        held_row(window).is_some_and(|row| row.can_pin)
    });
    window.until("no pin", |window| {
        window.click_if_shown("unpin");
        pins(window) == Some(Vec::new())
    });
    window.until("the message without its pin", |window| {
        held_row(window).is_some_and(|row| !row.message.pinned)
    });
    // And pins the message from its menu.
    window.in_shell(|shell, window, cx| {
        shell.open_message_menu(Pane::Main, root_id, window, cx);
    });
    window.click("message-pin");
    window.until("the pin again", |window| {
        pins(window) == Some(vec![root_id])
    });

    // The list closes with its button, and the client core holds it no longer.
    window.click("close-pins");
    assert!(!window.shows("pins-panel"));
    assert!(
        world
            .run(admin.handle.api().pins(channel_id))
            .unwrap()
            .iter()
            .any(|pin| pin.id == root_id)
    );
}

#[gpui::test]
fn going_to_a_pin_shows_the_message_and_a_pinned_reply_in_its_thread(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _channel_id, root_id) = with_root(cx, &world);
    let api = admin.handle.api();
    // More replies than the first page of a thread: the oldest one is not read when the
    // thread opens.
    let first = admin.reply(&world, root_id, document("the oldest reply"));
    for number in 0..60 {
        admin.reply(&world, root_id, document(&format!("reply {number}")));
    }
    let pinned = |window: &mut Window| -> Option<Vec<MessageId>> {
        let view = window.view();
        match &view.workspace()?.pins.as_ref()?.state {
            PinsState::Loaded(rows) => Some(rows.iter().map(|row| row.message.id).collect()),
            PinsState::Loading | PinsState::Failed => None,
        }
    };

    // A pinned message of the channel: the list marks it.
    world.run(api.pin_message(root_id)).unwrap();
    window.click("pins-button");
    window.until("the pin of the message", |window| {
        pinned(window) == Some(vec![root_id])
    });
    window.click("go-to-pin");
    window.until("the message of the pin is marked", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == root_id && row.target)
    });

    // A pinned reply: its thread opens at the reply, which the list of the thread holds,
    // draws and marks.
    world.run(api.unpin_message(root_id)).unwrap();
    world.run(api.pin_message(first)).unwrap();
    window.until("the pin of the reply", |window| {
        pinned(window) == Some(vec![first])
    });
    window.click("go-to-pin");
    window.until("the pinned reply is shown in its thread", |window| {
        let view = window.view();
        let marked = view
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .filter(|thread| thread.root_id == root_id)
            .and_then(|thread| thread.feed.as_ref())
            .is_some_and(|feed| {
                feed.rows.iter().any(|row| {
                    matches!(row, FeedRow::Message(row) if row.message.id == first && row.target)
                })
            });
        marked && window.shows_named(format!("message:{first}"))
    });
    assert!(!window.shows("pins-panel"));
}

#[gpui::test]
fn a_person_writes_to_another_person_and_a_channel_marks_permanent_messages(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, _directory, mut window, channel_id, root_id) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    let directs = |window: &mut Window| {
        window
            .view()
            .lists()
            .map(|lists| lists.directs.clone())
            .unwrap_or_default()
    };
    assert!(directs(&mut window).is_empty());

    // The members panel starts a conversation with a member.
    window.click("members-button");
    window.until("the members", |window| window.shows_member("Ada"));
    window.click_named(format!("message-member:{ada}"));
    window.until("the conversation is shown", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
    });
    let conversation = window.view().lists().unwrap().conversation.clone().unwrap();
    assert_eq!(conversation.account_id, ada);
    assert_eq!(conversation.name.as_deref(), Some("Ada"));
    assert!(window.view().workspace().unwrap().channel.is_none());
    assert!(window.shows_named(format!("direct-row:{}", conversation.id)));
    window.until("the empty conversation", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });

    // The same list and composer: a message goes to the other person.
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "Hello, directly", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.press("enter");
    window.until_message("Hello, directly");
    let direct = PlaceId::Direct(conversation.id);
    let received = world
        .run(admin.handle.api().messages(direct, &Default::default()))
        .unwrap();
    assert_eq!(received.messages.len(), 1);
    // A message of a direct conversation has no thread and no pin.
    let row = window.messages().remove(0);
    assert!(!row.can_thread && !row.can_pin);
    assert!(!window.shows("pins-button"));

    // The person goes back to the channel; an answer of the other person is a sign on
    // the row of the conversation.
    window.click_channel("general");
    window.until("the channel again", |window| {
        let view = window.view();
        view.lists()
            .is_some_and(|lists| lists.conversation.is_none())
            && view
                .workspace()
                .is_some_and(|workspace| workspace.channel.is_some())
    });
    let answer = SendMessage {
        document: document("Hello back"),
        reply_to: None,
        attachments: Vec::new(),
    };
    world
        .run(admin.handle.api().send_message(direct, &answer))
        .unwrap();
    window.until("the unread conversation", |window| {
        directs(window)
            .iter()
            .any(|row| row.unread.messages == 1 && row.unread.sign)
    });
    window.click_named(format!("direct-row:{}", conversation.id));
    window.until_message("Hello back");

    // A channel with permanent messages says so in its header and in its composer.
    assert!(!window.shows("permanent-marker"));
    window.click_channel("general");
    world
        .run(admin.handle.api().update_channel(
            channel_id,
            &junto_client_core::protocol::structure::UpdateChannel {
                permanent_messages: Some(true),
                ..Default::default()
            },
        ))
        .unwrap();
    window.until("the marker of permanent messages", |window| {
        window.shows("permanent-marker")
    });
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "We approve the scope.", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.press("enter");
    let permanent = window.until_message("We approve the scope.");
    assert!(permanent.message.permanent && !permanent.can_edit && !permanent.can_delete);
    // The composer of a thread of that channel says so as well.
    assert!(!window.shows("composer-permanent"));
    window.open_thread_of(root_id);
    window.until("the statement in the composer of the thread", |window| {
        window.shows("composer-permanent")
    });
}

#[gpui::test]
fn a_mention_in_a_direct_conversation_offers_the_other_person_only(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _channel_id, _root_id) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    let api = admin.handle.api();
    let workspace_id = world.run(api.workspaces()).unwrap()[0].id;
    let role = CreateWorkspaceRole {
        name: "Editors".to_owned(),
        permissions: Vec::new(),
    };
    world
        .run(api.create_workspace_role(workspace_id, &role))
        .unwrap();
    let names =
        |window: &mut Window| window.in_shell(|shell, _, _| shell.mention_names(Pane::Main));
    let type_text = |window: &mut Window, text: &str| {
        window.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, text, window, cx);
            shell.focus_composer(Pane::Main, window, cx);
        });
    };

    // In a channel the picker offers the members and the roles of the workspace.
    window.until("the role is in the workspace", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.roles.iter().any(|(_, name)| name == "Editors"))
    });
    type_text(&mut window, "@");
    let offered = names(&mut window);
    assert!(offered.iter().any(|name| name == "Ada"), "{offered:?}");
    assert!(offered.iter().any(|name| name == "Editors"), "{offered:?}");
    assert!(window.shows("mention-picker"));
    type_text(&mut window, "");

    // In a direct conversation it offers the other person only: the server takes no
    // other mention there (ADR 0408).
    window.click("members-button");
    window.until("the members", |window| window.shows_member("Ada"));
    window.click_named(format!("message-member:{ada}"));
    window.until("the empty conversation", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
            && window.feed().is_some_and(|feed| feed.loaded)
    });
    let conversation = window.view().lists().unwrap().conversation.clone().unwrap();
    type_text(&mut window, "@");
    assert_eq!(names(&mut window), vec!["Ada".to_owned()]);
    type_text(&mut window, "@Edi");
    assert!(names(&mut window).is_empty() && !window.shows("mention-picker"));

    // The entry of the picker takes the mention, and the server takes the message.
    type_text(&mut window, "@");
    window.click_named("candidate:0".to_owned());
    window.press("enter");
    let names_ada = |document: &Document| {
        document.blocks.iter().any(|block| {
            matches!(block, Block::Paragraph { content }
                if content.contains(&Inline::Account { account_id: ada }))
        })
    };
    window.until("the message with the mention is sent", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.document.as_ref().is_some_and(names_ada))
    });
    let sent = world
        .run(api.messages(PlaceId::Direct(conversation.id), &Default::default()))
        .unwrap();
    assert_eq!(sent.messages.len(), 1);
    assert!(sent.messages[0].document.as_ref().is_some_and(names_ada));
}

#[gpui::test]
fn a_late_answer_for_a_conversation_does_not_replace_what_the_person_selected(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _, _) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    let server_id = window.view().selected.unwrap().id;
    let workspace_id = window.view().workspace().unwrap().id;
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    // The person asks for the conversation and selects a channel before the answer.
    window.in_shell(|shell, _, cx| {
        shell.state().update(cx, |state, cx| {
            state.open_conversation(server_id, ada, cx);
            state.select_channel(workspace_id, voice.id, cx);
        });
    });
    window.until("the conversation is in the state", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| !lists.directs.is_empty())
    });
    window.until("the answer of the server was taken", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation_asked.is_none())
    });
    let view = window.view();
    assert!(view.lists().unwrap().conversation.is_none());
    let workspace = view.workspace().unwrap();
    assert_eq!(workspace.channel.as_ref().unwrap().id, voice.id);
}

#[gpui::test]
fn a_late_answer_for_a_conversation_is_not_followed_after_the_person_went_away_and_back(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, _directory, mut window, text, _) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    let server_id = window.view().selected.unwrap().id;
    let workspace_id = window.view().workspace().unwrap().id;
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    // The person asks for the conversation, selects another channel, and selects the
    // first channel again before the answer.
    window.in_shell(|shell, _, cx| {
        shell.state().update(cx, |state, cx| {
            state.open_conversation(server_id, ada, cx);
            state.select_channel(workspace_id, voice.id, cx);
            state.select_channel(workspace_id, text, cx);
        });
    });
    window.until("the conversation is in the state", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| !lists.directs.is_empty())
    });
    window.until("the answer of the server was taken", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation_asked.is_none())
    });
    let view = window.view();
    assert!(view.lists().unwrap().conversation.is_none());
    let workspace = view.workspace().unwrap();
    assert_eq!(workspace.channel.as_ref().unwrap().id, text);
}

#[gpui::test]
fn a_person_follows_a_thread_and_stops_following_it(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, channel_id, root_id) = with_root(cx, &world);
    let server_id = window.view().selected.unwrap().id;
    let follow = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.follow.clone())
    };
    // The row of the thread under its channel in the channel column.
    let entry = |window: &mut Window| {
        window
            .channels()
            .into_iter()
            .flat_map(|row| row.threads)
            .find(|entry| entry.root_id == root_id)
    };

    // A member who did not write in the thread and was not named in it does not follow
    // it, and made no choice.
    window.open_thread_of(root_id);
    window.until("the follow state is read", |window| {
        follow(window).is_some()
    });
    let control = follow(&mut window).unwrap();
    assert_eq!((control.following, control.setting), (false, None));
    assert_eq!(
        text::followed::state(control.following, control.setting),
        "You do not follow this thread."
    );
    assert_eq!(text::followed::action(control.following), "Follow");
    assert!(window.shows("thread-follow-state") && window.shows("thread-follow"));
    assert!(!window.shows("thread-follow-default") && !window.shows("thread-follow-refused"));

    // The member presses "Follow": the server stores the choice.
    window.click("thread-follow");
    window.until("the choice to follow", |window| {
        follow(window).is_some_and(|control| {
            control.following && control.setting == Some(true) && !control.busy
        })
    });
    let control = follow(&mut window).unwrap();
    assert_eq!(
        text::followed::state(control.following, control.setting),
        "You follow this thread."
    );
    assert_eq!(control.refused, None);
    assert!(window.shows("thread-follow-default"));

    // The panel is closed. A thread without a reply is in no list.
    window.click("close-thread");
    assert!(!window.shows("thread-panel"));
    assert_eq!(entry(&mut window), None);

    // Another person answers: the thread is listed under its channel with what is new.
    admin.reply(&world, root_id, document("One per speaker."));
    window.until("the unread thread under its channel", |window| {
        entry(window).is_some_and(|entry| {
            entry.unread == 1 && entry.title.as_deref() == Some("Tracks or mixdown?")
        })
    });
    assert!(window.shows_named(format!("thread-row:{root_id}")));

    // The member opens the thread and reads the reply.
    window.click_named(format!("thread-row:{root_id}"));
    window.until("the reply is drawn", |window| {
        window.replies() == ["One per speaker."]
    });
    let place = PlaceId::Thread {
        channel_id,
        root_id,
    };
    window.until("the reply is read", |window| {
        // The read position follows the end of a list in an active window. The window
        // of a test need not be the active one: the position is moved as the list
        // moves it.
        let end = window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref())
            .and_then(|feed| feed.end_position);
        if let Some(position) = end {
            window.in_shell(|shell, _, cx| {
                shell.state().update(cx, |state, _| {
                    state.mark_read(server_id, place, position);
                });
            });
        }
        entry(window).is_some_and(|entry| entry.open && entry.unread == 0)
    });

    // The panel is closed: the thread stays under its channel as a thread that the
    // member follows, without a sign.
    window.click("close-thread");
    assert!(!window.shows("thread-panel"));
    window.until("the followed thread under its channel", |window| {
        entry(window).is_some_and(|entry| {
            !entry.open
                && entry.unread == 0
                && entry.mentions == 0
                && entry.title.as_deref() == Some("Tracks or mixdown?")
        })
    });
    assert!(window.shows_named(format!("thread-row:{root_id}")));

    // The member opens the thread from that row and stops following it.
    window.click_named(format!("thread-row:{root_id}"));
    window.until("the follow state of the followed thread", |window| {
        follow(window).is_some_and(|control| control.following && control.setting == Some(true))
    });
    assert_eq!(text::followed::action(true), "Stop following");
    window.click("thread-follow");
    window.until("the choice not to follow", |window| {
        follow(window).is_some_and(|control| {
            !control.following && control.setting == Some(false) && !control.busy
        })
    });
    let control = follow(&mut window).unwrap();
    assert_eq!(
        text::followed::state(control.following, control.setting),
        text::followed::NOT_FOLLOWING_BY_CHOICE
    );
    window.click("close-thread");
    window.until("the thread is not listed any more", |window| {
        entry(window).is_none()
    });
    assert!(!window.shows_named(format!("thread-row:{root_id}")));

    // A further reply is not unread for the member: no row is added.
    admin.reply(&world, root_id, document("Agreed."));
    let replies = |window: &mut Window| {
        window
            .messages()
            .into_iter()
            .find(|row| row.message.id == root_id)
            .and_then(|row| row.thread)
            .map(|mark| (mark.replies, mark.unread))
    };
    window.until("the second reply is counted on the message", |window| {
        replies(window).is_some_and(|(count, _)| count == 2)
    });
    assert_eq!(replies(&mut window), Some((2, 0)));
    assert_eq!(entry(&mut window), None);
    assert!(!window.shows_named(format!("thread-row:{root_id}")));
}

impl Window {
    /// Opens the direct conversation with a member through the members panel.
    fn open_conversation_with(
        &mut self,
        account_id: junto_client_core::protocol::ids::AccountId,
        name: &str,
    ) -> PlaceId {
        self.click("members-button");
        self.until("the members", |window| window.shows_member(name));
        self.click_named(format!("message-member:{account_id}"));
        self.until("the conversation is shown", |window| {
            window
                .view()
                .lists()
                .is_some_and(|lists| lists.conversation.is_some())
                && window.feed().is_some_and(|feed| feed.loaded)
        });
        let conversation = self.view().lists().unwrap().conversation.clone().unwrap();
        PlaceId::Direct(conversation.id)
    }
}

#[gpui::test]
fn in_a_direct_conversation_a_person_replies_edits_reacts_and_sends_a_file(
    cx: &mut TestAppContext,
) {
    use junto_client_core::transfers::UploadStage;
    let world = World::new();
    let (_server, admin, _directory, mut window, _channel_id, _root_id) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    let api = admin.handle.api();
    let direct = window.open_conversation_with(ada, "Ada");
    let question = SendMessage {
        document: document("Can you send the mix?"),
        reply_to: None,
        attachments: Vec::new(),
    };
    let question = world.run(api.send_message(direct, &question)).unwrap().id;
    window.until_message("Can you send the mix?");

    // Reply at the bar of the message, with a file chosen in the file dialog.
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("mix.flac");
    std::fs::write(&path, b"the mix of the second take").unwrap();
    window.hover_message(question);
    window.click_named(format!("reply:{question}"));
    assert!(window.shows("compose-state"));
    window.click("attach-file");
    let chosen = path.clone();
    window
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![chosen]));
    window.until("the file is uploaded", |window| {
        window.view().lists().is_some_and(|lists| {
            lists.draft.len() == 1
                && matches!(lists.draft[0].progress.stage, UploadStage::Complete(_))
        })
    });
    window.write("Here it is");
    let answer = window.until_message("Here it is");
    assert_eq!(answer.message.reply_to, Some(question));
    assert_eq!(answer.message.attachments.len(), 1);
    assert!(window.shows_named("file:0".to_owned()));
    let held = world.run(api.message(answer.message.id)).unwrap();
    assert_eq!(held.reply_to, Some(question));
    assert_eq!(held.attachments[0].name, "mix.flac");
    assert_eq!(held.attachments[0].bytes, 26);

    // Edit at the bar of the own message.
    window.hover_message(answer.message.id);
    window.click_named(format!("edit:{}", answer.message.id));
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, Some(answer.message.id))
    );
    window.write("Here it is, the second take");
    let edited = window.until_message("Here it is, the second take");
    assert_eq!(edited.message.id, answer.message.id);
    let held = world.run(api.message(answer.message.id)).unwrap();
    assert!(held.edited_at.is_some());
    assert_eq!(
        held.document
            .as_ref()
            .map(|document| plain_text(document, &Default::default(), 200)),
        Some("Here it is, the second take".to_owned())
    );

    // A reaction at the bar of the message of the other person.
    window.hover_message(question);
    window.click_named(format!("react:{question}"));
    assert!(window.shows("emoji-picker"));
    window.in_shell(|shell, window, cx| shell.find_emoji("thumbs up", window, cx));
    window.click_named("emoji:\u{1f44d}".to_owned());
    window.until("the reaction is drawn", |window| {
        window.messages().iter().any(|row| {
            row.message.id == question
                && row
                    .message
                    .reactions
                    .iter()
                    .any(|reaction| reaction.emoji == "\u{1f44d}" && reaction.own)
        })
    });
    let held = world.run(api.message(question)).unwrap();
    assert!(
        held.reactions
            .iter()
            .any(|reaction| reaction.emoji == "\u{1f44d}" && reaction.count == 1)
    );
}

#[gpui::test]
fn a_person_sends_a_mention_of_a_role_and_of_everyone(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, channel_id, _root_id) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    let api = admin.handle.api();
    let workspace_id = world.run(api.workspaces()).unwrap()[0].id;
    let role = CreateWorkspaceRole {
        name: "Editors".to_owned(),
        permissions: Vec::new(),
    };
    let editors = world
        .run(api.create_workspace_role(workspace_id, &role))
        .unwrap()
        .id;
    world
        .run(api.assign_workspace_role(workspace_id, ada, editors))
        .unwrap();
    admin.allow_everyone(&world, &[WorkspacePermission::MentionEveryone]);
    window.until("the role and the mention of everyone are known", |window| {
        window.view().workspace().is_some_and(|workspace| {
            workspace.roles.iter().any(|(_, name)| name == "Editors")
                && workspace.allowed.mention_everyone
        })
    });
    let type_text = |window: &mut Window, text: &str| {
        window.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, text, window, cx);
            shell.focus_composer(Pane::Main, window, cx);
        });
    };
    let candidate = |window: &mut Window, name: &str| {
        let names = window.in_shell(|shell, _, _| shell.mention_names(Pane::Main));
        let index = names
            .iter()
            .position(|held| held == name)
            .unwrap_or_else(|| panic!("the picker does not offer {name}: {names:?}"));
        window.click_named(format!("candidate:{index}"));
    };

    // The role: typed, chosen in the picker, sent.
    let holds = |document: &Document, node: &Inline| {
        document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Paragraph { content } if content.contains(node)))
    };
    let sent_with = |window: &mut Window, node: &Inline| {
        window.until_value("the message with the mention is drawn", |window| {
            window.messages().into_iter().find(|row| {
                row.message
                    .document
                    .as_ref()
                    .is_some_and(|document| holds(document, node))
            })
        })
    };
    // The words are put in, and the mention is typed key by key, so that the picker
    // opens through the input of the text area.
    type_text(&mut window, "Review please, ");
    window.cx.simulate_input("@Edi");
    candidate(&mut window, "Editors");
    window.press("enter");
    let role_node = Inline::Role { role_id: editors };
    let to_role = sent_with(&mut window, &role_node);
    let drawn = to_role.message.document.as_ref().unwrap();
    let names = window.feed().unwrap().names;
    assert_eq!(
        plain_text(drawn, &names, 200).trim_end(),
        "Review please, @Editors"
    );
    let held = world.run(api.message(to_role.message.id)).unwrap();
    assert!(
        held.document
            .as_ref()
            .is_some_and(|document| holds(document, &role_node))
    );

    // Everyone: chosen in the picker, sent.
    type_text(&mut window, "Meeting at noon, ");
    window.cx.simulate_input("@every");
    candidate(&mut window, text::EVERYONE);
    window.press("enter");
    let to_all = sent_with(&mut window, &Inline::Everyone);
    let held = world.run(api.message(to_all.message.id)).unwrap();
    assert!(
        held.document
            .as_ref()
            .is_some_and(|document| holds(document, &Inline::Everyone))
    );

    // Both mentions reach the administrator, who holds the role and is one of everyone.
    let state = world
        .run(api.message_state(PlaceId::Channel(channel_id)))
        .unwrap();
    assert_eq!(state.mentions, 2, "{state:?}");
}
