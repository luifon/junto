//! The states of a server that a person must see, the settings, and the keyboard.

use std::time::Duration;

use gpui::TestAppContext;
use junto_client::shell::Open;
use junto_client::state::view::{Content, ServerStatus};
use junto_client::text;
use junto_client_core::Reach;
use junto_client_core::book::ConnectionMode;
use junto_client_core::flow::access::Notice;
use junto_client_core::protocol::account::RecoverAccount;

use crate::harness::{Window, World, member, named, open, open_with};
use crate::support::{Device, FaultProxy, logged_in};

#[gpui::test]
fn an_address_that_is_not_valid_or_does_not_answer_is_refused_in_the_server_book(
    cx: &mut TestAppContext,
) {
    let (_directory, mut window) = named(cx, "Luca");
    let mut refused = |address: &str| {
        window.enter_address(address);
        window.until("the address is refused", |window| {
            window.in_shell(|shell, _, _| shell.address_was_refused())
        });
        assert!(window.shows("address-help"));
        assert!(!window.shows("join-dialog"));
        assert!(window.view().servers.is_empty());
        window
            .in_shell(|shell, _, _| shell.address_error().map(str::to_owned))
            .unwrap()
    };
    assert_eq!(refused(""), "The address is empty.");
    assert_eq!(
        refused("harbor.example/chat"),
        "A server address has no user name, path, query or fragment."
    );
    assert_eq!(
        refused("http://harbor.example"),
        "A connection without TLS is accepted only for a server on this machine."
    );
    // A port of this machine where nothing listens.
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert_eq!(
        refused(&format!("http://127.0.0.1:{port}")),
        "No connection to the server. Check the address and the network connection."
    );
}

#[gpui::test]
fn a_ban_a_removal_and_a_forgotten_account_show_their_states(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let account_id = window.view().person.account_id.unwrap();
    let api = admin.handle.api();

    // The server ends the session of a banned account. The window shows the ban without
    // an action of the person.
    world.run(api.ban_account(account_id)).unwrap();
    window.until("the ban is shown", |window| {
        window.status() == Some(ServerStatus::Banned)
    });
    assert!(window.shows("banned"));
    assert_eq!(window.view().selected.unwrap().content, Content::State);
    assert!(window.view().servers[0].workspaces.is_empty());

    // A second start shows the ban from the server book before any contact.
    window.close();
    let checked_before = proxy.seen("/api/server").len();
    proxy.hold("/api/server");
    let mut window = open(cx, directory.path());
    window.until("the ban is shown again", |window| window.shows("banned"));
    window.until("the startup request is held", |_| {
        proxy.seen("/api/server").len() > checked_before
    });
    let held = std::time::Instant::now();
    while held.elapsed() < Duration::from_millis(200) {
        window.view();
        std::thread::sleep(Duration::from_millis(10));
    }
    proxy.release();
    window.until("the startup check has finished", |window| {
        let selected = window.view().selected;
        selected.is_some_and(|selected| {
            let checked = window.state.read_with(&window.cx, |state, _| {
                state.server(selected.id).is_some_and(|server| {
                    startup_check_finished(server.reach(), server.access().in_progress.is_some())
                })
            });
            checked && !selected.busy
        })
    });

    // "Check again" finds the unban, and the account is the same.
    world.run(api.unban_account(account_id)).unwrap();
    window.click("check-server");
    window.until_ready();
    assert_eq!(window.view().person.account_id, Some(account_id));

    // A removed account has no access; the window says so and offers the way back in.
    world.run(api.remove_account(account_id)).unwrap();
    window.until("the removal is shown", |window| {
        window.status() == Some(ServerStatus::NoAccount(Some(Notice::Removed)))
    });
    assert!(window.shows("no-account") && window.shows("join-server"));

    // Joining again with a code creates a new account with the same key.
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.until("the rejoin form is drawn", |window| {
        window.shows("join-with-code")
    });
    window.join_with_code(code.expose());
    window.until_ready();
    let again = window.view().person.account_id.unwrap();
    assert_ne!(again, account_id);

    // A banned person can forget the account: the key goes and the server stays.
    world.run(api.ban_account(again)).unwrap();
    window.until("the second ban is shown", |window| window.shows("banned"));
    window.click("forget-account");
    assert!(window.shows("confirm-forget"));
    window.click("confirm");
    window.until("the account is forgotten", |window| {
        window.status() == Some(ServerStatus::NoAccount(None))
    });
    assert_eq!(window.view().servers.len(), 1);
}

