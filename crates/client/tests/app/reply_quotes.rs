use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client::state::feed::{FeedRow, QuoteText};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;

use crate::harness::{World, member};
use crate::messages::document;

#[gpui::test]
fn reply_quotes_load_older_history_at_the_original_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let channel = world.run(api.snapshot()).unwrap().snapshot.workspaces[0]
        .channels
        .iter()
        .find(|channel| channel.kind == junto_client_core::protocol::structure::ChannelKind::Text)
        .unwrap()
        .id;
    let original = admin.send(&world, channel, document("the older original"));
    for index in 0..60 {
        admin.send(&world, channel, document(&format!("later {index}")));
    }
    let answer = world
        .run(api.send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: document("the answer"),
                reply_to: Some(original.id),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until_message("the answer");
    assert!(
        !window
            .messages()
            .iter()
            .any(|row| row.message.id == original.id)
    );
    window.click_named(format!("reply-quote:{}", original.id));
    window.until("the quote opens its original in older history", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == original.id && row.target)
    });
    assert!(!window.shows("message-menu"));
    window.in_shell(|shell, window, cx| shell.go_to_end(Pane::Main, window, cx));
    window.until("the latest answer returns", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == answer.id)
    });
}

#[gpui::test]
fn reply_quotes_use_keyboard_activation_and_refuse_a_deleted_original(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let original = admin.send(&world, window.selected_channel(), document("the original"));
    world
        .run(admin.handle.api().send_message(
            PlaceId::Channel(window.selected_channel()),
            &SendMessage {
                document: document("the answer"),
                reply_to: Some(original.id),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("the answer");
    window.click_named(format!("reply-quote:{}", original.id));
    window.until("the original is selected", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == original.id && row.target)
    });
    world
        .run(admin.handle.api().delete_message(original.id))
        .unwrap();
    window.until("the quote knows that the original is deleted", |window| {
        window
            .messages()
            .iter()
            .find(|row| row.message.reply_to == Some(original.id))
            .and_then(|row| row.reply.as_ref())
            .is_some_and(|quote| matches!(quote.text, QuoteText::Deleted | QuoteText::Unknown))
    });
    window.press("enter");
    window.until("a deleted original has the shared refusal", |window| {
        window.shows("message-link-refusal")
    });
    assert!(!window.shows("message-menu"));
}

#[gpui::test]
fn reply_quotes_open_originals_in_conversations_and_threads_without_sending_a_draft(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let direct = window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    let original = world
        .run(admin.handle.api().send_message(
            direct,
            &SendMessage {
                document: document("the private original"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            direct,
            &SendMessage {
                document: document("the private answer"),
                reply_to: Some(original.id),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("the private answer");
    window.type_into("composer-input", "unfinished work");
    window.click_named(format!("reply-quote:{}", original.id));
    window.until("the direct original is selected", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == original.id && row.target)
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "unfinished work"
    );
    window.click_channel("general");
    let root = admin.send(&world, window.selected_channel(), document("a discussion"));
    let first = admin.reply(&world, root.id, document("the first reply"));
    world
        .run(admin.handle.api().reply_in_thread(
            root.id,
            &SendMessage {
                document: document("the second reply"),
                reply_to: Some(first),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("a discussion");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread is shown", |window| {
        window.replies().contains(&"the second reply".to_owned())
    });
    window.click_named(format!("reply-quote:{first}"));
    window.until("the quoted reply is selected in its thread", |window| {
        window.view().workspace().and_then(|workspace| workspace.thread.as_ref()).and_then(|thread| thread.feed.as_ref())
            .is_some_and(|feed| feed.rows.iter().any(|row| matches!(row, FeedRow::Message(row) if row.message.id == first && row.target)))
    });
}

/// A read of a quoted message that fails for a time is sent again after a wait of its own,
/// also while nothing else changes in the state (ADR 0747).
#[gpui::test]
fn a_read_of_a_quoted_message_that_fails_for_a_time_is_read_again_by_itself(
    cx: &mut TestAppContext,
) {
    use crate::support::{Fault, FaultProxy};
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let channel = world.run(api.snapshot()).unwrap().snapshot.workspaces[0]
        .channels
        .iter()
        .find(|channel| channel.kind == junto_client_core::protocol::structure::ChannelKind::Text)
        .unwrap()
        .id;
    let original = admin.send(&world, channel, document("the older original"));
    for index in 0..60 {
        admin.send(&world, channel, document(&format!("later {index}")));
    }
    world
        .run(api.send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: document("the answer"),
                reply_to: Some(original.id),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    let read = format!("/messages/{}", original.id);
    proxy.fail_ending(&read, [Fault::Cut]);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until_message("the answer");
    let quote = |window: &mut crate::harness::Window| {
        window
            .messages()
            .iter()
            .find(|row| row.message.reply_to == Some(original.id))
            .and_then(|row| row.reply.as_ref())
            .map(|quote| quote.text.clone())
    };
    window.until("the first read of the original was cut", |_| {
        proxy
            .all()
            .iter()
            .any(|seen| seen.method == "GET" && seen.path.ends_with(&read))
    });
    window.until(
        "the quote shows the original after a read of its own",
        |window| quote(window) == Some(QuoteText::Text("the older original".to_owned())),
    );
    assert!(
        proxy
            .all()
            .iter()
            .filter(|seen| seen.method == "GET" && seen.path.ends_with(&read))
            .count()
            >= 2
    );
}

/// A forward keeps the original that the window read while a reload of the state reads it
/// again (ADR 0747).
#[gpui::test]
fn a_forward_keeps_its_original_while_a_reload_reads_it_again(cx: &mut TestAppContext) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let source_channel = window.selected_channel();
    let source = admin.send(&world, source_channel, document("the forwarded plan"));
    let target = admin.text_channel(&world, &mut window, "delivery");
    let server_id = window.view().selected.unwrap().id;
    window.until_message("the forwarded plan");
    window.right_click_message(source.id);
    window.click("message-forward");
    window.click_named(format!("forward-destination:{server_id}:channel:{target}"));
    window.click("forward-send");
    window.until("the forward shows its original", |window| {
        window.selected_channel() == target
            && window.shows_named(format!("forward-message:{}", source.id))
    });
    let read = format!("/messages/{}", source.id);
    let reads = |proxy: &FaultProxy| {
        proxy
            .all()
            .iter()
            .filter(|seen| seen.method == "GET" && seen.path.ends_with(&read))
            .count()
    };
    let before = reads(&proxy);
    proxy.hold_ending(&read);
    let refreshing = window.state.read_with(&window.cx, |state, _| {
        state.client().server(server_id).unwrap().live().refresh()
    });
    window.until("the reload reads the original again", |_| {
        reads(&proxy) > before
    });
    window.settle();
    assert!(
        window.shows_named(format!("forward-message:{}", source.id)),
        "the forward keeps its original while the read waits"
    );
    assert!(!window.shows_named(format!("forward-reading:{}", source.id)));
    proxy.release();
    window.until("the answer of the read is shown", |window| {
        window.shows_named(format!("forward-message:{}", source.id))
    });
    drop(refreshing);
}
