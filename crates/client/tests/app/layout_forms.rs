use crate::harness::{World, named, open};
use gpui::TestAppContext;

#[gpui::test]
fn a_welcome_error_keeps_the_dialog_controls_in_place(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    assert!(window.shows("continue"));
    let before = window.cx.debug_bounds("continue").unwrap();
    window.click("continue");
    window.until("the name error is drawn", |window| {
        window.shows("name-error")
    });
    assert_eq!(window.cx.debug_bounds("continue").unwrap(), before);
}

#[gpui::test]
fn a_name_error_keeps_the_following_form_controls_in_place(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    window.click("app-settings");
    window.in_shell(|shell, window, cx| shell.type_name(" ", window, cx));
    assert!(window.shows("scale-4"));
    let before = window.cx.debug_bounds("scale-4").unwrap();
    window.click("save-name");
    window.until("the name error is drawn", |window| {
        window.shows("name-error")
    });
    assert_eq!(window.cx.debug_bounds("scale-4").unwrap(), before);
}

#[gpui::test]
fn a_join_error_keeps_the_dialog_controls_in_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Layout");
    let (_directory, mut window) = named(cx, "Member");
    window.find_server(&server.address());
    let before = window.cx.debug_bounds("join-with-code").unwrap();
    window.click("join-with-code");
    window.until("the join error is drawn", |window| {
        window.shows("join-error")
    });
    assert_eq!(window.cx.debug_bounds("join-with-code").unwrap(), before);
}
