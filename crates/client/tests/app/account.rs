//! The account layer against the real server (ADR 0781): the name and the picture of the
//! account, and its devices.

use gpui::TestAppContext;
use junto_client::state::account::{
    AccountPage, AccountView, AskKind, DeviceLine, DeviceList, PageView, ProfilePage,
};
use junto_client::state::view::ServerStatus;
use junto_client::text;
use junto_client_core::ServerHandle;
use junto_client_core::flow::access::{AccessAction, AccessState, Notice};
use junto_client_core::profile::{InvalidAvatar, avatar_hash};
use junto_client_core::protocol::account::DeviceOrigin;
use junto_client_core::protocol::ids::DeviceId;

use crate::harness::{Window, World, member, selector};
use crate::support::{Device, FaultProxy, TestServer, logged_in};

const ACCOUNT: &str = "/api/v1/account";
const AVATAR: &str = "/api/v1/account/avatar";

/// A still PNG image as far as the server reads it: the header chunk with the size, and
/// the start of the image data.
fn png(width: u32, height: u32, padding: usize) -> Vec<u8> {
    let mut image = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    image.extend_from_slice(&width.to_be_bytes());
    image.extend_from_slice(&height.to_be_bytes());
    image.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    image.extend_from_slice(&(padding as u32).to_be_bytes());
    image.extend_from_slice(b"IDAT");
    image.resize(image.len() + padding + 4, 0);
    image
}

impl Window {
    /// The account layer as the window drew it last. Absent while the layer is closed.
    pub(crate) fn account(&mut self) -> Option<AccountView> {
        self.cx.run_until_parked();
        self.shell.read_with(&self.cx, |shell, _| {
            shell
                .account_drawn()
                .filter(|_| shell.account_is_open())
                .cloned()
        })
    }

    /// Opens the account layer through the menu of the server.
    #[track_caller]
    pub(crate) fn open_account(&mut self) {
        self.click("server-menu-button");
        self.click("menu-account");
        assert!(self.shows("account"));
    }

    /// Opens the account layer at the page of the devices and waits for the list.
    #[track_caller]
    pub(crate) fn open_devices(&mut self) {
        self.open_account();
        self.click("account-nav-devices");
        self.until("the devices are listed", |window| {
            !window.devices().is_empty()
        });
    }

    fn profile(&mut self) -> Option<ProfilePage> {
        match self.account()?.page {
            PageView::Profile(page) => Some(page),
            PageView::Devices(_) | PageView::Recoveries(_) | PageView::Departure(_) => None,
        }
    }

