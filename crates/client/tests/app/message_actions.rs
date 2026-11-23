//! The hover bar and the menu of a message take their actions from one list (ADR 0740).

use gpui::{TestAppContext, px};

use crate::admin_pages::owner_window;
use crate::harness::{World, selector};

#[gpui::test]
fn the_bar_and_the_menu_of_a_message_offer_one_set_of_actions(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.write("A plan to share");
    let message = window.until_message("A plan to share").message.id;

    // The bar: icons with a tooltip for the frequent actions, and the menu button.
    window.hover_message(message);
    for name in [
        "react",
        "reply",
        "thread",
        "forward",
        "copy-message",
        "edit",
    ] {
        assert!(window.shows_named(format!("{name}:{message}")), "{name}");
    }
    for name in ["pin", "delete", "mark-unread", "copy-link"] {
        assert!(!window.shows_named(format!("{name}:{message}")), "{name}");
    }
    let copy = window
        .cx
        .debug_bounds(selector(format!("copy-message:{message}")))
        .unwrap();
    assert!(copy.size.width < px(40.0), "Copy text is an icon");
    window.click_named(format!("forward:{message}"));
    assert!(window.shows("forward-dialog"));
    window.press("escape");

    // The menu: the reactions first, then every action in groups, with Delete last.
    window.hover_message(message);
    window.click_named(format!("message-more:{message}"));
    assert!(window.shows("message-menu"));
    assert!(window.shows("message-react"));
    let mut above = window.cx.debug_bounds("message-react").unwrap().bottom();
    for name in [
        "message-reply",
        "message-thread",
        "message-forward",
        "message-pin",
        "message-mark-unread",
        "message-copy-text",
        "message-copy-link",
        "message-edit",
        "message-delete",
    ] {
        let bounds = window
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not in the menu"));
        assert!(bounds.top() >= above, "{name} is out of its order");
        above = bounds.bottom();
    }
    window.click("message-delete");
    assert!(window.shows("confirm-delete-message"));
}

/// An edit happens in the row of the message, and the composer keeps its new message
/// (ADR 0740).
#[gpui::test]
fn a_message_is_edited_in_its_row_while_the_composer_keeps_its_text(cx: &mut TestAppContext) {
    use junto_client::shell::{Pane, Pen};

    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.write("A first plan");
    let message = window.until_message("A first plan").message.id;
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "a new note", window, cx));

    window.hover_message(message);
    window.click_named(format!("edit:{message}"));
    assert!(window.shows_named(format!("message-editor:{message}")));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pen::Edit(Pane::Main), cx)),
        "A first plan"
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "a new note"
    );
    // Escape cancels the edit and leaves the message as it was.
    window.press("escape");
    assert!(!window.shows_named(format!("message-editor:{message}")));

    window.hover_message(message);
    window.click_named(format!("edit:{message}"));
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pen::Edit(Pane::Main), "A changed plan", window, cx);
        shell.focus_composer(Pen::Edit(Pane::Main), window, cx);
    });
    window.press("enter");
    let edited = window.until_message("A changed plan");
    assert_eq!(edited.message.id, message);
    window.until("the editor of the row closes", |window| {
        !window.shows_named(format!("message-editor:{message}"))
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "a new note"
    );
}

/// The editor of a row has its own completion, edit menu and files rule: what the person
/// types, cuts or pastes there changes the edit and leaves the composer as it was
/// (ADR 0740).
#[gpui::test]
fn the_editor_of_a_row_completes_cuts_and_refuses_files_on_its_own(cx: &mut TestAppContext) {
    use junto_client::shell::{Pane, Pen};

    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    window.write("A first plan");
    let message = window.until_message("A first plan").message.id;
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "a new note", window, cx));
    let edit = Pen::Edit(Pane::Main);
    let edited = |window: &mut crate::harness::Window| {
        window.in_shell(|shell, _, cx| shell.composer_text(edit, cx))
    };
    let composed = |window: &mut crate::harness::Window| {
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx))
    };

    window.hover_message(message);
    window.click_named(format!("edit:{message}"));
    // A closed shortcode becomes its emoji in the editor of the row.
    window.simulate(|cx| cx.simulate_input(" :smile:"));
    window.settle();
    let smile = emojis::get_by_shortcode("smile").unwrap().as_str();
    assert_eq!(edited(&mut window), format!("A first plan {smile}"));
    // A mention typed there offers the candidates of the editor; the arrow chooses the
    // second one, and Tab takes it.
    for name in ["Art", "Build"] {
        let role = junto_client_core::protocol::roles::CreateWorkspaceRole {
            color: None,
            hoist: false,
            name: name.to_owned(),
            permissions: Vec::new(),
        };
        let workspace_id = window.view().workspace().unwrap().id;
        world
            .run(owner.api().create_workspace_role(workspace_id, &role))
            .unwrap();
    }
    window.simulate(|cx| cx.simulate_input(" @"));
    let offered = window.until_value("the editor offers two candidates", |window| {
        let offered = window.in_shell(|shell, _, _| shell.mention_names(edit));
        (offered.len() >= 2).then_some(offered)
    });
    let chosen = |window: &mut crate::harness::Window| {
        window.in_shell(|shell, _, _| shell.chosen_mention(edit))
    };
    assert_eq!(chosen(&mut window).as_ref(), Some(&offered[0]));
    window.press("down");
    assert_eq!(chosen(&mut window).as_ref(), Some(&offered[1]));
    window.press("tab");
    let text = edited(&mut window);
    assert!(text.contains(&format!("@{}", offered[1])), "{text}");
    assert!(!text.contains(&format!("@{}", offered[0])), "{text}");
    assert_eq!(composed(&mut window), "a new note");

    // The edit menu of the editor cuts the text of the edit, not that of the composer.
    window.press("secondary-a");
    window.right_click_named("edit-composer-input".to_owned());
    assert!(window.shows("composer-menu"));
    window.click("composer-cut");
    assert_eq!(edited(&mut window), "");
    assert_eq!(composed(&mut window), "a new note");

    // A picture pasted into the editor is refused with its reason and attaches nothing.
    window
        .cx
        .write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            crate::pictures::png(4, 4),
        )));
    window.press("secondary-v");
    assert!(window.shows("edit-compose-problem"));
    assert!(window.view().lists().unwrap().draft.is_empty());
    assert_eq!(composed(&mut window), "a new note");
}
