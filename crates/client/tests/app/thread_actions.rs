//! The actions of a thread, of pins and of the permanence of a channel that a person takes
//! in the window against the real server (ADR 0741, ADR 0796): the default following of a
//! thread, the followed threads after the first eight, a fork into a voice channel, an
//! edit, a reaction and a file in a thread, the pin of a permanent message, the limit of
//! the pins of a channel, and permanent messages turned on and off.

use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client::state::feed::FeedRow;
use junto_client::state::view::PinsState;
use junto_client::text;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, MessageId};
use junto_client_core::protocol::message::{Message, SendMessage};
use junto_client_core::protocol::structure::{ChannelKind, UpdateChannel};
use junto_client_core::transfers::UploadStage;

use crate::harness::{Window, World, named, selector};
use crate::messages::document;
use crate::threads::with_root;

impl Window {
    /// The follow state of the thread of the side panel: whether the person follows it and
    /// the choice of the person, once no change is on its way.
    fn follow_state(&mut self) -> Option<(bool, Option<bool>)> {
        self.view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.follow.clone())
            .filter(|control| !control.busy)
            .map(|control| (control.following, control.setting))
    }

    /// The roots of the threads that the channel column lists under a channel, and whether
    /// it offers the further followed threads.
    fn threads_under(&mut self, channel_id: ChannelId) -> (Vec<MessageId>, bool) {
        self.channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .map(|row| {
                (
                    row.threads.iter().map(|entry| entry.root_id).collect(),
                    row.more_followed.is_some(),
                )
            })
            .unwrap_or_default()
    }

    /// The messages of the open pin list.
    fn pinned(&mut self) -> Option<Vec<MessageId>> {
        let view = self.view();
        match &view.workspace()?.pins.as_ref()?.state {
            PinsState::Loaded(rows) => Some(rows.iter().map(|row| row.message.id).collect()),
            PinsState::Loading | PinsState::Failed => None,
        }
    }

    /// The rows of the thread of the side panel.
    fn thread_rows(&mut self) -> Vec<FeedRow> {
        self.view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref())
            .map(|feed| feed.rows.to_vec())
            .unwrap_or_default()
    }

    /// The message of the thread of the side panel with this identifier.
    fn thread_message(
        &mut self,
        message_id: MessageId,
    ) -> Option<junto_client::state::feed::MessageRow> {
        self.thread_rows().into_iter().find_map(|row| match row {
            FeedRow::Message(row) if row.message.id == message_id => Some(row),
            _ => None,
        })
    }

    /// Opens the thread of a message of the channel from the bar of the message, as a person
    /// does with the pointer.
    fn open_thread_by_pointer(&mut self, root_id: MessageId) {
        self.act_on(root_id, "thread");
        self.until("the thread is read", |window| {
            window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| {
                    thread.root_id == root_id
                        && thread.root.is_some()
                        && thread.feed.as_ref().is_some_and(|feed| feed.loaded)
                })
        });
    }

    /// Types a reply into the composer of the thread with the keys and sends it with Enter.
    fn type_reply(&mut self, text: &str) {
        self.type_into("thread-composer-input", text);
        self.press("enter");
    }

    /// The number of replies that the row of a message in the channel shows.
    fn replies_shown(&mut self, message_id: MessageId) -> u64 {
        self.messages()
            .into_iter()
            .find(|row| row.message.id == message_id)
            .and_then(|row| row.thread)
            .map_or(0, |mark| mark.replies)
    }

    /// Takes one action of the bar of a message: the pointer goes to the message, and the
    /// action is clicked.
    fn act_on(&mut self, message_id: MessageId, action: &str) {
        self.hover_named(format!("message:{message_id}"));
        self.click_named(format!("{action}:{message_id}"));
    }
}

