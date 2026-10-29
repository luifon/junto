use crate::harness::{Window, named};
use gpui::{Action, OwnedMenuItem, TestAppContext};

fn main_window(cx: &mut TestAppContext) -> (tempfile::TempDir, Window) {
    let (directory, mut original) = named(cx, "Member");
    let handle = cx
        .update(|cx| {
            junto_client::app::open_main_window(original.state.clone(), Default::default(), cx)
        })
        .unwrap();
    let main = original.window_of(handle.into());
    original.cx.update(|window, _| window.remove_window());
    (directory, main)
}

fn action(window: &mut Window, title: &str) -> Box<dyn Action> {
    window.cx.update(|_, cx| {
        cx.get_menus()
            .unwrap()
            .into_iter()
            .flat_map(|menu| menu.items)
            .find_map(|item| match item {
                OwnedMenuItem::Action { name, action, .. } if name == title => Some(action),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing menu action: {title}"))
    })
}

fn choose(window: &mut Window, title: &str) {
    let action = action(window, title);
    window.simulate(|cx| cx.update(|window, cx| window.dispatch_action(action, cx)));
}

#[gpui::test]
fn native_menus_include_editing_settings_and_standard_window_commands(cx: &mut TestAppContext) {
    let (_directory, mut window) = main_window(cx);
    let menus = window.cx.update(|_, cx| cx.get_menus().unwrap());
    assert_eq!(
        menus
            .iter()
            .map(|menu| menu.name.as_str())
            .collect::<Vec<_>>(),
        ["Junto", "File", "Edit", "View", "Window", "Help"]
    );
    for name in [
        "About Junto",
        "Settings…",
        "Hide Junto",
        "Hide Others",
        "Show All",
        "Quit Junto",
        "Close Window",
        "Undo",
        "Redo",
        "Cut",
        "Copy",
        "Paste",
        "Select All",
        "Enter Full Screen",
        "Minimize",
        "Zoom",
        "Bring All to Front",
        "Keyboard Shortcuts",
    ] {
        let _ = action(&mut window, name);
    }
}

#[gpui::test]
fn edit_menu_commands_reach_the_focused_input(cx: &mut TestAppContext) {
    let (_directory, mut window) = main_window(cx);
    choose(&mut window, "Settings…");
    assert!(window.shows("settings-dialog"));
    window.click("name-field");
    choose(&mut window, "Select All");
    window.simulate(|cx| cx.simulate_input("New Name"));
    choose(&mut window, "Select All");
    choose(&mut window, "Cut");
    assert_eq!(
        window
            .cx
            .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("New Name".into())
    );
    choose(&mut window, "Paste");
    choose(&mut window, "Select All");
    choose(&mut window, "Copy");
    assert_eq!(
        window
            .cx
            .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("New Name".into())
    );
}

#[gpui::test]
fn application_menu_settings_restore_the_closed_main_window(cx: &mut TestAppContext) {
    let (_directory, mut window) = main_window(cx);
    let settings = action(&mut window, "Settings…");
    choose(&mut window, "Close Window");
    assert!(cx.windows().is_empty());
    cx.update(|cx| cx.dispatch_action(settings.as_ref()));
    let handle = cx.windows()[0];
    let mut restored = window.window_of(handle);
    assert!(restored.shows("settings-dialog"));
    choose(&mut restored, "About Junto");
    assert!(restored.shows("version-section"));
    choose(&mut restored, "Keyboard Shortcuts");
    assert!(restored.shows("reset-keybinds"));
}

#[gpui::test]
fn the_standard_settings_shortcut_keeps_working_in_an_input(cx: &mut TestAppContext) {
    let (_directory, mut window) = main_window(cx);
    window.press("secondary-,");
    assert!(window.shows("settings-dialog"));
    window.click("name-field");
    window.press("escape");
    window.press("secondary-,");
    assert!(window.shows("settings-dialog"));
}
