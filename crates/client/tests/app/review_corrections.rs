use crate::harness::{World, member};
use crate::messages::document;
use gpui::TestAppContext;
use junto_client_core::api::PageQuery;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::MessageId;
use junto_client_core::protocol::message::{Block, Document, SendMessage};

fn excessive_list() -> Document {
    let mut body = document("still visible");
    body.blocks.insert(
        0,
        Block::Forward {
            server: "https://chat.example.test".parse().unwrap(),
            message_id: MessageId::generate(),
        },
    );
    body.blocks.push(Block::List {
        ordered: false,
        items: vec![vec![]; 5_501],
    });
    body
}

#[gpui::test]
fn an_api_document_cannot_poison_desktop_history_with_empty_list_items(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let place = PlaceId::Channel(window.selected_channel());
    let api = admin.handle.api();
    let result = world.run(api.send_message(
        place,
        &SendMessage {
            document: excessive_list(),
            reply_to: None,
            attachments: vec![],
        },
    ));
    assert_eq!(
        result.unwrap_err().rejection().map(|error| error.code),
        Some(ErrorCode::InvalidRequest)
    );
    let sent = admin.send(
        &world,
        window.selected_channel(),
        document("history remains readable"),
    );
    let page = world
        .run(api.messages(place, &PageQuery::default()))
        .unwrap();
    assert!(page.messages.iter().all(junto_client_core::model::fits));
    assert!(page.messages.iter().any(|message| message.id == sent.id));
    window.until_message("history remains readable");
}

#[gpui::test]
fn composer_refuses_the_same_list_item_overflow(_cx: &mut TestAppContext) {
    assert!(junto_client::compose::check(&excessive_list(), false).is_err());
    let mut boundary = excessive_list();
    if let Block::List { items, .. } = &mut boundary.blocks[2] {
        items.pop();
    }
    assert!(junto_client::compose::check(&boundary, false).is_ok());
    boundary.blocks.push(Block::Checklist {
        items: vec![junto_client_core::protocol::message::CheckItem {
            checked: false,
            content: vec![],
        }],
    });
    assert!(junto_client::compose::check(&boundary, false).is_err());
}

#[gpui::test]
fn forwarding_unicode_mentions_above_32_kib_is_confirmed_in_the_desktop(cx: &mut TestAppContext) {
    use junto_client::shell::Pane;
    use junto_client_core::protocol::message::Inline;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    world
        .run(admin.handle.api().update_profile(&"界".repeat(64)))
        .unwrap();
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let body = Document {
        blocks: vec![Block::Paragraph {
            content: vec![
                Inline::Account {
                    account_id: admin.handle.account().unwrap(),
                };
                200
            ],
        }],
    };
    assert!(serde_json::to_vec(&body).unwrap().len() < 16 * 1024);
    let source = admin.send(&world, window.selected_channel(), body);
    let target = admin.text_channel(&world, &mut window, "delivery");
    let server_id = window.view().selected.unwrap().id;
    window.until("the source is loaded", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == source.id)
    });
    window.in_shell(|shell, window, cx| shell.open_message_menu(Pane::Main, source.id, window, cx));
    window.click("message-forward");
    window.click_named(format!("forward-destination:{server_id}:channel:{target}"));
    window.click("forward-send");
    window.until("the forward is confirmed or refused", |window| {
        window.messages().iter().any(|row| {
            row.message
                .document
                .as_ref()
                .is_some_and(|doc| doc.forward_origins() == 1)
        }) || window.shows("send-again")
    });
    let copied = window
        .messages()
        .into_iter()
        .find(|row| {
            row.message
                .document
                .as_ref()
                .is_some_and(|doc| doc.forward_origins() == 1)
        })
        .expect("the large forward must be confirmed");
    assert!(serde_json::to_vec(&copied.message.document).unwrap().len() > 32_768);
    assert_eq!(
        world
            .run(admin.handle.api().message(copied.message.id))
            .unwrap()
            .document,
        copied.message.document
    );
}