    /// The rows of the devices page. Empty while the list is not read.
    pub(crate) fn devices(&mut self) -> Vec<DeviceLine> {
        match self.account().map(|view| view.page) {
            Some(PageView::Devices(page)) => match page.list {
                DeviceList::Listed { rows, .. } => rows,
                DeviceList::Reading | DeviceList::Failed(_) => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn only_device(&mut self) -> bool {
        matches!(
            self.account().map(|view| view.page),
            Some(PageView::Devices(page))
                if matches!(page.list, DeviceList::Listed { only: true, .. })
        )
    }

    fn refused(&mut self, what: AskKind) -> Option<String> {
        self.account()
            .and_then(|view| view.refused(what).map(str::to_owned))
    }

    /// The server of the window as the client core holds it.
    pub(crate) fn handle(&mut self) -> ServerHandle {
        self.state
            .read_with(&self.cx, |state, _| state.client().servers())
            .remove(0)
    }
}

/// A second device of the account of a window, without a window: it asks for a link, and
/// the device of the window approves it through the API.
pub(crate) fn second_device(
    world: &World,
    server: &TestServer,
    first: &ServerHandle,
) -> (Device, ServerHandle) {
    let device = Device::new();
    let handle = world.run(async {
        let handle = device.add(&server.address()).await;
        handle
            .dispatch(AccessAction::RequestLink {
                device_name: "Phone".to_owned(),
            })
            .await
            .unwrap();
        let AccessState::WaitingForLink {
            code, public_key, ..
        } = handle.access().state
        else {
            panic!("the second device does not wait for a link");
        };
        first
            .api()
            .approve_device_link(code, public_key)
            .await
            .unwrap();
        handle.dispatch(AccessAction::Check).await.unwrap();
        handle
    });
    logged_in(&handle);
    (device, handle)
}

/// The last login that the devices page shows for a device.
fn other_login(window: &mut Window, device_id: DeviceId) -> Option<time::OffsetDateTime> {
    window
        .devices()
        .into_iter()
        .find(|row| row.id == device_id)
        .and_then(|row| row.last_login)
}

fn named_row(device_id: DeviceId, part: &str) -> String {
    format!("device-{part}-{device_id}")
}

#[gpui::test]
fn the_name_of_the_account_changes_in_the_window_of_another_member(
    cx: &mut TestAppContext,
    other_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_other_directory, mut other) =
        member(other_cx, &world, &server.address(), &admin, "Bruno");
    other.click("members-button");
    other.until("the other member lists the members", |other| {
        other.shows_member("Luca")
    });

    // The layer opens from the menu of the server, at the profile, with the name that
    // the server holds in its field.
    window.open_account();
    let view = window.account().unwrap();
    assert_eq!(view.selected, AccountPage::Profile);
    assert_eq!(view.server_name, "Acme Studio");
    assert_eq!(
        window.profile(),
        Some(ProfilePage {
            name: "Luca".to_owned(),
            picture: None,
        })
    );
    assert!(window.shows("account-name-field") && window.shows("account-picture"));

    // A name that no server takes is refused at the field, and nothing is sent.
    window.in_shell(|shell, window, cx| shell.type_account_name("   ", window, cx));
    window.click("account-name-save");
    assert_eq!(
        window.refused(AskKind::SaveName).as_deref(),
        Some("Type a name.")
    );
    assert!(window.shows("account-name-refused"));
    window.in_shell(|shell, window, cx| shell.type_account_name(&"x".repeat(65), window, cx));
    assert!(
        !window.shows("account-name-refused"),
        "a refusal belongs to the text that was refused"
    );
    window.click("account-name-save");
    assert_eq!(
        window.refused(AskKind::SaveName).as_deref(),
        Some("A name has at most 64 characters.")
    );
    assert_eq!(window.view().person.name, "Luca");

    // The new name reaches the server, this window and the window of the other member.
    window.in_shell(|shell, window, cx| shell.type_account_name(" Luca Moreira ", window, cx));
    window.click("account-name-save");
    window.until("the profile page shows the new name", |window| {
        window
            .profile()
            .is_some_and(|page| page.name == "Luca Moreira")
    });
    assert_eq!(window.refused(AskKind::SaveName), None);
    assert_eq!(window.view().person.name, "Luca Moreira");
    other.until("the other member sees the new name", |other| {
        other.shows_member("Luca Moreira")
    });
    assert!(!other.shows_member("Luca"));
    let handle = window.handle();
    assert_eq!(
        world.run(handle.api().own_account()).unwrap().name,
        "Luca Moreira"
    );

    // Escape closes the layer.
    window.press("escape");
    assert!(!window.shows("account"));
    assert_eq!(window.account(), None);
}

#[gpui::test]
fn a_picture_is_checked_before_it_is_sent_and_set_and_removed(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    // The window reaches the server through a proxy that records each request.
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let files = tempfile::tempdir().unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let file = files.path().join(name);
        std::fs::write(&file, bytes).unwrap();
        file
    };
    let small = png(64, 48, 100);
    // 300 KiB with the first bytes of a PNG image.
    let large = write("large.png", &png(64, 48, 300 * 1024));
    let text_file = write("notes.txt", b"These bytes are no image.");
    let wide = write("wide.png", &png(2000, 2000, 100));
    let picture = write("picture.png", &small);
    let puts = |proxy: &FaultProxy| {
        proxy
            .seen(AVATAR)
            .iter()
            .filter(|seen| seen.method == "PUT")
            .count()
    };

    window.open_account();
    assert!(window.shows("account-picture-choose"));
    assert_eq!(window.profile().unwrap().picture, None);

    // A file of 300 KiB and a file that is no image are refused before any request.
    for (file, invalid) in [
        (large, InvalidAvatar::TooLarge),
        (text_file, InvalidAvatar::Unsupported),
    ] {
        window.in_shell(|shell, _, cx| shell.set_account_picture_path(file, cx));
        window.until("the file is refused", |window| {
            window.refused(AskKind::SetPicture).is_some()
        });
        assert_eq!(
            window.refused(AskKind::SetPicture).as_deref(),
            Some(text::account::invalid_picture(invalid))
        );
        assert!(window.shows("account-picture-refused"));
    }
    // A request of the same client that is answered now is behind every earlier one.
    let handle = window.handle();
    world.run(handle.api().own_account()).unwrap();
    assert_eq!(puts(&proxy), 0, "a refused file was sent");

    // The server reads the size in pixels: its refusal is told at the same control.
    window.in_shell(|shell, _, cx| shell.set_account_picture_path(wide, cx));
    window.until("the server refused the picture", |window| {
        puts(&proxy) == 1 && window.refused(AskKind::SetPicture).is_some()
    });
    let refused = window.refused(AskKind::SetPicture).unwrap();
    assert!(refused.contains("1024 by 1024 pixels"), "{refused}");
    assert_eq!(window.profile().unwrap().picture, None);

    // A small PNG image is stored, and the page describes what the server holds.
    window.in_shell(|shell, _, cx| shell.set_account_picture_path(picture, cx));
    window.until("the server holds the picture", |window| {
        window.profile().is_some_and(|page| page.picture.is_some())
    });
    let held = window.profile().unwrap().picture.unwrap();
    assert_eq!(held.hash, avatar_hash(&small));
    assert_eq!((held.width, held.height), (64, 48));
    assert_eq!(held.bytes as usize, small.len());
    assert_eq!(window.refused(AskKind::SetPicture), None);
    assert!(!window.shows("account-picture-refused"));
    assert_eq!(puts(&proxy), 2);
    let own = world.run(handle.api().own_account()).unwrap();
    assert_eq!(own.avatar.unwrap().hash, avatar_hash(&small));

    // "Remove picture" removes it on the server.
    window.click("account-picture-remove");
    window.until("the picture is removed", |window| {
        window.profile().is_some_and(|page| page.picture.is_none())
    });
    assert_eq!(world.run(handle.api().own_account()).unwrap().avatar, None);
}

#[gpui::test]
fn the_profile_page_sends_one_request_at_a_time(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let files = tempfile::tempdir().unwrap();
    let picture = files.path().join("picture.png");
    std::fs::write(&picture, png(64, 48, 100)).unwrap();
    let sent = |proxy: &FaultProxy, path: &str, method: &str| {
        proxy
            .seen(path)
            .iter()
            .filter(|seen| seen.method == method)
            .count()
    };

    // The request for the name waits at the proxy.
    window.open_account();
    proxy.hold(ACCOUNT);
    window.in_shell(|shell, window, cx| shell.type_account_name("Luca Moreira", window, cx));
    window.click("account-name-save");
    window.until("the name is on its way", |_| {
        sent(&proxy, ACCOUNT, "PATCH") == 1
    });
    assert!(window.account().unwrap().sending(AskKind::SaveName));

    // A file that the dialog returns now is not sent, and the name is not sent a second
    // time. The page shows the name as sending.
    let chosen = picture.clone();
    window.in_shell(|shell, _, cx| shell.set_account_picture_path(chosen, cx));
    window.in_shell(|shell, window, cx| shell.submit_account_name(window, cx));
    let view = window.account().unwrap();
    assert!(view.sending(AskKind::SaveName));
    assert!(!view.sending(AskKind::SetPicture));
    // The page says at the button that the file was not sent.
    assert_eq!(
        window.in_shell(|shell, _, _| shell.account_picture_refusal()),
        Some(text::account::PAGE_BUSY)
    );
    assert!(window.shows("account-picture-refused"));

    proxy.release();
    window.until("the server holds the new name", |window| {
        window
            .profile()
            .is_some_and(|page| page.name == "Luca Moreira")
            && window.account().is_some_and(|view| view.ask.is_none())
    });
    assert_eq!(sent(&proxy, ACCOUNT, "PATCH"), 1);
    assert_eq!(sent(&proxy, AVATAR, "PUT"), 0);

    // The page takes the picture after the answer.
    window.in_shell(|shell, _, cx| shell.set_account_picture_path(picture, cx));
    window.until("the server holds the picture", |window| {
        window.profile().is_some_and(|page| page.picture.is_some())
    });
    assert_eq!(sent(&proxy, AVATAR, "PUT"), 1);
    assert_eq!(
        window.in_shell(|shell, _, _| shell.account_picture_refusal()),
        None
    );
    assert!(!window.shows("account-picture-refused"));
}

#[gpui::test]
fn a_device_is_renamed_and_removed_and_the_list_follows_the_server(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let first = window.handle();

    // One device: this one. It cannot be removed, and its button takes no click.
    window.open_devices();
    let rows = window.devices();
    assert_eq!(rows.len(), 1);
    let own = rows[0].clone();
    assert!(own.this_device && window.only_device());
    assert_eq!(own.name, text::device_name());
    assert_eq!(own.origin, DeviceOrigin::Invite);
    assert!(own.last_login.is_some());
    window.click_named(named_row(own.id, "remove"));
    assert!(!window.shows("device-remove-confirm"));

    // The server refuses the removal of the last device too; its refusal is told at the
    // row.
    window.in_shell(|shell, _, cx| {
        shell
            .state()
            .clone()
            .update(cx, |state, cx| state.remove_device(own.id, cx));
    });
    window.until("the refusal of the server is shown", |window| {
        window.shows("device-refused")
    });
    assert_eq!(
        window.refused(AskKind::RemoveDevice(own.id)).as_deref(),
        Some(text::account::LAST_DEVICE)
    );
    assert_eq!(world.run(first.api().devices()).unwrap().len(), 1);

    // A device that is added elsewhere appears without an action in the window.
    let (_second_directory, second) = second_device(&world, &server, &first);
    window.until("the new device is listed", |window| {
        window.devices().len() == 2
    });
    assert!(!window.only_device());
    let other = window
        .devices()
        .into_iter()
        .find(|row| !row.this_device)
        .unwrap();
    assert_eq!(other.name, "Phone");
    assert_eq!(other.origin, DeviceOrigin::Link);

    // A new name, typed in the row of the device.
    window.click_named(named_row(other.id, "rename"));
    assert!(window.shows("device-name-field"));
    window.in_shell(|shell, window, cx| shell.type_device_name(&"n".repeat(65), window, cx));
    window.click("device-form-send");
    assert_eq!(
        window.refused(AskKind::RenameDevice(other.id)).as_deref(),
        Some("A name has at most 64 characters.")
    );
    assert!(
        window.shows("device-name-field"),
        "a refusal keeps the form"
    );
    window.in_shell(|shell, window, cx| shell.type_device_name("Pocket phone", window, cx));
    window.click("device-form-send");
    window.until("the device has its new name", |window| {
        window
            .devices()
            .iter()
            .any(|row| row.id == other.id && row.name == "Pocket phone")
            && !window.shows("device-name-field")
    });
    let listed = world.run(first.api().devices()).unwrap();
    assert!(
        listed
            .iter()
            .any(|device| device.id == other.id && device.name == "Pocket phone")
    );

    // A name that another device gives reaches the list by the event of the server.
    world
        .run(second.api().rename_device(own.id, "Desk".to_owned()))
        .unwrap();
    window.until("the name that the other device gave is listed", |window| {
        window
            .devices()
            .iter()
            .any(|row| row.id == own.id && row.name == "Desk")
    });

    // A login of the other device reaches its row by the event of the server: the page
    // shows its new last login without an act.
    let before = other_login(&mut window, other.id);
    let logged_in_at = world.run(async {
        second.dispatch(AccessAction::LogOut).await.unwrap();
        second.dispatch(AccessAction::LogIn).await.unwrap();
        second
            .api()
            .devices()
            .await
            .unwrap()
            .into_iter()
            .find(|device| device.id == other.id)
            .and_then(|device| device.last_login_at)
            .unwrap()
    });
    assert!(Some(logged_in_at) > before);
    window.until("the new login of the other device is listed", |window| {
        other_login(window, other.id) == Some(logged_in_at)
    });

    // The removal asks first.
    window.click_named(named_row(other.id, "remove"));
    assert!(window.shows("device-remove-confirm"));
    window.click("device-remove-cancel");
    assert!(!window.shows("device-remove-confirm"));
    assert_eq!(world.run(first.api().devices()).unwrap().len(), 2);
    window.click_named(named_row(other.id, "remove"));
    window.click("device-remove-confirm");
    window.until("the device is removed", |window| {
        window.devices().len() == 1 && !window.shows("device-remove-confirm")
    });
    assert!(window.only_device());
    assert_eq!(world.run(first.api().devices()).unwrap().len(), 1);
    // The removed device has no access any more.
    world
        .run(second.dispatch(AccessAction::Check))
        .unwrap_or_default();
    assert!(!matches!(
        second.access().state,
        AccessState::LoggedIn { .. }
    ));
}

#[gpui::test]
fn the_removal_of_this_device_ends_its_session_and_closes_the_layer(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let first = window.handle();
    let (_second_directory, second) = second_device(&world, &server, &first);

    window.open_devices();
    window.until("both devices are listed", |window| {
        window.devices().len() == 2
    });
    let own = window
        .devices()
        .into_iter()
        .find(|row| row.this_device)
        .unwrap();
    window.click_named(named_row(own.id, "remove"));
    assert!(window.shows("device-remove-confirm"));
    window.click("device-remove-confirm");

    // The server ends the session of this device: the layer has no account to show, and
    // the server shows that the device was removed.
    window.until("the layer closes", |window| !window.shows("account"));
    window.until("the server shows the removal", |window| {
        window.status() == Some(ServerStatus::NoAccount(Some(Notice::Removed)))
    });
    assert_eq!(window.account(), None);
    let left = world.run(second.api().devices()).unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].current);
}

