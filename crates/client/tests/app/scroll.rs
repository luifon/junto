//! The wheel and the scroll bar over each kind of list of messages in the active window
//! (ADR 0740): the list moves, the next page is read, the client core is told where the list
//! stands, and the read position follows the end of the list after the scroll. The list
//! calls the window while it holds its own state, so the window may not read that state
//! there; a test window that is not active never reached that read.

use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta,
    ScrollWheelEvent, TestAppContext, point, px,
};
use junto_client::shell::Pane;
use junto_client::state::feed::Feed;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::ChannelKind;

use crate::calls::{connected_call, show_call, voice_channel};
use crate::harness::{Window, World, member};
use crate::messages::{core_anchor, document};
use crate::support::{Fault, FaultProxy, TestServer};

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

    pub(crate) fn pane_at_end(&mut self, pane: Pane) -> bool {
        self.in_shell(|shell, _, _| shell.feed_is_at_end(pane))
    }

    /// One turn of the wheel over the middle of the list of `pane` by `distance` pixels.
    /// A positive distance moves the list toward its start.
    pub(crate) fn wheel_over(&mut self, pane: Pane, distance: f32) {
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

    /// Presses the thumb of the scroll bar of the list of `pane` at the end of the list,
    /// moves the pointer with the button held to above the list, and releases it.
    fn drag_scroll_bar_to_top(&mut self, pane: Pane) {
        let list = self
            .in_shell(|shell, _, _| shell.feed_bounds(pane))
            .expect("the list was drawn");
        let x = list.right() - px(5.0);
        let from = point(x, list.bottom() - px(6.0));
        let modifiers = Modifiers::none();
        self.dispatch(MouseMoveEvent {
            position: from,
            pressed_button: None,
            modifiers,
        });
        self.dispatch(MouseDownEvent {
            position: from,
            button: MouseButton::Left,
            modifiers,
            click_count: 1,
            first_mouse: false,
        });
        let mut position = from;
        for _ in 0..10 {
            position.y -= (list.size.height + px(40.0)) / 10.0;
            self.dispatch(MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                modifiers,
            });
        }
        self.dispatch(MouseUpEvent {
            position,
            button: MouseButton::Left,
            modifiers,
            click_count: 1,
        });
        self.settle();
    }

    fn told_anchor(&mut self, place: PlaceId) -> Option<Option<u64>> {
        let server_id = self.view().selected.unwrap().id;
        core_anchor(self, server_id, place)
    }
}

/// The person reads a list to its end in the active window, scrolls away from the end with
/// the wheel, gets three messages of another person there, and scrolls back to the end with
/// the wheel. `send` sends a message of the other person to the place; `stored` reads the
/// unread messages of the place on the server.
fn scroll_away_and_back(
    window: &mut Window,
    pane: Pane,
    place: PlaceId,
    send: &dyn Fn(&str),
    stored: &dyn Fn() -> u64,
) {
    // Each read of the server counts against the history requests of the account, 120 in a
    // minute, as the pages that the window reads do; a read at each step of a wait went past
    // that limit when the wait was long. The server is read at most once a second.
    let last = std::cell::Cell::new(None::<(std::time::Instant, u64)>);
    let stored = || match last.get() {
        Some((at, value)) if at.elapsed() < std::time::Duration::from_secs(1) => value,
        _ => {
            let value = stored();
            last.set(Some((std::time::Instant::now(), value)));
            value
        }
    };
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
    // The client core follows the end, so a reload reads the latest messages.
    window.until(
        "the client core is told that the list stands at its end",
        |window| window.told_anchor(place) == Some(None),
    );

    // A few pixels away from the end and back (gpui takes a list within one pixel of its end
    // as at the end). The list takes up the end again only in its layout after the scroll,
    // so only the work after that layout can draw the window without the button to the end.
    // In the chat of a call the call view also draws the window again by itself, so there
    // this step passes without that work too.
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
        PlaceId::Channel(general),
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
    scroll_away_and_back(&mut window, Pane::Main, direct, &send, &|| {
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
        PlaceId::Thread {
            channel_id: general,
            root_id,
        },
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
        PlaceId::Channel(daily),
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

/// The scroll bar moves the list without the wheel: after the drag the next page is read and
/// the client core is told where the list stands, as after a scroll with the wheel.
#[gpui::test]
fn a_drag_of_the_scroll_bar_to_the_top_reads_the_older_page_and_tells_the_position(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let general = world
        .run(admin.handle.api().snapshot())
        .unwrap()
        .snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.channels)
        .find(|channel| channel.kind == ChannelKind::Text)
        .unwrap()
        .id;
    // Written before the person joins, so the channel opens with its latest page.
    for number in 0..90 {
        admin.send(&world, general, document(&format!("Line {number}")));
    }
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    assert_eq!(window.selected_channel(), general);
    let place = PlaceId::Channel(general);
    window.until_message("Line 89");
    window.settle();
    let first_page = window.texts();
    assert!(
        window.feed().unwrap().more_before,
        "the first page holds the whole history"
    );
    window.until("the client core follows the end", |window| {
        window.told_anchor(place) == Some(None)
    });

    window.drag_scroll_bar_to_top(Pane::Main);
    assert!(!window.pane_at_end(Pane::Main));
    window.until("the older page is read", |window| {
        window.texts().len() > first_page.len()
    });
    window.until("the client core is told where the list stands", |window| {
        matches!(window.told_anchor(place), Some(Some(_)))
    });
}

/// A wheel up over a list whose rows all fit stops its following in gpui, and its next layout
/// takes up the end again at once: the button to the end is not left drawn.
#[gpui::test]
fn a_wheel_up_over_a_list_that_fits_leaves_no_button_to_the_end(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    for number in 0..3 {
        admin.send(&world, general, document(&format!("Line {number}")));
    }
    window.until_message("Line 2");
    assert!(window.pane_at_end(Pane::Main));
    assert!(!window.shows("go-to-end"));
    window.wheel_over(Pane::Main, 5.0);
    assert!(window.pane_at_end(Pane::Main));
    assert!(
        !window.shows("go-to-end"),
        "the button to the end is drawn over a list at its end"
    );
}

/// The reads of a page of older messages that went through the proxy.
fn older_reads(proxy: &FaultProxy) -> usize {
    proxy
        .all()
        .iter()
        .filter(|seen| seen.path.contains("/messages?") && seen.path.contains("before"))
        .count()
}

/// A member's window on a channel of 120 messages, through a proxy that refused the read of
/// the page before the loaded messages, a refusal that the client core does not try again.
/// The list stands at its first row.
fn older_page_refused(
    cx: &mut TestAppContext,
) -> (World, TestServer, FaultProxy, tempfile::TempDir, Window) {
    let world = World::new();
    let server = world.server("Pages");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let general = world
        .run(admin.handle.api().snapshot())
        .unwrap()
        .snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.channels)
        .find(|channel| channel.kind == ChannelKind::Text)
        .unwrap()
        .id;
    // Written before the person joins, so the channel opens with its latest page.
    for index in 0..120 {
        admin.send(&world, general, document(&format!("Message {index}")));
    }
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Member");
    window.until_message("Message 119");
    assert!(window.feed().unwrap().more_before);
    assert_eq!(older_reads(&proxy), 0);
    proxy.fail_ending("&limit=50", [Fault::Refused]);
    window.in_shell(|shell, _, cx| shell.show_feed_row(Pane::Main, 0, cx));
    window.until("the page before is refused", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.reading.older_failed == 1 && !feed.reading.older)
    });
    // The notice of the refusal takes its place above the list and asks for nothing.
    window.settle();
    assert_eq!(older_reads(&proxy), 1);
    (world, server, proxy, directory, window)
}

