//! The read position of a place in the window against the real server (ADR 0740): the end
//! of a list that an active window shows is read without an action of the person, and two
//! windows of one account show one read position.

use gpui::TestAppContext;
use junto_client::state::feed::FeedRow;
use junto_client_core::flow::access::AccessState;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::ChannelId;

use crate::harness::{Window, World, member, named};
use crate::messages::document;
use crate::support::TestServer;

impl Window {
    /// The unread messages and mentions of the row of a channel.
    #[track_caller]
    fn unread_of(&mut self, channel_id: ChannelId) -> (u64, u64) {
        self.channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .map(|row| (row.unread.messages, row.unread.mentions))
            .unwrap()
    }

    fn activate(&mut self) {
        self.cx.update(|window, _| window.activate_window());
        self.settle();
    }

    fn is_active(&mut self) -> bool {
        self.cx.update(|window, _| window.is_window_active())
    }
}

/// A second window of the account of `first`, on a device that the first window links.
fn linked_window(
    cx: &mut TestAppContext,
    world: &World,
    server: &TestServer,
    first: &mut Window,
) -> (tempfile::TempDir, Window) {
    let (directory, mut second) = named(cx, "Luca");
    second.find_server(&server.address());
    second.click("request-link");
    let (code, public_key) = second.until_value("the second device waits for its link", |second| {
        let handle = second
            .state
            .read_with(&second.cx, |state, _| state.client().servers())
            .into_iter()
            .next()?;
        match handle.access().state {
            AccessState::WaitingForLink {
                code, public_key, ..
            } => Some((code, public_key)),
            _ => None,
        }
    });
    world
        .run(first.handle().api().approve_device_link(code, public_key))
        .unwrap();
    second.until_ready();
    assert_eq!(
        second.view().person.account_id,
        first.view().person.account_id
    );
    (directory, second)
}

#[gpui::test]
fn the_end_of_a_list_in_the_active_window_is_read_without_an_action(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let random = admin.text_channel(&world, &mut window, "random");
    let own = window.handle();
    let stored = |world: &World| {
        world
            .run(own.api().message_state(PlaceId::Channel(random)))
            .unwrap()
            .unread
    };
    admin.send(&world, random, document("first news"));
    admin.send(&world, random, document("second news"));
    window.until("the unread messages of the other channel", |window| {
        window.unread_of(random) == (2, 0)
    });

    // The window is not the active window of the system: it shows the messages, and they
    // stay unread.
    assert!(!window.is_active());
    window.click_channel("random");
    window.until("the messages of the other channel are drawn", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Channel(random))
            && window.texts() == ["first news", "second news"]
    });
    assert!(window.in_shell(|shell, _, _| shell.feed_is_at_end(junto_client::shell::Pane::Main)));
    // The client core sends a read mark at most one second after the window makes it, so
    // the count of the server is read again after two seconds of the window, and both
    // readings must hold: one reading right after the drawing would also pass with a mark
    // that is still on its way.
    assert_eq!(window.unread_of(random), (2, 0));
    assert_eq!(stored(&world), 2);
    window.pass(std::time::Duration::from_secs(2));
    assert_eq!(window.unread_of(random), (2, 0));
    assert_eq!(stored(&world), 2);
    // A further message is drawn at the end of the list of the inactive window, and it
    // stays unread too: the window reads nothing later by itself.
    admin.send(&world, random, document("more news"));
    window.until_message("more news");
    window.until("the server counts the three messages", |window| {
        window.unread_of(random) == (3, 0) && stored(&world) == 3
    });

    // The person looks at the window: the end of the list is shown, and the messages are
    // read on the server.
    window.activate();
    window.until("the messages are read", |window| {
        window.unread_of(random) == (0, 0)
    });
    window.until("the server holds the read position", |_| {
        stored(&world) == 0
    });

    // A message that arrives at the end of the shown list is read at once.
    admin.send(&world, random, document("third news"));
    window.until_message("third news");
    window.until("the new message is read", |window| {
        window.unread_of(random) == (0, 0) && stored(&world) == 0
    });
}

#[gpui::test]
fn two_windows_of_one_account_show_one_read_position(
    cx: &mut TestAppContext,
    second_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut first) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second_directory, mut second) = linked_window(second_cx, &world, &server, &mut first);
    let random = admin.text_channel(&world, &mut first, "random");
    second.until("the second window lists the channel", |second| {
        second.channels().iter().any(|row| row.id == random)
    });
    admin.send(&world, random, document("first news"));
    admin.send(&world, random, document("second news"));
    for window in [&mut first, &mut second] {
        window.until("the unread messages", |window| {
            window.unread_of(random) == (2, 0)
        });
    }

    // The first window shows the channel to the person: its messages are read.
    first.click_channel("random");
    first.activate();
    first.until("the first window read the messages", |first| {
        first.unread_of(random) == (0, 0)
    });

    // The second window shows no unread message there, and its list of the channel has
    // no line of the first unread message. It is not the active window, so it moved
    // nothing itself.
    assert!(!second.is_active());
    second.until("the second window has the read position", |second| {
        second.unread_of(random) == (0, 0)
    });
    second.click_channel("random");
    second.until("the messages in the second window", |second| {
        second.texts() == ["first news", "second news"]
    });
    let feed = second.feed().unwrap();
    assert!(
        feed.rows.iter().all(|row| !matches!(row, FeedRow::Unread)),
        "the second window marks a message as the first unread one"
    );
    assert_eq!(
        world
            .run(
                second
                    .handle()
                    .api()
                    .message_state(PlaceId::Channel(random))
            )
            .unwrap()
            .unread,
        0
    );
}