#[gpui::test]
fn use_the_default_gives_the_following_of_a_thread_back_to_the_rule_of_the_server(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, _admin, _directory, mut window, _, root_id) = with_root(cx, &world);
    let api = window.handle().api();
    let stored = |world: &World| {
        let state = world.run(api.thread_state(root_id)).unwrap();
        (state.following, state.follow_setting)
    };

    // A member who did not write in the thread does not follow it by the rule.
    window.open_thread_by_pointer(root_id);
    window.until("the follow state is read", |window| {
        window.follow_state() == Some((false, None))
    });
    assert!(!window.shows("thread-follow-default"));

    // The member follows by choice. "Use the default" removes the choice: the rule
    // decides again, and the member follows the thread no longer.
    window.click("thread-follow");
    window.until("the choice to follow", |window| {
        window.follow_state() == Some((true, Some(true)))
    });
    window.click("thread-follow-default");
    window.until("the choice is removed", |window| {
        window.follow_state() == Some((false, None))
    });
    assert_eq!(stored(&world), (false, None));
    assert!(!window.shows("thread-follow-default"));

    // The member writes in the thread and follows it by the rule, then chooses not to
    // follow it. "Use the default" gives the thread back to the rule: the member follows
    // it again.
    window.type_reply("One per speaker.");
    window.until("the member follows by the rule", |window| {
        window.follow_state() == Some((true, None))
    });
    window.click("thread-follow");
    window.until("the choice not to follow", |window| {
        window.follow_state() == Some((false, Some(false)))
    });
    assert_eq!(stored(&world), (false, Some(false)));
    window.click("thread-follow-default");
    window.until("the rule decides again", |window| {
        window.follow_state() == Some((true, None))
    });
    assert_eq!(stored(&world), (true, None));
    assert_eq!(
        window
            .view()
            .workspace()
            .unwrap()
            .thread
            .as_ref()
            .unwrap()
            .root_id,
        root_id
    );
}

#[gpui::test]
fn the_followed_threads_after_the_first_eight_are_listed_when_the_person_asks(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general, _) = with_root(cx, &world);
    let api = window.handle().api();
    // The member writes in a thread and follows it by the rule; an own reply is not
    // unread, so the thread is listed as a followed thread without a sign.
    let follow = |world: &World, count: usize, from: usize| -> Vec<MessageId> {
        (from..from + count)
            .map(|number| {
                let root = admin.send(world, general, document(&format!("Topic {number}")));
                let reply = SendMessage {
                    document: document(&format!("Answer {number}")),
                    reply_to: None,
                    attachments: Vec::new(),
                };
                world.run(api.reply_in_thread(root.id, &reply)).unwrap();
                root.id
            })
            .collect()
    };

    // Ten followed threads: the channel lists eight and offers the other two.
    let first = follow(&world, 10, 0);
    window.until("eight followed threads and the offer", |window| {
        let (listed, more) = window.threads_under(general);
        listed.len() == 8 && more
    });
    let more = window
        .channels()
        .into_iter()
        .find(|row| row.id == general)
        .and_then(|row| row.more_followed)
        .unwrap();
    assert_eq!(more.count, Some(2));
    window.click_named(format!("followed-more:{general}"));
    window.until("the ten followed threads", |window| {
        let (listed, more) = window.threads_under(general);
        listed.len() == 10 && !more
    });
    let (listed, _) = window.threads_under(general);
    assert!(first.iter().all(|root| listed.contains(root)), "{listed:?}");
    assert!(!window.shows_named(format!("followed-more:{general}")));

    // More followed threads than the server gives in a page: the window lists the threads
    // of the page that it read and offers the rest without a count.
    let later = follow(&world, 45, 10);
    window.until("a page of followed threads and the offer", |window| {
        let (listed, more) = window.threads_under(general);
        listed.len() == 50 && more
    });
    let more = window
        .channels()
        .into_iter()
        .find(|row| row.id == general)
        .and_then(|row| row.more_followed)
        .unwrap();
    assert_eq!(more.count, None);
    // The offer is at the end of the long list. The press reads the next page from the
    // server.
    let offer = selector(format!("followed-more:{general}"));
    window.wheel_to("channel-list", offer);
    window.click(offer);
    window.until("every followed thread", |window| {
        let (listed, more) = window.threads_under(general);
        listed.len() == 55 && !more
    });
    let (listed, _) = window.threads_under(general);
    assert!(
        first.iter().chain(&later).all(|root| listed.contains(root)),
        "{listed:?}"
    );
    let stored = world
        .run(api.followed_threads(general, None, Some(200)))
        .unwrap();
    assert_eq!(stored.threads.len(), 55);
}

