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