#[gpui::test]
fn manual_unread_stays_unread_in_active_windows_and_on_the_other_device(
    cx: &mut TestAppContext,
    second_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut first) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second_directory, mut second) = linked_window(second_cx, &world, &server, &mut first);
    let channel = first.selected_channel();
    let message = admin.send(&world, channel, document("come back to this work"));
    for window in [&mut first, &mut second] {
        window.until_message("come back to this work");
        window.activate();
        window.until("the message is read", |window| {
            window.unread_of(channel) == (0, 0)
        });
    }
    first.right_click_named(format!("message:{}", message.id));
    first.click("message-mark-unread");
    for window in [&mut first, &mut second] {
        window.until(
            "the explicit unread position reaches this device",
            |window| window.unread_of(channel) == (1, 0),
        );
    }
    admin.send(&world, channel, document("later work also stays unread"));
    for window in [&mut first, &mut second] {
        window.until_message("later work also stays unread");
        window.until("new messages keep the explicit unread boundary", |window| {
            window.unread_of(channel) == (2, 0)
        });
    }
    first.close();
    let mut first = crate::harness::open(cx, directory.path());
    first.until_ready();
    first.until("the unread reminder survives a restart", |window| {
        window.unread_of(channel) == (2, 0)
    });
    first.click_channel("general");
    first.click("mark-as-read");
    for window in [&mut first, &mut second] {
        window.until(
            "an explicit read clears the unread signs on both devices",
            |window| window.unread_of(channel) == (0, 0),
        );
    }
}

#[gpui::test]
fn manual_unread_works_in_direct_conversations_and_threads(cx: &mut TestAppContext) {
    use junto_client_core::protocol::message::SendMessage;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let direct = window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    let sent = world
        .run(admin.handle.api().send_message(
            direct,
            &SendMessage {
                document: document("a private reminder"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("a private reminder");
    window.activate();
    window.until("the direct message is read", |window| {
        window.feed().is_some_and(|feed| feed.unread == 0)
    });
    window.right_click_named(format!("message:{}", sent.id));
    window.click("message-mark-unread");
    window.until("the direct message is unread again", |window| {
        window.feed().is_some_and(|feed| feed.unread == 1)
    });
    assert_eq!(
        world
            .run(window.handle().api().message_state(direct))
            .unwrap()
            .last_read,
        sent.position - 1
    );
    window.click("mark-as-read");
    window.until("the direct message is explicitly read", |window| {
        window.feed().is_some_and(|feed| feed.unread == 0)
    });
    window.click_channel("general");
    let root = admin.send(&world, window.selected_channel(), document("a discussion"));
    let reply = admin.reply(&world, root.id, document("a reply reminder"));
    window.until_message("a discussion");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread reply is drawn", |window| {
        window.replies().contains(&"a reply reminder".to_owned())
    });
    window.right_click_named(format!("message:{reply}"));
    window.click("message-mark-unread");
    window.until("the thread is unread again", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref())
            .is_some_and(|feed| feed.unread == 1)
    });
    window.click("thread-mark-as-read");
    window.until("the thread is explicitly read", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref())
            .is_some_and(|feed| feed.unread == 0)
    });
}

/// An unread count that arrives while the menu of a message is open moves no item of the
/// menu: "Mark as read" stays in it while nothing is unread, disabled (ADR 0730). In the
/// parallel runs of the thread tests the item came with the count of a message that had just
/// arrived and moved "Reply in thread" under the press, so the press opened nothing.
#[gpui::test]
fn an_unread_count_that_arrives_while_a_message_menu_is_open_moves_no_item(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    window.type_into("composer-input", "my own message");
    window.press("enter");
    window.until_message("my own message");
    let own = window.messages().last().unwrap().message.id;
    assert_eq!(window.unread_of(channel), (0, 0));
    window.right_click_message(own);
    let before = window.cx.debug_bounds("message-thread").unwrap();
    assert!(window.shows("message-mark-read"));
    admin.send(&world, channel, document("a message that is not read"));
    window.until("the channel counts the new message", |window| {
        window.unread_of(channel) == (1, 0)
    });
    assert!(window.shows("message-mark-read"));
    assert_eq!(window.cx.debug_bounds("message-thread"), Some(before));
}