#[gpui::test]
fn a_recovery_is_shown_until_the_person_states_that_it_was_seen(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let account_id = window.view().person.account_id.unwrap();
    assert!(!window.shows("recovery-notice"));

    // The owner gives a new device access to the account. The server ends every session
    // of the account; this device logs in again and is told of the recovery.
    let replacement = world.run(async {
        let device = Device::new();
        let handle = device.add(&server.address()).await;
        handle.connect().await.unwrap();
        admin
            .handle
            .api()
            .recover_account(
                account_id,
                &RecoverAccount {
                    public_key: handle.device_public_key().unwrap(),
                    device_name: "Replacement".to_owned(),
                },
            )
            .await
            .unwrap();
        handle.connect().await.unwrap();
        assert_eq!(logged_in(&handle), account_id);
        (device, handle)
    });
    window.until("the recovery is shown", |window| {
        window.shows("recovery-notice")
    });
    window.until_ready();
    assert_eq!(window.view().selected.unwrap().recoveries.len(), 1);
    assert!(window.view().servers[0].has_recovery);

    // A notice that was drawn is not acknowledged: a second start shows it again.
    window.close();
    let mut window = open(cx, directory.path());
    window.until("the recovery is shown again", |window| {
        window.shows("recovery-notice")
    });

    // The button sends the acknowledgement, and the notice goes for good.
    window.click("recovery-seen");
    window.until("the recovery is gone", |window| {
        !window.shows("recovery-notice") && window.view().selected.unwrap().recoveries.is_empty()
    });
    assert!(!window.shows("recovery-error"));
    window.close();
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert!(!window.shows("recovery-notice"));
    // The other device of the account keeps its own notice.
    let other = world.run(replacement.1.api().recovery_notices()).unwrap();
    assert_eq!(other.len(), 1);
}

#[gpui::test]
fn a_lost_connection_keeps_the_last_state_and_a_stopped_server_is_tried_again(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let mut server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let channels = window.channel_names();
    assert!(!window.shows("stale-state"));

    // The connection of the event stream fails and no new one is accepted. The lists stay
    // and the window says that they are the last known state.
    proxy.block_streams(true);
    proxy.cut_streams();
    window.until("the interruption is shown", |window| {
        window.status() == Some(ServerStatus::Reconnecting)
    });
    assert!(window.shows("stale-state"));
    assert_eq!(window.channel_names(), channels);

    // The client connects again by itself.
    proxy.block_streams(false);
    window.until_ready();
    assert!(!window.shows("stale-state"));

    // A start while the server is down: the window says that the server cannot be
    // reached, and nothing of the server book is lost.
    window.close();
    server.stop();
    let mut window = open(cx, directory.path());
    window.until("the server is shown as unreachable", |window| {
        window.status() == Some(ServerStatus::Unreachable)
    });
    assert!(window.shows("unreachable") && window.shows("check-server"));
    assert_eq!(window.view().servers.len(), 1);

    // The server returns. Nothing contacts it before the interval of the application has
    // passed; then the application logs in by itself.
    world.run(server.restart());
    window.cx.executor().advance_clock(Duration::from_secs(21));
    window.until_ready();
    assert_eq!(window.channel_names(), channels);

    // A start while the server is down again, and the server returns: "Try again"
    // contacts it at once, without the interval.
    window.close();
    server.stop();
    let mut window = open(cx, directory.path());
    window.until("the server is shown as unreachable again", |window| {
        window.status() == Some(ServerStatus::Unreachable)
    });
    world.run(server.restart());
    window.click("check-server");
    window.until_ready();
    assert_eq!(window.channel_names(), channels);
}

