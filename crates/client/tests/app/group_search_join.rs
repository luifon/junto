use crate::{
    admin_pages::{invite, joined},
    harness::{World, member},
    messages::document,
    support::logged_in,
};
use gpui::TestAppContext;
use junto_client::state::{
    notices::Whereabouts,
    search::{ResultTarget, SearchShown},
};
use junto_client_core::{
    model::PlaceId,
    protocol::message::{Block, Document, Inline, SendMessage},
    transfers::UploadSource,
};

#[gpui::test]
fn group_filter_controls_authors_files_and_jumps_keep_their_current_scope(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_writer_device, writer) = joined(&world, &server, &code, "Writer");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let (own, writer_id, third_id) = (window.own_account(), logged_in(&writer), logged_in(&third));
    let group = world
        .run(
            window
                .handle()
                .api()
                .create_group("Planning".to_owned(), vec![writer_id, third_id]),
        )
        .unwrap();
    let place = PlaceId::Direct(group.id);
    let mut messages = Vec::new();
    for body in [
        document("quartz plan"),
        document("quartz https://example.test/plan"),
        Document {
            blocks: vec![Block::Paragraph {
                content: vec![
                    Inline::Text {
                        text: "quartz ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Account { account_id: own },
                ],
            }],
        },
    ] {
        messages.push(
            world
                .run(writer.api().send_message(
                    place,
                    &SendMessage {
                        document: body,
                        reply_to: None,
                        attachments: Vec::new(),
                    },
                ))
                .unwrap(),
        );
    }
    let (attachment, picture) = world.run(async {
        let attachment = writer
            .transfers()
            .upload(
                place,
                UploadSource::Bytes {
                    name: "plan.png".to_owned(),
                    bytes: std::sync::Arc::new(crate::pictures::png(4, 4)),
                },
            )
            .finished()
            .await
            .unwrap();
        let picture = writer
            .api()
            .send_message(
                place,
                &SendMessage {
                    document: Document { blocks: Vec::new() },
                    reply_to: None,
                    attachments: vec![attachment],
                },
            )
            .await
            .unwrap();
        (attachment, picture)
    });
    window.until("the group is a current destination", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    window.click_named(format!("direct-row:{}", group.id));
    window.until_message("quartz plan");
    window.search_for("");
    window.in_shell(|shell, _, cx| {
        shell.set_search_filters(
            junto_client::state::search::Filters {
                conversation: Some(group.id),
                ..Default::default()
            },
            cx,
        )
    });
    window.until_searched();
    for (selector, expected) in [
        ("search-has-file", vec![picture.id]),
        ("search-has-picture", vec![picture.id]),
        ("search-has-link", vec![messages[1].id]),
        ("search-mentions-me", vec![messages[2].id]),
        (
            "search-direct",
            vec![messages[0].id, messages[1].id, messages[2].id, picture.id],
        ),
    ] {
        window.click(selector);
        let panel = window.until_searched();
        assert!(!panel.files);
        assert_eq!(panel.messages().len(), expected.len(), "{selector}");
        for row in panel.messages() {
            assert!(
                expected
                    .iter()
                    .any(|id| row.target == ResultTarget::Message(*id)),
                "{selector}"
            );
            assert_eq!(row.workspace_id, None);
            assert!(
                matches!(&row.place, Whereabouts::Direct { name: Some(name) } if name == "Planning")
            );
        }
        window.click(selector);
        window.until_searched();
    }
    window.search_for("quartz");
    let panel = window.until_searched();
    let mut authors: Vec<_> = panel.authors.iter().map(|(id, _)| *id).collect();
    authors.sort();
    let mut expected = vec![own, writer_id, third_id];
    expected.sort();
    assert_eq!(authors, expected);
    window.click("search-author");
    window.click_named(format!("search-author:{third_id}"));
    assert_eq!(window.until_searched().state, SearchShown::Nothing);
    window.click("search-author");
    window.click("search-author:anyone");
    assert_eq!(window.until_searched().messages().len(), 3);
    window.click_named(format!("search-result:{}", messages[0].id));
    window.until("a group search result opens in its group", |window| window.feed().is_some_and(|feed| feed.place == place && feed.rows.iter().any(|row| matches!(row, junto_client::state::feed::FeedRow::Message(row) if row.message.id == messages[0].id && row.target))));
    window.right_click_named(format!("direct-row:{}", group.id));
    window.click("direct-files");
    let panel = window.until_searched();
    assert!(panel.files);
    assert_eq!(panel.filters.conversation, Some(group.id));
    assert_eq!(panel.messages().len(), 1);
    assert_eq!(
        panel.messages()[0].target,
        ResultTarget::Message(picture.id)
    );
    window.click_named(format!("listed-open:{attachment}"));
    window.until("a group file has a live readable dialog", |window| {
        window.shows("linked-file-dialog")
    });
    world
        .run(window.handle().api().leave_group(group.id))
        .unwrap();
    window.until("the old group file and results lose access", |window| {
        let view = window.view();
        let panel = view
            .lists()
            .and_then(|lists| lists.search.as_ref())
            .or_else(|| {
                view.workspace()
                    .and_then(|workspace| workspace.search.as_ref())
            });
        !window.shows("linked-file-dialog")
            && panel.is_none_or(|panel| {
                panel
                    .results
                    .iter()
                    .all(|row| row.target != ResultTarget::Message(picture.id))
            })
    });
}
