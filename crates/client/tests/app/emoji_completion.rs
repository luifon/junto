use gpui::TestAppContext;
use junto_client::shell::Pane;

use crate::harness::{World, member, open};

#[gpui::test]
fn shortcode_completion_takes_tab_enter_and_full_names_without_sending(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let before = window.messages().len();
    window.type_into("composer-input", ":smile");
    assert!(window.shows("emoji-completion"));
    window.press("tab");
    let smile = emojis::get_by_shortcode("smile").unwrap().as_str();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        format!("{smile} ")
    );
    assert_eq!(window.messages().len(), before);
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, ":tada", window, cx));
    window.press("enter");
    let party = emojis::get_by_shortcode("tada").unwrap().as_str();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        format!("{party} ")
    );
    assert_eq!(window.messages().len(), before);
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "", window, cx));
    window.type_into("composer-input", ":smile:");
    window.settle();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        smile
    );
    window.press("secondary-z");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ":smile:"
    );
    window.press("escape");
    for literal in [
        "`:smile:`",
        "```\n:smile:\n```",
        "https://example.test/:smile:",
        "\\:smile:",
    ] {
        window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, literal, window, cx));
        window.settle();
        assert_eq!(
            window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
            literal
        );
        assert!(!window.shows("emoji-completion"));
    }
}

#[gpui::test]
fn recent_emoji_are_shared_by_pickers_quick_reactions_and_a_restart(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let party = emojis::get_by_shortcode("tada").unwrap().as_str();
    window.click("insert-emoji");
    window.in_shell(|shell, window, cx| shell.find_emoji("tada", window, cx));
    window.click_named(format!("emoji:{party}"));
    window.click("insert-emoji");
    assert!(window.shows("emoji-recents"));
    assert!(window.shows_named(format!("recent-emoji:{party}")));
    window.press("escape");
    let message = admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("react here"),
    );
    window.until_message("react here");
    window.hover_named(format!("message:{}", message.id));
    window.click_named(format!("quick-emoji:{}:{party}", message.id));
    window.until("the quick reaction is stored", |window| {
        window.messages().iter().any(|row| {
            row.message.id == message.id
                && row
                    .message
                    .reactions
                    .iter()
                    .any(|reaction| reaction.emoji == party && reaction.own)
        })
    });
    window.right_click_named(format!("message:{}", message.id));
    window.click("message-react");
    assert!(window.shows_named(format!("recent-emoji:{party}")));
    window.press("escape");
    window
        .in_shell(|shell, window, cx| shell.type_message(Pane::Main, "a raw star ⭐", window, cx));
    window.click("composer-input");
    window.press("enter");
    window.until_message("a raw star ⭐");
    window.click("insert-emoji");
    assert!(window.shows_named("recent-emoji:⭐".to_owned()));
    window.press("escape");
    window
        .cx
        .update(|_, cx| junto_client::state::finish_writes(cx));
    window.close();
    let mut window = open(cx, directory.path());
    window.until_ready();
    window.click("insert-emoji");
    assert!(window.shows_named("recent-emoji:⭐".to_owned()));
    assert!(window.shows_named(format!("recent-emoji:{party}")));
}

#[gpui::test]
fn thread_completion_keeps_the_main_draft_and_unicode_prefix(cx: &mut TestAppContext) {
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
    window.type_into("thread-composer-input", "日本語 :tada");
    assert!(window.shows("thread-emoji-completion"));
    window.press("escape");
    assert!(!window.shows("thread-emoji-completion"));
    window.type_into("thread-composer-input", "!");
    window
        .in_shell(|shell, window, cx| shell.type_message(Pane::Thread, "日本語 :tada", window, cx));
    window.press("enter");
    let party = emojis::get_by_shortcode("tada").unwrap().as_str();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        format!("日本語 {party} ")
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "main draft"
    );
    window.press("enter");
    window.until("the completed emoji is sent to the thread", |window| {
        window.replies().iter().any(|text| text.contains(party))
    });
}