#[gpui::test]
fn the_settings_keep_the_name_the_scale_and_the_form_of_the_server_book(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    window.choose_name("Luca");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        100
    );

    window.click("app-settings");
    assert!(window.shows("settings-dialog"));
    // A name that no server accepts is refused before anything is stored.
    window.in_shell(|shell, window, cx| shell.type_name("   ", window, cx));
    window.click("save-name");
    assert!(window.shows("name-error"));
    assert_eq!(window.view().person.name, "Luca");
    window.in_shell(|shell, window, cx| shell.type_name("Luca R.", window, cx));
    window.click("save-name");
    assert!(!window.shows("name-error"));
    assert_eq!(window.view().person.name, "Luca R.");

    // The scale changes the size of one rem at once.
    window.click("scale-4");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        150
    );
    window.cx.run_until_parked();
    assert_eq!(
        window.cx.update(|window, _| window.rem_size()),
        gpui::px(24.0)
    );
    window.click("dialog-close");
    assert!(!window.shows("settings-dialog"));

    // The server book collapses to icons and expands again.
    let collapsed = window.shows("expand-server-book");
    let toggle = if collapsed {
        "expand-server-book"
    } else {
        "collapse-server-book"
    };
    window.click(toggle);
    assert_eq!(window.shows("expand-server-book"), !collapsed);

    // A second start has the name, the scale and the form of the server book.
    let stored = window
        .state
        .read_with(&window.cx, |state, _| state.settings());
    window.close();
    let mut window = open(cx, directory.path());
    assert!(!window.shows("welcome-dialog"));
    assert_eq!(window.view().person.name, "Luca R.");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings()),
        stored
    );
    assert_eq!(stored.scale_percent, 150);

    // A scale that a start argument gives is the scale of that start only.
    window.close();
    let mut window = open_with(cx, directory.path(), Some(90));
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().scale_percent),
        90
    );
    // Another setting that changes in that start does not store the scale of the start.
    let toggle = if window.shows("expand-server-book") {
        "expand-server-book"
    } else {
        "collapse-server-book"
    };
    window.click(toggle);
    window.close();
    let window = open(cx, directory.path());
    let settings = window
        .state
        .read_with(&window.cx, |state, _| state.settings());
    assert_eq!(settings.scale_percent, 150);
    assert_ne!(settings.server_book_collapsed, stored.server_book_collapsed);
}

/// The settings file of the data directory that the harness opens.
fn settings_file(directory: &std::path::Path) -> std::path::PathBuf {
    directory.join("junto").join("settings.json")
}

#[gpui::test]
fn a_change_that_was_accepted_is_written_after_the_window_has_closed(cx: &mut TestAppContext) {
    let (directory, mut window) = named(cx, "Luca");
    let before = window
        .state
        .read_with(&window.cx, |state, _| state.settings());
    assert_eq!(before.scale_percent, 100);

    // Two settings change, and the window closes before a write has started.
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_scale(125, cx);
            state.set_server_book_collapsed(!before.server_book_collapsed, cx);
        });
    });
    drop(state);
    window.close();
    let mut window = open(cx, directory.path());
    let stored = window
        .state
        .read_with(&window.cx, |state, _| state.settings());
    assert_eq!(stored.scale_percent, 125);
    assert_eq!(stored.server_book_collapsed, !before.server_book_collapsed);

    // The end of the application waits for a write that was accepted.
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| state.set_scale(150, cx));
        junto_client::state::finish_writes(cx);
    });
    let file = std::fs::read_to_string(settings_file(directory.path())).unwrap();
    let file: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!(file["scale_percent"], 150);
}