#[gpui::test]
fn a_thread_is_forked_into_a_voice_channel(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _, root_id) = with_root(cx, &world);
    admin.reply(&world, root_id, document("One per speaker."));
    // The reply changes the row of its root. The pointer acts on the row as it is drawn
    // with the reply, as a person sees it; a row that changes between the press and the
    // release takes no click (see the test after this one).
    window.until("the reply is shown on its root", |window| {
        window.replies_shown(root_id) == 1
    });
    window.open_thread_by_pointer(root_id);
    admin.allow_everyone(&world, &[WorkspacePermission::ManageChannel]);
    window.until("the permission to fork", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| thread.may_fork)
    });

    // "Voice" opens the dialog for a voice channel, and the fork is made.
    window.click("fork-voice");
    assert!(window.shows("fork-dialog"));
    window.type_into("fork-name", "listening room");
    window.click("create-fork");
    window.until("the voice channel of the fork", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.name == "listening room" && row.kind == ChannelKind::Voice)
    });
    window.until("the thread names its voice fork", |window| {
        !window.shows("fork-dialog")
            && window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| {
                    thread.fork.is_none()
                        && thread.forks.len() == 1
                        && thread.forks[0].1 == "listening room"
                        && thread.forks[0].2 == ChannelKind::Voice
                })
    });
    let fork = window
        .channels()
        .into_iter()
        .find(|row| row.name == "listening room")
        .unwrap();
    let workspace_id = world.run(admin.handle.api().workspaces()).unwrap()[0].id;
    let listed = world
        .run(admin.handle.api().channels(workspace_id))
        .unwrap();
    assert!(
        listed
            .channels
            .iter()
            .any(|channel| channel.id == fork.id && channel.kind == ChannelKind::Voice)
    );

    // The link of the thread shows the chat of the voice channel with the copies.
    assert!(window.shows("fork-link") && !window.shows("fork-voice"));
    window.click("fork-link");
    window.until("the copies in the chat of the voice channel", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.place == PlaceId::Channel(fork.id))
            && window.texts() == ["Tracks or mixdown?", "One per speaker."]
    });
}

