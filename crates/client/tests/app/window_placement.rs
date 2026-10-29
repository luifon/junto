use crate::harness::named;
use gpui::{TestAppContext, px, size};
use junto_client::app::{self, WindowStart};
use junto_client_core::window_placement::{WindowPlacement, WindowStore};

#[gpui::test]
fn the_main_window_restores_its_size_and_saves_resizing_after_reopening(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    let state = window.state.clone();
    state.update(&mut window.cx, |state, cx| {
        state.save_main_window(
            WindowPlacement {
                width: 1020,
                height: 660,
                ..WindowPlacement::main_window()
            },
            cx,
        )
    });
    let handle = cx
        .update(|cx| app::open_main_window(state.clone(), WindowStart::default(), cx))
        .unwrap();
    let mut main = window.window_of(handle.into());
    assert_eq!(
        main.cx
            .update(|window, _| window.window_bounds().get_bounds().size),
        size(px(1020.), px(660.))
    );
    main.simulate(|cx| cx.simulate_resize(size(px(1000.), px(650.))));
    window.until("resizing updates the main placement", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.main_window_settings().width == 1000
        })
    });
    main.cx.update(|window, _| window.remove_window());
    drop(main);
    let handle = cx
        .update(|cx| app::open_main_window(state.clone(), WindowStart::default(), cx))
        .unwrap();
    let mut main = window.window_of(handle.into());
    assert_eq!(
        main.cx
            .update(|window, _| window.window_bounds().get_bounds().size),
        size(px(1000.), px(650.))
    );
    let directory = state.read_with(&window.cx, |state, _| state.client().data_dir().clone());
    window.until("the main placement survives a process restart", |_| {
        WindowStore::main_window(directory.clone())
            .load()
            .is_ok_and(|saved| saved.width == 1000 && saved.height == 650)
    });
    assert_eq!(
        WindowStore::new(directory).load().unwrap(),
        WindowPlacement::default()
    );
}

#[gpui::test]
fn the_detached_window_uses_and_updates_only_its_own_placement(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    let state = window.state.clone();
    state.update(&mut window.cx, |state, cx| {
        state.save_call_window(
            WindowPlacement {
                width: 800,
                height: 500,
                ..WindowPlacement::default()
            },
            cx,
        )
    });
    cx.update(|cx| junto_client::shell::detached::open(state.clone(), cx));
    let handle = state
        .read_with(&window.cx, |state, _| state.call_window())
        .unwrap();
    let mut detached = window.window_of(handle);
    assert_eq!(
        detached
            .cx
            .update(|window, _| window.window_bounds().get_bounds().size),
        size(px(800.), px(500.))
    );
    detached.simulate(|cx| cx.simulate_resize(size(px(860.), px(540.))));
    window.until("resizing updates the detached placement", |window| {
        state.read_with(&window.cx, |state, _| {
            state.call_window_settings().width == 860
        })
    });
    state.update(&mut window.cx, |state, cx| state.return_call(cx));
    drop(detached);
    cx.update(|cx| junto_client::shell::detached::open(state.clone(), cx));
    let handle = state
        .read_with(&window.cx, |state, _| state.call_window())
        .unwrap();
    let mut detached = window.window_of(handle);
    assert_eq!(
        detached
            .cx
            .update(|window, _| window.window_bounds().get_bounds().size),
        size(px(860.), px(540.))
    );
    let directory = state.read_with(&window.cx, |state, _| state.client().data_dir().clone());
    window.until("the detached placement survives a process restart", |_| {
        WindowStore::new(directory.clone())
            .load()
            .is_ok_and(|saved| saved.width == 860 && saved.height == 540)
    });
    assert_eq!(
        state.read_with(&window.cx, |state, _| state.main_window_settings()),
        WindowPlacement::main_window()
    );
}