/// A page of older messages whose read failed and that the client core does not try again
/// is asked for again when the person moves the list again, at the same number of rows
/// (open item RP1 of the release round). Before, the list asked once per number of rows and
/// never again.
#[gpui::test]
fn a_page_whose_read_failed_is_read_again_at_the_next_move_of_the_list(cx: &mut TestAppContext) {
    let (_world, _server, proxy, _directory, mut window) = older_page_refused(cx);
    // The person moves the list at the same number of rows.
    window.in_shell(|shell, _, cx| shell.show_feed_row(Pane::Main, 2, cx));
    window.settle();
    window.in_shell(|shell, _, cx| shell.show_feed_row(Pane::Main, 0, cx));
    window.until("the page before is read again", |window| {
        window.texts().iter().any(|text| text == "Message 69")
    });
    // The page is read again; its rows can lead the list to ask for the next older page.
    assert!(older_reads(&proxy) >= 2);
}

/// At its first row the list cannot move further up, and a turn of the wheel shows the same
/// rows: that turn reads the refused page again too. Before, only a change of the first row
/// in view did, and the list stayed without its older messages.
#[gpui::test]
fn a_page_whose_read_failed_is_read_again_at_a_turn_of_the_wheel_at_the_first_row(
    cx: &mut TestAppContext,
) {
    let (_world, _server, proxy, _directory, mut window) = older_page_refused(cx);
    let list = window.cx.debug_bounds("message-list").unwrap();
    window.dispatch(ScrollWheelEvent {
        position: list.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(120.0))),
        ..Default::default()
    });
    window.until("the page before is read again", |window| {
        window.texts().iter().any(|text| text == "Message 69")
    });
    assert!(older_reads(&proxy) >= 2);
}

/// A turn of the wheel that moves the list by less than a row keeps the bar of actions on
/// the row under the pointer: gpui tests the row under a resting pointer at each paint, so
/// the window keeps the row that the pointer is over (ADR 0740).
#[gpui::test]
fn the_bar_of_actions_stays_on_the_row_under_a_resting_pointer_when_the_list_moves(
    cx: &mut TestAppContext,
) {
    use junto_client::state::feed::FeedRow;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let general = window.selected_channel();
    for index in 0..60 {
        admin.send(&world, general, document(&format!("Message {index}")));
    }
    window.until_message("Message 59");
    let list = window
        .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
        .expect("the list was drawn");
    let pointer = list.center();
    window.simulate(|cx| cx.simulate_mouse_move(pointer, None::<MouseButton>, Modifiers::none()));
    window.settle();
    let under = |window: &mut Window| {
        let ids: Vec<_> = window
            .feed()
            .unwrap()
            .rows
            .iter()
            .filter_map(|row| match row {
                FeedRow::Message(row) => Some(row.message.id),
                _ => None,
            })
            .collect();
        ids.into_iter().find(|id| {
            let name = crate::harness::selector(format!("message:{id}"));
            window
                .cx
                .debug_bounds(name)
                .is_some_and(|bounds| bounds.contains(&pointer))
        })
    };
    let hovered = under(&mut window).expect("a message under the pointer");
    let bar = crate::harness::selector(format!("react:{hovered}"));
    assert!(window.shows(bar), "the bar of the row under the pointer");
    window.wheel_over(Pane::Main, 4.0);
    assert_eq!(under(&mut window), Some(hovered));
    assert!(
        window.shows(bar),
        "the list moved by 4 pixels under the pointer and the bar of its row is gone"
    );
}