/// A reply that is drawn while the pointer is pressed on the thread button of its root moves
/// the button: the release lands outside it, and the thread does not open. The test of the
/// fork above failed so on macOS when the reply event arrived between its press and its
/// release; this test makes that order on every system.
#[gpui::test]
fn a_reply_drawn_between_the_press_and_the_release_moves_the_thread_button(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
    let world = World::new();
    let (_server, admin, _directory, mut window, _, root_id) = with_root(cx, &world);
    window.hover_named(format!("message:{root_id}"));
    let button = selector(format!("thread:{root_id}"));
    let pressed = window
        .cx
        .debug_bounds(button)
        .expect("the bar of the root is drawn");
    window.dispatch(MouseDownEvent {
        position: pressed.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    admin.reply(&world, root_id, document("One per speaker."));
    window.until("the reply is shown on its root", |window| {
        window.replies_shown(root_id) == 1
    });
    let moved = window.cx.debug_bounds(button);
    window.dispatch(MouseUpEvent {
        position: pressed.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    window.settle();
    assert_ne!(moved, Some(pressed), "the thread button stayed in place");
    assert!(
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.thread.is_none()),
        "the thread opened from a press and a release on two places"
    );
}

#[gpui::test]
fn a_person_edits_reacts_to_and_attaches_a_file_to_a_reply_in_a_thread(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _, root_id) = with_root(cx, &world);
    let api = admin.handle.api();
    window.open_thread_by_pointer(root_id);
    window.type_reply("One per speaker");
    let reply = window.until_value("the reply is drawn in the thread", |window| {
        window.thread_rows().into_iter().find_map(|row| match row {
            FeedRow::Message(row) if row.own => Some(row.message.id),
            _ => None,
        })
    });

    // The bar of the reply edits it in the composer of the thread.
    window.act_on(reply, "edit");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Thread)),
        (None, Some(reply))
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        "One per speaker"
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
    // The caret is at the end of the text: the person adds to it, and Enter saves.
    window.type_into("thread-composer-input", " and one mix");
    window.press("enter");
    window.until("the edited reply", |window| {
        window.replies() == ["One per speaker and one mix"]
            && window
                .thread_message(reply)
                .is_some_and(|row| row.message.edited_at.is_some())
    });
    let stored: Message = world.run(api.message(reply)).unwrap();
    assert_eq!(
        stored.document,
        Some(document("One per speaker and one mix"))
    );
    assert!(stored.edited_at.is_some());

    // The bar of the reply opens the picker for a reaction to it.
    window.act_on(reply, "react");
    assert!(window.shows("emoji-picker"));
    window.type_into("find-emoji", "thumbs up");
    window.click_named("emoji:\u{1f44d}".to_owned());
    window.until("the reaction on the reply", |window| {
        window.thread_message(reply).is_some_and(|row| {
            let held = &row.message.reactions;
            held.len() == 1 && held[0].emoji == "\u{1f44d}" && held[0].own
        })
    });
    let stored: Message = world.run(api.message(reply)).unwrap();
    assert_eq!(stored.reactions.len(), 1);
    assert_eq!(stored.reactions[0].count, 1);
    // Its sign takes the reaction away.
    window.click_named(format!("reaction:{reply}:0"));
    window.until("the reaction is gone", |window| {
        window
            .thread_message(reply)
            .is_some_and(|row| row.message.reactions.is_empty())
    });
    let stored: Message = world.run(api.message(reply)).unwrap();
    assert!(stored.reactions.is_empty());

    // A file dropped on the composer of the thread goes with the next reply, which stays
    // in the thread.
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("stems.txt");
    std::fs::write(&path, b"vocals, drums, bass").unwrap();
    window.drop_files("thread-composer", vec![path]);
    window.until("the file is uploaded for the thread", |window| {
        window.view().workspace().is_some_and(|workspace| {
            workspace.thread.as_ref().is_some_and(|thread| {
                thread.draft.len() == 1
                    && thread.draft[0].name == "stems.txt"
                    && matches!(thread.draft[0].progress.stage, UploadStage::Complete(_))
            })
        })
    });
    assert!(window.view().lists().unwrap().draft.is_empty());
    window.type_reply("The stems");
    let sent = window.until_value("the reply with the file", |window| {
        window.thread_rows().into_iter().find_map(|row| match row {
            FeedRow::Message(row)
                if row.message.attachments.len() == 1
                    && row.message.attachments[0].name == "stems.txt" =>
            {
                Some(row.message.id)
            }
            _ => None,
        })
    });
    assert_eq!(window.texts(), ["Tracks or mixdown?"]);
    let stored: Message = world.run(api.message(sent)).unwrap();
    assert_eq!(stored.thread_root, Some(root_id));
    assert_eq!(stored.attachments.len(), 1);
    assert_eq!(stored.attachments[0].bytes, 19);
}

