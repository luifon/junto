//! The logo of a server in the window against the real server (ADR 0101): an administrator
//! sets and removes it on the page "Server" of the administration, and the server book of
//! every window follows the event.

use gpui::TestAppContext;
use junto_client::state::admin::view::{Scope, ServerPage};

use crate::admin_pages::{click_in_page, invite, open_page};
use crate::harness::{Window, World, named};

/// A PNG image of one colour.
fn png(side: u32, shade: u8) -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(side, side, image::Rgba([shade, 90, 40, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// True when the administration shows the page "Server".
fn shows_server_page(window: &mut Window) -> bool {
    matches!(
        window.admin().map(|view| view.scope),
        Some(Scope::Server {
            page: ServerPage::Server(_),
            ..
        })
    )
}

#[gpui::test]
fn an_administrator_sets_and_removes_the_logo_and_the_window_of_a_member_follows(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    // The owner claims the server in a window.
    let (_owner_directory, mut owner) = named(cx, "Ada");
    owner.find_server(&server.address());
    owner.join_with_code(&server.owner_code().unwrap());
    owner.until_ready();
    // The window of a member is open before the server has a logo.
    let code = invite(&world, &owner.handle(), Vec::new());
    let (_member_directory, mut member) = named(cx, "Luca");
    member.find_server(&server.address());
    member.join_with_code(code.expose());
    member.until_ready();
    assert!(!owner.shows("server-logo-0") && !member.shows("server-logo-0"));

    owner.open_administration();
    open_page(&mut owner, "admin-nav-server", shows_server_page);
    assert!(owner.shows("logo-none") && owner.shows("logo-choose"));
    assert!(!owner.shows("logo-picture") && !owner.shows("logo-remove"));

    // The owner chooses a file: the server stores its bytes, and the page and the server
    // book of the owner show the image.
    let files = tempfile::tempdir().unwrap();
    let image = png(64, 200);
    let chosen = files.path().join("logo.png");
    std::fs::write(&chosen, &image).unwrap();
    owner.in_shell(|shell, _, cx| shell.use_server_logo(chosen, cx));
    owner.until("the page shows the logo", |window| {
        window.shows("logo-picture") && window.shows("logo-remove")
    });
    assert!(!owner.shows("logo-none") && !owner.shows("logo-refused"));
    owner.until("the server book of the owner shows the logo", |window| {
        window.shows("server-logo-0")
    });
    assert!(owner.view().servers[0].logo.is_some());
    let stored = world.run(owner.handle().logo()).unwrap().unwrap();
    assert_eq!(stored.bytes, image);

    // The window of the member follows the event: the member does nothing.
    member.until("the server book of the member shows the logo", |window| {
        window.shows("server-logo-0")
    });
    assert!(member.view().servers[0].logo.is_some());

    // The removal asks first, and nothing is sent before the answer.
    click_in_page(&mut owner, "logo-remove");
    assert!(owner.shows("logo-remove-confirm") && owner.shows("logo-remove-cancel"));
    click_in_page(&mut owner, "logo-remove-cancel");
    assert!(owner.shows("logo-remove") && !owner.shows("logo-remove-confirm"));
    assert!(world.run(owner.handle().logo()).unwrap().is_some());
    assert!(owner.shows("server-logo-0"));

    click_in_page(&mut owner, "logo-remove");
    click_in_page(&mut owner, "logo-remove-confirm");
    owner.until("the page shows no logo", |window| {
        window.shows("logo-none") && window.shows("logo-choose") && !window.shows("logo-remove")
    });
    assert!(!owner.shows("logo-picture") && !owner.shows("logo-refused"));
    owner.until(
        "the server book of the owner shows the generated picture",
        |window| !window.shows("server-logo-0"),
    );
    member.until(
        "the server book of the member shows the generated picture",
        |window| !window.shows("server-logo-0"),
    );
    assert_eq!(member.view().servers[0].logo, None);
    assert!(world.run(owner.handle().logo()).unwrap().is_none());
}
