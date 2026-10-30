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