#[gpui::test]
fn a_setting_that_was_not_written_is_told_until_it_is_in_the_file(cx: &mut TestAppContext) {
    let (directory, mut window) = named(cx, "Luca");
    let file = settings_file(directory.path());
    let failure = |window: &mut Window| {
        window.cx.run_until_parked();
        window.state.read_with(&window.cx, |state, _| {
            state.settings_error().map(str::to_owned)
        })
    };

    // The settings file cannot be written: a directory has its name.
    let _ = std::fs::remove_file(&file);
    std::fs::create_dir(&file).unwrap();
    let state = window.state.clone();
    window
        .cx
        .update(|_, cx| state.update(cx, |state, cx| state.set_scale(150, cx)));
    assert!(failure(&mut window).is_some());
    window.click("app-settings");
    assert!(window.shows("settings-error"));
    window.click("dialog-close");

    // The file can be written again. The write of another setting writes the scale too,
    // and only then the failure is no longer told.
    std::fs::remove_dir(&file).unwrap();
    let collapsed = window.state.read_with(&window.cx, |state, _| {
        state.settings().server_book_collapsed
    });
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_server_book_collapsed(!collapsed, cx)
        });
    });
    assert_eq!(failure(&mut window), None);
    drop(state);
    window.close();
    let window = open(cx, directory.path());
    let stored = window
        .state
        .read_with(&window.cx, |state, _| state.settings());
    assert_eq!(stored.scale_percent, 150);
    assert_eq!(stored.server_book_collapsed, !collapsed);
}

#[gpui::test]
fn a_name_that_was_stored_does_not_mark_other_text_as_saved(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Luca");
    window.click("app-settings");
    let saved = |window: &mut Window| window.in_shell(|shell, _, _| shell.name_is_marked_saved());

    // The person saves a name and types another one before the file is written.
    let shell = window.shell.clone();
    window.cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.type_name("Alice", window, cx);
            shell.submit_name(window, cx);
            shell.type_name("Bob", window, cx);
        });
    });
    window.until("the name is stored", |window| {
        window.view().person.name == "Alice"
    });
    assert!(!saved(&mut window));

    // A name that is still in the field when it was stored is marked.
    window.in_shell(|shell, window, cx| shell.type_name("  Bob ", window, cx));
    window.click("save-name");
    window.until("the name is stored", |window| {
        window.view().person.name == "Bob"
    });
    assert!(saved(&mut window));
}

#[gpui::test]
fn a_removal_that_is_asked_for_twice_runs_once(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let _owner = world.owner(&server);
    let (_directory, mut window) = named(cx, "Luca");
    // The server is in the server book without an account.
    window.find_server(&server.address());
    window.join_with_code("AAAA-BBBB-CCCC-DDDD");
    window.until("the refusal is shown", |window| window.shows("join-error"));
    window.click("dialog-close");
    let harbor = window.view().servers[0].id;
    let state = window.state.clone();

    // The removal is asked for twice before the first one has ended: one removal runs,
    // and nothing is told as failed.
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            assert!(state.remove_server(harbor, cx));
            assert!(!state.remove_server(harbor, cx));
            assert!(state.pending().removing.contains(&harbor));
        });
    });
    window.until("the server is removed", |window| {
        window.view().servers.is_empty()
    });
    let pending = state.read_with(&window.cx, |state, _| state.pending().clone());
    assert!(pending.remove_failed.is_empty(), "{pending:?}");
    assert!(pending.removing.is_empty());
}

#[gpui::test]
fn the_connection_mode_is_written_in_the_order_of_the_changes(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let _owner = world.owner(&server);
    let (directory, mut window) = named(cx, "Luca");
    window.find_server(&server.address());
    window.join_with_code("AAAA-BBBB-CCCC-DDDD");
    window.until("the refusal is shown", |window| window.shows("join-error"));
    window.click("dialog-close");
    let harbor = window.view().servers[0].id;
    let mode = |window: &Window| {
        window.state.read_with(&window.cx, |state, _| {
            state.client().servers()[0].entry().connection_mode
        })
    };
    assert_eq!(mode(&window), ConnectionMode::Always);

    // Three changes before one is written: the last one stays.
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_keep_connected(harbor, false, cx);
            state.set_keep_connected(harbor, true, cx);
            state.set_keep_connected(harbor, false, cx);
        });
    });
    drop(state);
    window.until("the connection mode is written", |window| {
        mode(window) == ConnectionMode::WhenSelected
    });
    window.cx.run_until_parked();
    assert_eq!(mode(&window), ConnectionMode::WhenSelected);
    window.close();
    let window = open(cx, directory.path());
    assert_eq!(mode(&window), ConnectionMode::WhenSelected);
}

