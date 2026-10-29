use gpui::TestAppContext;
use junto_client_core::links::{AppLink, parse};
use junto_client_core::model::PlaceId;

use crate::harness::{World, member};

#[gpui::test]
fn a_file_row_has_copy_and_download_actions_without_the_message_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let downloads = tempfile::tempdir().unwrap();
    window.state.update(&mut window.cx, |state, _| {
        state.set_download_directory(downloads.path().to_path_buf())
    });
    let message = crate::pictures::send_files(
        &world,
        &admin,
        PlaceId::Channel(window.selected_channel()),
        "The plan",
        vec![("plan.txt", b"a shared plan".to_vec())],
    );
    window.until_message("The plan");
    window.right_click_named("file:0".to_owned());
    assert!(window.shows("row-context-menu"));
    assert!(!window.shows("message-menu"));
    window.click_named("context-action:Copy file name".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "plan.txt"
    );
    assert!(window.shows("copied-confirmation"));
    window.right_click_named("file:0".to_owned());
    window.click_named("context-action:Copy link".to_owned());
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(
        matches!(parse(&copied), Some(AppLink::File { id, .. }) if id == message.attachments[0].id)
    );
    window.right_click_named("file:0".to_owned());
    window.click_named("context-action:Download".to_owned());
    window.until("the file from its row menu is downloaded", |_| {
        std::fs::read(downloads.path().join("plan.txt"))
            .is_ok_and(|bytes| bytes == b"a shared plan")
    });
}

#[gpui::test]
fn a_picture_menu_opens_the_viewer_and_its_menu_tracks_the_shown_picture(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let message = crate::pictures::send_files(
        &world,
        &admin,
        PlaceId::Channel(window.selected_channel()),
        "The diagrams",
        vec![
            ("first.png", crate::pictures::png(32, 24)),
            ("second.png", crate::pictures::png(32, 24)),
        ],
    );
    window.until_message("The diagrams");
    let first = message.attachments[0].id;
    let second = message.attachments[1].id;
    window.right_click_named(format!("picture:{first}"));
    assert!(window.shows("row-context-menu"));
    assert!(!window.shows("picture-viewer"));
    assert!(!window.shows("message-menu"));
    window.click_named("context-action:Open the picture".to_owned());
    window.until("the picture viewer opens from its menu", |window| {
        window.shows("picture-viewer")
    });
    window.press("shift-f10");
    window.click_named("context-action:Copy file name".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "first.png"
    );
    window.right_click_named("viewer-picture".to_owned());
    window.click("viewer-next");
    window.until("changing the picture closes its previous menu", |window| {
        !window.shows("row-context-menu")
    });
    window.right_click_named("viewer-picture".to_owned());
    window.click_named("context-action:Copy link".to_owned());
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(matches!(parse(&copied), Some(AppLink::File { id, .. }) if id == second));
    window.press("escape");
    assert!(!window.shows("picture-viewer"));
}