#[gpui::test]
fn a_moderator_pins_and_unpins_permanent_messages(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general, root_id) = with_root(cx, &world);
    let api = admin.handle.api();
    admin.allow_everyone(&world, &[WorkspacePermission::ManageMessages]);
    world
        .run(api.update_channel(
            general,
            &UpdateChannel {
                permanent_messages: Some(true),
                ..UpdateChannel::default()
            },
        ))
        .unwrap();
    let decision = admin.send(&world, general, document("We approve the scope."));
    let reply = admin.reply(&world, root_id, document("Tracks, one per speaker."));
    let row = window.until_message("We approve the scope.");
    assert!(row.message.permanent);
    window.until("the pin is offered", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == decision.id && row.can_pin)
    });
    let pins = |world: &World| -> Vec<MessageId> {
        world
            .run(api.pins(general))
            .unwrap()
            .into_iter()
            .map(|pin| pin.id)
            .collect()
    };

    // The bar of a permanent message pins it, as it pins any message.
    window.act_on(decision.id, "pin");
    window.until("the permanent message is pinned", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == decision.id && row.message.pinned)
    });
    assert_eq!(pins(&world), [decision.id]);

    // A permanent reply in a thread is pinned from its bar in the side panel.
    window.open_thread_by_pointer(root_id);
    window.until("the permanent reply", |window| {
        window
            .thread_message(reply)
            .is_some_and(|row| row.message.permanent && row.can_pin)
    });
    window.act_on(reply, "pin");
    window.until("the permanent reply is pinned", |window| {
        window
            .thread_message(reply)
            .is_some_and(|row| row.message.pinned)
    });
    assert_eq!(pins(&world), [decision.id, reply]);
    window.click("close-thread");

    // The pin list holds both.
    window.click("pins-button");
    window.until("both pins in the list", |window| {
        window.pinned() == Some(vec![decision.id, reply])
    });
    window.click("close-pins");

    // The bar takes the pin of a permanent message away, and the message stays.
    window.act_on(decision.id, "pin");
    window.until("the pin is taken away", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == decision.id && !row.message.pinned)
    });
    assert_eq!(pins(&world), [reply]);
    let stored: Message = world.run(api.message(decision.id)).unwrap();
    assert!(stored.permanent && !stored.pinned);
}

#[gpui::test]
fn the_pin_after_the_hundredth_of_a_channel_is_refused_with_its_reason(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general, root) = with_root(cx, &world);
    admin.allow_everyone(&world, &[WorkspacePermission::ManageMessages]);
    let messages: Vec<MessageId> = (0..100)
        .map(|number| {
            admin
                .send(&world, general, document(&format!("Point {number}")))
                .id
        })
        .collect();
    window.until_message("Point 99");
    // The member pins the root of the scene and 98 points: two messages at the end of the
    // list are left for the window.
    let own = window.handle().api();
    for message_id in std::iter::once(&root).chain(&messages[..98]) {
        world.run(own.pin_message(*message_id)).unwrap();
    }
    let (hundredth, refused) = (messages[98], messages[99]);
    window.until("the pin is offered at the end of the list", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == refused && row.can_pin)
    });
    let pinned_at = |window: &mut Window, message_id: MessageId| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message_id && row.message.pinned)
    };

    // The 100th pin is taken.
    window.act_on(hundredth, "pin");
    window.until("the 100th pin", |window| pinned_at(window, hundredth));
    assert!(!window.shows("compose-problem"));

    // The 101st is refused, and the composer tells why.
    window.act_on(refused, "pin");
    window.until("the refusal of the 101st pin", |window| {
        window.in_shell(|shell, _, cx| shell.state().read(cx).message_refusal().map(str::to_owned))
            == Some(text::PIN_LIMIT.to_owned())
    });
    assert!(window.shows("compose-problem"));
    assert!(!pinned_at(&mut window, refused));
    let stored = world.run(admin.handle.api().pins(general)).unwrap();
    assert_eq!(stored.len(), 100);
    assert!(stored.iter().all(|pin| pin.id != refused));
}

