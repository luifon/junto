//! The wheel of the pointer over each kind of list of messages in the active window
//! (ADR 0740): the list moves, and the read position follows the end of the list after
//! the scroll. The list calls the window while it holds its own state, so the window may
//! not read that state there; a test window that is not active never reached that read.

use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use junto_client::shell::Pane;
use junto_client::state::feed::Feed;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;

use crate::calls::{connected_call, show_call, voice_channel};
use crate::harness::{Window, World, member};
use crate::messages::document;

/// The messages in a list: more than the window shows at once.
const LINES: usize = 40;

impl Window {
    fn pane_feed(&mut self, pane: Pane) -> Option<Feed> {
        let view = self.view();
        match pane {
            Pane::Main => view.lists()?.feed.clone(),
            Pane::Thread => view.workspace()?.thread.as_ref()?.feed.clone(),
        }
    }

    fn pane_unread(&mut self, pane: Pane) -> Option<u64> {
        self.pane_feed(pane).map(|feed| feed.unread)
    }

    fn pane_at_end(&mut self, pane: Pane) -> bool {
        self.in_shell(|shell, _, _| shell.feed_is_at_end(pane))
    }

    /// One turn of the wheel over the middle of the list of `pane` by `distance` pixels.
    /// A positive distance moves the list toward its start.
    fn wheel_over(&mut self, pane: Pane, distance: f32) {
        let list = self
            .in_shell(|shell, _, _| shell.feed_bounds(pane))
            .expect("the list was drawn");
        self.dispatch(ScrollWheelEvent {
            position: list.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(distance))),
            ..Default::default()
        });
        self.settle();
    }
}

/// The person reads a list to its end in the active window, scrolls away from the end with
/// the wheel, gets three messages of another person there, and scrolls back to the end with
/// the wheel. `send` sends a message of the other person to the place; `stored` reads the
/// unread messages of the place on the server.
fn scroll_away_and_back(
    window: &mut Window,
    pane: Pane,
    send: &dyn Fn(&str),
    stored: &dyn Fn() -> u64,
) {
    window.cx.update(|window, _| window.activate_window());
    window.settle();
    window.until("the list is read to its end", |window| {
        window.pane_at_end(pane) && window.pane_unread(pane) == Some(0) && stored() == 0
    });

    window.wheel_over(pane, 2000.0);
    assert!(
        !window.pane_at_end(pane),
        "the wheel did not move the list away from its end"
    );

    for number in 0..3 {
        send(&format!("Later {number}"));
    }
    window.until("the later messages are unread", |window| {
        window.pane_unread(pane) == Some(3) && stored() == 3
    });
    assert!(!window.pane_at_end(pane));

    window.until("the wheel brings the list to its end", |window| {
        if !window.pane_at_end(pane) {
            window.wheel_over(pane, -300.0);
        }
        window.pane_at_end(pane)
    });
    window.until("the later messages are read", |window| {
        window.pane_unread(pane) == Some(0) && stored() == 0
    });

    // A few pixels away from the end and back (gpui takes a list within one pixel of its end
    // as at the end). The list takes up the end again only in its layout after the scroll,
    // and the position told to the client core does not change, so only that layout can draw
    // the window without the button to the end.
    let go_to_end = match pane {
        Pane::Main => "go-to-end",
        Pane::Thread => "thread-go-to-end",
    };
    assert!(!window.shows(go_to_end));
    window.wheel_over(pane, 5.0);
    assert!(!window.pane_at_end(pane));
    assert!(window.shows(go_to_end));
    window.wheel_over(pane, -5.0);
    assert!(window.pane_at_end(pane));
    assert!(
        !window.shows(go_to_end),
        "the button to the end is drawn after the list reached its end"
    );
}

#[gpui::test]
fn the_wheel_over_a_channel_in_the_active_window_moves_the_read_position(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    for number in 0..LINES {
        admin.send(&world, general, document(&format!("Line {number}")));
    }
    window.until_message(&format!("Line {}", LINES - 1));
    let own = window.handle();
    scroll_away_and_back(
        &mut window,
        Pane::Main,
        &|text| {
            admin.send(&world, general, document(text));
        },
        &|| {
            world
                .run(own.api().message_state(PlaceId::Channel(general)))
                .unwrap()
                .unread
        },
    );
}

#[gpui::test]
fn the_wheel_over_a_direct_conversation_in_the_active_window_moves_the_read_position(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let ada = admin.handle.account().unwrap();
    window.click("members-button");
    window.until("the members", |window| window.shows_member("Ada"));
    window.click_named(format!("message-member:{ada}"));
    window.until("the conversation is shown", |window| {
        window.view().lists().is_some_and(|lists| {
            lists.conversation.is_some() && lists.feed.as_ref().is_some_and(|feed| feed.loaded)
        })
    });
    let conversation = window.view().lists().unwrap().conversation.clone().unwrap();
    let direct = PlaceId::Direct(conversation.id);
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "Hello, directly", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.press("enter");
    window.until_message("Hello, directly");

    let send = |text: &str| {
        let message = SendMessage {
            document: document(text),
            reply_to: None,
            attachments: Vec::new(),
        };
        world
            .run(admin.handle.api().send_message(direct, &message))
            .unwrap();
    };
    for number in 0..LINES {
        send(&format!("Line {number}"));
    }
    window.until_message(&format!("Line {}", LINES - 1));
    let own = window.handle();
    scroll_away_and_back(&mut window, Pane::Main, &send, &|| {
        world.run(own.api().message_state(direct)).unwrap().unread
    });
}

#[gpui::test]
fn the_wheel_over_a_thread_in_the_active_window_moves_the_read_position(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let root_id = admin
        .send(&world, general, document("Tracks or mixdown?"))
        .id;
    window.until_message("Tracks or mixdown?");
    window.open_thread_of(root_id);
    // The person writes in the thread, so the thread is followed and its replies count as
    // unread (ADR 0412).
    window.write_reply("Tracks, please");
    window.until("the own reply", |window| {
        window
            .replies()
            .iter()
            .any(|reply| reply == "Tracks, please")
    });
    for number in 0..LINES {
        admin.reply(&world, root_id, document(&format!("Reply {number}")));
    }
    let last = format!("Reply {}", LINES - 1);
    window.until("the replies are drawn", |window| {
        window.replies().contains(&last)
    });
    let own = window.handle();
    scroll_away_and_back(
        &mut window,
        Pane::Thread,
        &|text| {
            admin.reply(&world, root_id, document(text));
        },
        &|| world.run(own.api().thread_state(root_id)).unwrap().unread,
    );
}

#[gpui::test]
fn the_wheel_over_the_chat_of_a_call_in_the_active_window_moves_the_read_position(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    window.until("the chat tab of the call", |window| {
        window.shows("call-tab-chat") && window.shows("feed")
    });
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    for number in 0..LINES {
        admin.send(&world, daily, document(&format!("Line {number}")));
    }
    window.until_message(&format!("Line {}", LINES - 1));
    let own = window.handle();
    scroll_away_and_back(
        &mut window,
        Pane::Main,
        &|text| {
            admin.send(&world, daily, document(text));
        },
        &|| {
            world
                .run(own.api().message_state(PlaceId::Channel(daily)))
                .unwrap()
                .unread
        },
    );
}
