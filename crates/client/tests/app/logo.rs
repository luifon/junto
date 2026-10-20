//! The page "Server" of the administration against the real server: an administrator sets
//! and removes the logo (ADR 0101), and the server book of every window follows the event;
//! an administrator changes the limits of the files of messages (ADR 0408).

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
    // The head of the menu of the selected server shows it too.
    member.click("server-menu-button");
    assert!(member.shows("server-menu") && member.shows("selected-server-logo-0"));
    member.press("escape");
    assert!(!member.shows("server-menu"));

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
    member.click("server-menu-button");
    assert!(member.shows("server-menu") && !member.shows("selected-server-logo-0"));
    member.press("escape");
    assert!(world.run(owner.handle().logo()).unwrap().is_none());
}

#[gpui::test]
fn a_logo_dialog_that_is_answered_on_the_page_of_another_server_sends_nothing(
    cx: &mut TestAppContext,
) {
    use crate::support::FaultProxy;
    use junto_client::state::view::ServerStatus;

    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&second));
    // One person owns both servers.
    let (_directory, mut owner) = named(cx, "Ada");
    owner.find_server(&first.address());
    owner.join_with_code(&first.owner_code().unwrap());
    owner.until_ready();
    owner.find_server(&proxy.address());
    owner.join_with_code(&second.owner_code().unwrap());
    owner.until("the second server is shown", |window| {
        let view = window.view();
        view.servers.len() == 2
            && view.servers[1].selected
            && view.servers[1].status == ServerStatus::Ready
            && view.workspace().is_some()
    });
    let shows = |window: &mut Window, name: &str| {
        window
            .view()
            .selected
            .is_some_and(|server| server.name == name && server.status == ServerStatus::Ready)
    };
    let logo_requests = |proxy: &FaultProxy| {
        proxy
            .all()
            .iter()
            .filter(|seen| seen.method == "PUT" && seen.path.contains("logo"))
            .count()
    };
    let files = tempfile::tempdir().unwrap();
    let chosen = files.path().join("logo.png");
    std::fs::write(&chosen, png(64, 200)).unwrap();

    // The person opens the file dialog on the page "Server" of the first server.
    owner.click("server-0");
    owner.until("the first server is shown", |window| {
        shows(window, "Acme Studio")
    });
    owner.open_administration();
    open_page(&mut owner, "admin-nav-server", shows_server_page);
    click_in_page(&mut owner, "logo-choose");
    assert!(owner.cx.did_prompt_for_paths());

    // The dialog does not stop the window. The person opens the same page of the second
    // server and chooses the file afterwards: nothing is sent to the second server.
    owner.click("admin-close");
    owner.click("server-1");
    owner.until("the second server is shown again", |window| {
        shows(window, "Harbor")
    });
    owner.open_administration();
    open_page(&mut owner, "admin-nav-server", shows_server_page);
    let file = chosen.clone();
    owner
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![file]));
    owner.cx.run_until_parked();
    assert!(owner.shows("logo-none") && !owner.shows("logo-picture"));
    assert_eq!(logo_requests(&proxy), 0);

    // A dialog that is answered on the page that opened it sends the file.
    click_in_page(&mut owner, "logo-choose");
    let file = chosen.clone();
    owner
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![file]));
    owner.until("the page of the second server shows the logo", |window| {
        window.shows("logo-picture")
    });
    assert_eq!(logo_requests(&proxy), 1);
}

#[gpui::test]
fn an_administrator_changes_the_limits_of_files_and_the_window_of_a_member_follows(
    cx: &mut TestAppContext,
) {
    use junto_client_core::protocol::message::AttachmentLimits;
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_owner_directory, mut owner) = named(cx, "Ada");
    owner.find_server(&server.address());
    owner.join_with_code(&server.owner_code().unwrap());
    owner.until_ready();
    let code = invite(&world, &owner.handle(), Vec::new());
    let (_member_directory, mut member) = named(cx, "Luca");
    member.find_server(&server.address());
    member.join_with_code(code.expose());
    member.until_ready();
    let limits = |window: &mut Window| match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Server(page),
            ..
        }) => page.limits.loaded().cloned(),
        _ => None,
    };

    owner.open_administration();
    open_page(&mut owner, "admin-nav-server", shows_server_page);
    let shown = owner.until_value("the page shows the limits of the server", limits);
    assert_eq!(
        shown,
        AttachmentLimits {
            bytes_per_file: 100_000_000,
            files_per_message: 10,
        }
    );
    assert!(owner.shows("limit-file-size-value") && owner.shows("limit-files-value"));

    // A number outside the range is not sent, and the form says why.
    click_in_page(&mut owner, "limit-file-size");
    assert!(owner.shows("limit-field"));
    owner.in_shell(|shell, window, cx| shell.type_admin_value("2000", window, cx));
    owner.click("admin-form-send");
    assert!(owner.shows("admin-form-invalid"));
    owner.in_shell(|shell, window, cx| shell.type_admin_value("25", window, cx));
    owner.click("admin-form-send");
    owner.until("the page shows the new largest file", |window| {
        limits(window).is_some_and(|held| held.bytes_per_file == 25_000_000)
            && !window.shows("limit-field")
    });

    // The other limit is changed with Enter; the largest file stays.
    click_in_page(&mut owner, "limit-files");
    owner.in_shell(|shell, window, cx| shell.type_admin_value("3", window, cx));
    owner.press("enter");
    let changed = AttachmentLimits {
        bytes_per_file: 25_000_000,
        files_per_message: 3,
    };
    owner.until("the page shows the new number of files", |window| {
        limits(window).as_ref() == Some(&changed) && !window.shows("limit-field")
    });
    assert!(!owner.shows("limit-refused"));
    assert_eq!(
        world.run(owner.handle().api().attachment_limits()).unwrap(),
        changed
    );

    // The composer of the member follows the event: the limits are checked before a send.
    member.until("the member holds the new limits", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.feed.as_ref())
            .and_then(|feed| feed.limits.clone())
            == Some(changed.clone())
    });
}