#[gpui::test]
fn an_administrator_turns_permanent_messages_on_and_off_and_the_channel_tells_both(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window) = named(cx, "Ada");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let api = window.handle().api();
    let permanent = |world: &World| {
        let workspace_id = world.run(api.workspaces()).unwrap()[0].id;
        world
            .run(api.channels(workspace_id))
            .unwrap()
            .channels
            .into_iter()
            .find(|channel| channel.id == general)
            .unwrap()
            .permanent_messages
    };
    let settings = selector(format!("channel-settings-{general}"));
    let switch = |window: &mut Window| {
        window.open_administration();
        window.click("admin-nav-channels");
        window.until("the channel has its row", |window| window.shows(settings));
        window.click(settings);
        window.until("the settings of the channel are open", |window| {
            window.shows("channel-permanent")
        });
        window.click("channel-permanent");
    };

    // The administrator turns permanent messages on, and a message is permanent.
    switch(&mut window);
    window.until("the server holds the setting", |_| permanent(&world));
    window.click("admin-close");
    window.until("the marker of permanent messages", |window| {
        window.shows("permanent-marker")
    });
    window.write("We approve the scope.");
    let kept = window.until_message("We approve the scope.");
    assert!(kept.message.permanent && !kept.can_edit);

    // And turns them off: the channel says so, and a new message can be edited.
    switch(&mut window);
    window.until("the server holds the setting off", |_| !permanent(&world));
    window.click("admin-close");
    window.until("the marker is gone", |window| {
        !window.shows("permanent-marker")
    });
    window.write("Draft for later.");
    let draft = window.until_message("Draft for later.");
    assert!(!draft.message.permanent && draft.can_edit);
    // A message that was sent as permanent stays permanent (ADR 0409).
    let kept = window
        .messages()
        .into_iter()
        .find(|row| row.message.id == kept.message.id)
        .unwrap();
    assert!(kept.message.permanent && !kept.can_edit);

    // The channel holds one line for each change, in its order, with the name of the
    // administrator.
    let lines = window.until_value("the two lines of the setting", |window| {
        let feed = window.feed()?;
        let lines: Vec<(String, String)> = feed
            .rows
            .iter()
            .filter_map(|row| match row {
                FeedRow::System(line) => Some((
                    line.message.system.as_ref()?.action.clone(),
                    line.parts.iter().map(|part| part.text.as_str()).collect(),
                )),
                _ => None,
            })
            .filter(|(action, _)| action.starts_with("permanent_messages_"))
            .collect();
        (lines.len() == 2).then_some(lines)
    });
    assert_eq!(
        lines,
        [
            (
                "permanent_messages_turned_on".to_owned(),
                "Ada turned on permanent messages".to_owned()
            ),
            (
                "permanent_messages_turned_off".to_owned(),
                "Ada turned off permanent messages".to_owned()
            ),
        ]
    );
    let stored = world
        .run(api.messages(PlaceId::Channel(general), &Default::default()))
        .unwrap();
    let actions: Vec<&str> = stored
        .messages
        .iter()
        .filter_map(|message| message.system.as_ref())
        .map(|system| system.action.as_str())
        .filter(|action| action.starts_with("permanent_messages_"))
        .collect();
    assert_eq!(
        actions,
        [
            "permanent_messages_turned_on",
            "permanent_messages_turned_off"
        ]
    );
    for line in feed_system_ids(&mut window) {
        assert!(window.shows_named(format!("system:{line}")));
    }
}

/// The messages of the system lines of permanence that the list of the window draws.
fn feed_system_ids(window: &mut Window) -> Vec<MessageId> {
    window
        .feed()
        .map(|feed| {
            feed.rows
                .iter()
                .filter_map(|row| match row {
                    FeedRow::System(line)
                        if line.message.system.as_ref().is_some_and(|system| {
                            system.action.starts_with("permanent_messages_")
                        }) =>
                    {
                        Some(line.message.id)
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}