#[gpui::test]
fn the_keyboard_reaches_the_menus_and_the_dialogs(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let handle = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    assert_eq!(handle.entry().connection_mode, ConnectionMode::Always);

    // The menu of the server takes the focus. The arrow keys move between its items, and
    // Enter chooses one.
    window.click("server-menu-button");
    assert!(window.shows("server-menu"));
    window.press("down");
    window.press("enter");
    window.until("the connection mode changed", |_| {
        handle.entry().connection_mode == ConnectionMode::WhenSelected
    });
    assert!(window.shows("server-menu"), "a switch leaves the menu open");
    assert!(!window.view().selected.unwrap().keep_connected);
    window.press("space");
    window.until("the connection mode changed back", |_| {
        handle.entry().connection_mode == ConnectionMode::Always
    });

    // Escape closes the menu.
    window.press("escape");
    assert!(!window.shows("server-menu"));

    // A click on the button of the open menu closes it, and the next click opens it.
    window.click("server-menu-button");
    assert!(window.shows("server-menu"));
    window.click("server-menu-button");
    assert!(!window.shows("server-menu"));
    window.click("server-menu-button");
    assert!(window.shows("server-menu"));
    // A click outside the menu closes it.
    window.click("channel-title");
    assert!(!window.shows("server-menu"));

    // The last item, after the two for the notification settings, opens the question,
    // and Escape answers it with "no".
    window.click("server-menu-button");
    window.press("down down down down enter");
    assert!(window.shows("confirm-remove"));
    window.press("escape");
    assert!(!window.shows("confirm-remove"));
    assert_eq!(window.view().servers.len(), 1);

    // Tab stays inside an open dialog.
    window.click("app-settings");
    assert!(window.shows("settings-dialog"));
    for _ in 0..12 {
        window.press("tab");
        assert!(
            window.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)),
            "the focus left the dialog"
        );
    }
    window.press("escape");
    assert!(!window.shows("settings-dialog"));

    // The menu of a server in the collapsed server book: the keyboard opens it, moves
    // into it and chooses a workspace.
    window.click("collapse-server-book");
    assert!(window.shows("expand-server-book"));
    window.in_shell(|shell, window, cx| shell.open(Open::ServerHoverMenu, window, cx));
    assert!(window.shows("server-hover-menu"));
    window.press("down enter");
    assert!(!window.shows("server-hover-menu"));
    assert!(window.view().workspace().is_some());

    // The address field is reached from the collapsed server book through its button.
    window.click("add-server");
    assert!(window.shows("address-field") && window.shows("collapse-server-book"));
    assert_eq!(text::ADDRESS_HELP, "Enter the address your admin gave you.");
}

#[gpui::test]
fn a_long_name_of_a_server_moves_no_control_of_a_question(cx: &mut TestAppContext) {
    let world = World::new();
    // The longest name that the client keeps of a server, without a place to break it.
    let name = "W".repeat(64);
    let server = world.server(&name);
    let (_directory, mut window) = named(cx, "Luca");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    assert_eq!(window.view().servers[0].name, name);

    let inside = |window: &mut crate::harness::Window, outer: &'static str, inner: &'static str| {
        let outer = window.cx.debug_bounds(outer).unwrap();
        let inner = window.cx.debug_bounds(inner).unwrap();
        outer.contains(&inner.origin) && outer.contains(&inner.bottom_right())
    };
    window.click("server-menu-button");
    window.click("menu-remove-server");
    assert!(window.shows("confirm-remove"));
    for control in ["dialog-close", "cancel", "confirm"] {
        assert!(
            inside(&mut window, "confirm-remove", control),
            "{control} is outside the dialog"
        );
    }
    window.click("dialog-close");
    assert!(!window.shows("confirm-remove"));
    assert_eq!(window.view().servers.len(), 1);
}

fn startup_check_finished(reach: Reach, busy: bool) -> bool {
    reach != Reach::Unknown && !busy
}
