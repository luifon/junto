use gpui::{ClipboardItem, TestAppContext};
use junto_client::shell::Pane;
use junto_client::state::feed::FeedRow;
use junto_client_core::protocol::message::{Block, Inline};

use crate::harness::{Window, World, member};

fn painted(window: &mut Window) -> String {
    window.settle();
    window
        .in_shell(|shell, _, _| shell.painted_readonly_text())
        .join("\n")
}

#[gpui::test]
fn preview_renders_the_sent_document_and_keeps_the_written_text(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let text = "# Review\n-# A quiet note\nA **strong** result with ||a secret||.\n```rust\nfn run() {\n    work();\n}\n```";
    window.type_into("composer-input", text);
    let before = window.messages().len();
    assert!(window.shows("composer-view-1"));
    window.click("composer-view-1");
    assert!(window.shows("composer-preview"));
    assert!(!window.shows("composer-input"));
    let drawn = painted(&mut window);
    assert!(
        drawn.contains("Review") && drawn.contains("A quiet note") && drawn.contains("Spoiler"),
        "{drawn}"
    );
    assert!(!drawn.contains("a secret"));
    assert_eq!(window.messages().len(), before);
    window.click("composer-view-0");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        text
    );
    window.click("composer-view-1");
    window.click("send-message");
    window.until("the previewed message is stored", |window| {
        window.messages().len() > before
    });
    let message = window.messages().last().unwrap().message.clone();
    assert_eq!(
        message.document,
        Some(junto_client::compose::parse(text, &[]))
    );
    assert!(matches!(
        &message.document.as_ref().unwrap().blocks[0],
        Block::Heading { level: 1, .. }
    ));
    assert!(window.shows("composer-input"));
}

#[gpui::test]
fn preview_keeps_reply_files_and_edit_state_in_a_conversation(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    window.write("a starting message");
    let root = window.until_message("a starting message").message;
    window.in_shell(|shell, window, cx| shell.reply_to_message(Pane::Main, root.id, window, cx));
    window.type_into("composer-input", "**reviewed**");
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("review.txt");
    std::fs::write(&path, b"review notes").unwrap();
    window.in_shell(|shell, _, cx| shell.attach_paths(Pane::Main, vec![path], cx));
    window.until("the file is attached", |window| {
        window.shows("draft-file:0") && !window.shows("draft-attaching")
    });
    window.click("composer-view-1");
    assert!(window.shows("draft-file:0"));
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).0),
        Some(root.id)
    );
    window.click("send-message");
    let reply = window.until_message("reviewed").message;
    assert_eq!(reply.reply_to, Some(root.id));
    assert_eq!(reply.attachments.len(), 1);
    window.in_shell(|shell, window, cx| shell.edit_message(Pane::Main, reply.id, window, cx));
    let before = window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx));
    window.click("composer-view-1");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).1),
        Some(reply.id)
    );
    window.click("composer-view-0");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        before
    );
    window.click("composer-view-1");
    window.click("send-message");
    window.until("the previewed edit completes", |window| {
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).1.is_none())
    });
    assert_eq!(
        world
            .run(admin.handle.api().message(reply.id))
            .unwrap()
            .document,
        reply.document
    );
}

#[gpui::test]
fn thread_preview_is_independent_and_invalid_text_keeps_its_draft(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let root = admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("a discussion"),
    );
    window.until_message("a discussion");
    window.type_into("composer-input", "main draft");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread composer opens", |window| {
        window.shows("thread-composer-input")
    });
    window.type_into("thread-composer-input", "## Thread plan\nSee #gen");
    window.until("channel completion appears", |window| {
        window.shows("candidate:0")
    });
    window.click("candidate:0");
    let thread_text = window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx));
    window.click("thread-composer-view-1");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        thread_text
    );
    assert!(window.shows("thread-composer-preview"));
    assert!(window.shows("composer-input"));
    assert!(painted(&mut window).contains("Thread plan"));
    window.click("thread-send-message");
    window.until("the thread sends its preview", |window| {
        window
            .replies()
            .iter()
            .any(|text| text.contains("Thread plan"))
    });
    let view = window.view();
    let feed = view
        .workspace()
        .unwrap()
        .thread
        .as_ref()
        .unwrap()
        .feed
        .as_ref()
        .unwrap();
    assert!(feed.rows.iter().any(|row| {
        let FeedRow::Message(row) = row else {
            return false;
        };
        row.message.document.as_ref().is_some_and(|document| {
            document
                .inlines()
                .any(|(node, _)| matches!(node, Inline::Channel { .. }))
        })
    }));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "main draft"
    );
    let too_long = "x".repeat(20 * 1024);
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string(too_long.clone()));
    window.click("composer-input");
    window.press("secondary-a");
    window.press("secondary-v");
    window.click("composer-view-1");
    assert!(window.shows("compose-problem"));
    window.click("send-message");
    window.click("composer-view-0");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        too_long
    );
}

#[gpui::test]
fn preview_mention_menus_follow_the_current_document_in_both_composers(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let account = admin.handle.account().unwrap();
    let root = admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("a discussion"),
    );
    window.until_message("a discussion");
    for pane in [Pane::Main, Pane::Thread] {
        if pane == Pane::Thread {
            window.right_click_named(format!("message:{}", root.id));
            window.click("message-thread");
            window.until("the thread composer opens", |window| {
                window.shows("thread-composer-input")
            });
        }
        let input = if pane == Pane::Main {
            "composer-input"
        } else {
            "thread-composer-input"
        };
        let preview = if pane == Pane::Main {
            "composer-view-1"
        } else {
            "thread-composer-view-1"
        };
        window.type_into(input, "@Ada");
        window.until("the account can be mentioned", |window| {
            window.shows("candidate:0")
        });
        window.click("candidate:0");
        let draft = window.in_shell(|shell, _, cx| shell.composer_text(pane, cx));
        window.click(preview);
        let bounds = window.cx.debug_bounds("profile-text").unwrap();
        let point = gpui::point(bounds.left() + gpui::px(8.0), bounds.center().y);
        window.press_at_count(point, gpui::MouseButton::Right, 1);
        assert!(window.shows_named(format!("account-copy-id:{account}")));
        window.click_named(format!("account-copy-id:{account}"));
        assert_eq!(
            window.cx.read_from_clipboard().unwrap().text().unwrap(),
            account.to_string()
        );
        assert_eq!(
            window.in_shell(|shell, _, cx| shell.composer_text(pane, cx)),
            draft
        );
        window.press_at_count(point, gpui::MouseButton::Right, 1);
        assert!(window.shows("account-menu"));
        window.in_shell(|shell, window, cx| {
            shell.type_message(pane, "the changed draft", window, cx)
        });
        assert!(!window.shows("account-menu"));
        assert_eq!(
            window.in_shell(|shell, _, cx| shell.composer_text(pane, cx)),
            "the changed draft"
        );
        window.in_shell(|shell, window, cx| shell.show_composer_preview(pane, false, window, cx));
        if pane == Pane::Thread {
            assert_eq!(
                window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
                "the changed draft"
            );
        }
    }
}
