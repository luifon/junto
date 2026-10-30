use gpui::TestAppContext;

#[gpui::test]
fn the_profile_preset_without_a_server_has_its_name_menu(cx: &mut TestAppContext) {
    let (_directory, mut window) = crate::harness::named(cx, "Rin");
    window.right_click_named("profile-name".to_owned());
    assert!(window.shows("row-context-menu"));
    assert!(!window.shows("context-action:Copy account ID"));
    window.click("context-action:Copy name");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Rin"
    );
}
