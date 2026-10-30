use gpui::{ClipboardItem, TestAppContext};
use junto_client::state::feed::FeedRow;
use junto_client::text;
use junto_client_core::address::ServerAddress;
use junto_client_core::links::{self, AppLink};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::MessageId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};

use crate::harness::{Window, World, member};
use crate::messages::document;

fn open(window: &mut Window, address: &ServerAddress, id: MessageId) -> bool {
    window.state.update(&mut window.cx, |state, cx| {
        state.open_message_link(address, id, cx)
    })
}

#[gpui::test]
fn a_copied_link_opens_older_history_and_a_thread(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.write("the original message");
    let original = window.until_message("the original message").message;
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
    window.right_click_named(format!("message:{}", original.id));
    window.click("message-copy-link");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    let (address, id) = match links::parse(&copied) {
        Some(AppLink::Message { server, id }) => (server, id),
        #[allow(unreachable_patterns)]
        _ => panic!("the clipboard does not hold a message link"),
    };
    assert_eq!(address, origin);
    assert_eq!(id, original.id);
    assert!(window.shows("copied-confirmation"));
    let other = admin.text_channel(&world, &mut window, "other");
    for number in 0..60 {
        admin.send(
            &world,
            window.selected_channel(),
            document(&format!("later {number}")),
        );
    }
    window.click_channel("other");
    window.until("the other channel is open", |window| {
        window.selected_channel() == other
    });
    assert!(open(&mut window, &address, id));
    window.until("the linked message is marked in older history", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == id && row.target)
    });
    let reply = admin.reply(&world, id, document("a linked reply"));
    assert!(open(&mut window, &address, reply));
    window.until("the linked reply is marked in its thread", |window| {
        window.view().workspace().and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref()).is_some_and(|feed| feed.rows.iter().any(|row| matches!(row, FeedRow::Message(row) if row.message.id == reply && row.target)))
    });
    assert!(open(&mut window, &address, id));
    window.until("a channel link closes the previous thread", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.thread.is_none())
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == id && row.target)
    });
}

#[gpui::test]
fn a_message_link_opens_a_direct_conversation(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let direct = window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    let message = world
        .run(admin.handle.api().send_message(
            direct,
            &SendMessage {
                document: document("the private message"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("the private message");
    window.click_channel("general");
    assert!(open(&mut window, &origin, message.id));
    window.until("the linked direct message is marked", |window| {
        window.feed().is_some_and(|feed| feed.place == direct)
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == message.id && row.target)
    });
}

#[gpui::test]
fn missing_and_inaccessible_links_show_the_same_refusal_and_unknown_origins_are_returned(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the channel is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let hidden_channel = window.selected_channel();
    let hidden = admin.send(&world, hidden_channel, document("keep this private"));
    let other = admin.text_channel(&world, &mut window, "other");
    window.click_channel("other");
    let api = admin.handle.api();
    let workspace = window.view().workspace().unwrap().id;
    let everyone = world
        .run(api.workspace_roles(workspace))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    world
        .run(api.set_channel_rule(
            hidden_channel,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("access is removed", |window| {
        !window.channels().iter().any(|row| row.id == hidden_channel)
    });
    assert!(open(&mut window, &origin, hidden.id));
    window.until("the inaccessible link is refused", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.message_link_refusal() == Some(text::MESSAGE_LINK_UNAVAILABLE)
        })
    });
    assert!(window.shows("message-link-refusal"));
    assert_eq!(window.selected_channel(), other);
    window.click("dismiss-message-link-refusal");
    window.in_shell(|shell, window, cx| shell.open_version_settings(window, cx));
    let missing = MessageId::generate();
    assert!(open(&mut window, &origin, missing));
    window.until("the missing link is refused identically", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.message_link_refusal() == Some(text::MESSAGE_LINK_UNAVAILABLE)
        })
    });
    assert!(window.shows("message-link-refusal"));
    assert!(window.shows("version-build"));
    window.click("dismiss-message-link-refusal");
    assert!(!window.shows("message-link-refusal"));
    assert!(window.shows("version-build"));
    assert_eq!(window.selected_channel(), other);
    let unknown = ServerAddress::parse("https://unknown.example.test").unwrap();
    assert!(!open(&mut window, &unknown, missing));
    assert_eq!(window.selected_channel(), other);
}