/// The answer to the new name of a device closes the form, and the row of the device
/// under it keeps its place (ADR 0730).
#[gpui::test]
fn an_answer_to_the_name_of_a_device_moves_no_row_under_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let first = window.handle();
    let (_second_directory, _second) = second_device(&world, &server, &first);
    window.open_devices();
    window.until("both devices are listed", |window| {
        window.devices().len() == 2
    });
    let rows = window.devices();
    let (upper, lower) = (rows[0].id, rows[1].id);
    let below = selector(named_row(lower, "row"));

    // The name reaches the server, and its answer waits at the proxy.
    window.click_named(named_row(upper, "rename"));
    proxy.hold_answer_ending("PATCH", &format!("/account/devices/{upper}"));
    window.in_shell(|shell, window, cx| shell.type_device_name("Pocket phone", window, cx));
    window.click("device-form-send");
    window.until("the answer waits and the new name is listed", |window| {
        proxy.answers_waiting() == 1
            && window
                .devices()
                .iter()
                .any(|row| row.id == upper && row.name == "Pocket phone")
    });
    let before = window.cx.debug_bounds(below).unwrap();
    proxy.release();
    window.until("the form is closed", |window| {
        !window.shows("device-name-field")
    });
    assert_eq!(window.cx.debug_bounds(below).unwrap(), before);
}

