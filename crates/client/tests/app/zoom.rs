use crate::harness::{named, open};
use gpui::TestAppContext;

#[gpui::test]
fn zoom_keys_step_reset_and_keep_the_interface_scale_after_restart(cx: &mut TestAppContext) {
    let (directory, mut window) = named(cx, "Member");
    window.press("secondary-=");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        110
    );
    window.press("secondary--");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        100
    );
    for _ in 0..10 {
        window.press("secondary-=");
    }
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        150
    );
    for _ in 0..10 {
        window.press("secondary--");
    }
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        90
    );
    window.press("secondary-0");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        100
    );
    window.state.update(&mut window.cx, |state, cx| {
        state.use_scale_for_this_start(117, cx)
    });
    window.press("secondary-=");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        125
    );
    window.run_tasks();
    window.close();
    let mut reopened = open(cx, directory.path());
    assert_eq!(
        reopened
            .state
            .read_with(&reopened.cx, |state, _| state.settings().scale_percent),
        125
    );
    reopened.press("secondary-0");
    assert_eq!(
        reopened
            .state
            .read_with(&reopened.cx, |state, _| state.settings().scale_percent),
        100
    );
}

#[gpui::test]
fn the_detached_window_and_view_menu_share_the_same_zoom(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    cx.update(|cx| junto_client::shell::detached::open(window.state.clone(), cx));
    let handle = window
        .state
        .read_with(&window.cx, |state, _| state.call_window().unwrap());
    let mut detached = window.window_of(handle);
    detached.press("secondary-=");
    window.until("the main window follows the call window's zoom", |window| {
        window.cx.update(|window, _| window.rem_size()) == junto_client::theme::rem_size(110)
    });
    assert_eq!(
        detached.cx.update(|window, _| window.rem_size()),
        junto_client::theme::rem_size(110)
    );
    let reset = detached.cx.update(|_, cx| {
        cx.get_menus()
            .unwrap()
            .into_iter()
            .flat_map(|menu| menu.items)
            .find_map(|item| match item {
                gpui::OwnedMenuItem::Action { name, action, .. } if name == "Actual Size" => {
                    Some(action)
                }
                _ => None,
            })
            .unwrap()
    });
    detached.simulate(|cx| cx.update(|window, cx| window.dispatch_action(reset, cx)));
    window.until("the menu resets both windows", |window| {
        window.cx.update(|window, _| window.rem_size()) == junto_client::theme::rem_size(100)
    });
}

#[gpui::test]
fn zoom_rebinding_removes_the_old_menu_shortcut(cx: &mut TestAppContext) {
    use junto_client::state::KeybindChange;
    use junto_client_core::keybinds::KeyAction;
    let (_directory, mut window) = named(cx, "Member");
    window.state.update(&mut window.cx, |state, cx| {
        state
            .change_keybinds(KeybindChange::Set(KeyAction::ZoomIn, None), cx)
            .unwrap();
    });
    window.press("secondary-=");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        100
    );
    window.state.update(&mut window.cx, |state, cx| {
        state
            .change_keybinds(
                KeybindChange::Set(KeyAction::ZoomIn, Some("control+alt+z".parse().unwrap())),
                cx,
            )
            .unwrap();
    });
    window.press("ctrl-alt-z");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        110
    );
    window.press("secondary-=");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        110
    );
}