/// At the largest interface scale in the narrowest window the help sentence of the form of
/// a device name wraps, and the form is taller than the row of the device. The answer to
/// the new name closes the form, and the row of the device under it keeps its place: the
/// row keeps the height of its form (ADR 0730).
#[gpui::test]
fn an_answer_to_the_name_of_a_device_in_a_narrow_window_moves_no_row_under_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let first = window.handle();
    let (_second_directory, _second) = second_device(&world, &server, &first);
    window.open_devices();
    window.until("both devices are listed", |window| {
        window.devices().len() == 2
    });
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_scale(junto_client::theme::SCALE_MAX, cx);
        });
    });
    window
        .cx
        .simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0)));
    window.cx.run_until_parked();
    let rows = window.devices();
    let (upper, lower) = (rows[0].id, rows[1].id);
    let (row, below) = (
        selector(named_row(upper, "row")),
        selector(named_row(lower, "row")),
    );
    let without = window.cx.debug_bounds(row).unwrap().size.height;
    window.click_named(named_row(upper, "rename"));
    window.until("the form is open", |window| {
        window.shows("device-name-field")
    });
    let with = window.cx.debug_bounds(row).unwrap().size.height;
    assert!(
        with > without,
        "the form ({with:?}) is taller than the row of the device ({without:?})"
    );
    let before = window.cx.debug_bounds(below).unwrap();

    // The name reaches the server, and its answer waits at the proxy.
    proxy.hold_answer_ending("PATCH", &format!("/account/devices/{upper}"));
    window.in_shell(|shell, window, cx| shell.type_device_name("Pocket phone", window, cx));
    window.click("device-form-send");
    window.until("the answer waits and the new name is listed", |window| {
        proxy.answers_waiting() == 1
            && window
                .devices()
                .iter()
                .any(|row| row.id == upper && row.name == "Pocket phone")
    });
    proxy.release();
    window.until("the form is closed", |window| {
        !window.shows("device-name-field")
    });
    assert_eq!(window.cx.debug_bounds(below).unwrap(), before);
}
