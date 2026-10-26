//! Update controls use a substitute installer and never touch a system bundle.
use crate::harness::open_with_updates;
use gpui::TestAppContext;
use junto_client::state::book::CheckShown;
use junto_client::text::book as txt;
use junto_client_core::releases::install::Previous;
use junto_client_core::updates::{FakeSource, Release, UpdateError, Version};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[gpui::test]
fn update_notice_and_install_control_run_once_and_show_the_result(cx: &mut TestAppContext) {
    let source = FakeSource::new(Ok(Release::new(Version::new(2, 0, 0), None)));
    let root = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, root.path(), source, Version::new(1, 0, 0));
    window.choose_name("Luca");
    window.until("update offered", |window| window.shows("update-notice"));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    window.state.update(&mut window.cx, move |state, cx| {
        state.set_update_installer(move || {
            count.fetch_add(1, Ordering::SeqCst);
            let held = held.clone();
            Box::pin(async move {
                held.notified().await;
                Err("test refusal".into())
            })
        });
        cx.notify();
    });
    window.click("update-notice");
    window.click("install-update");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.notify_one();
    window.until("failed update is stated", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.update_install_status() == Some("The update failed: test refusal.")
        })
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(window.shows("install-update") && window.shows("update-install-status"));
    let count = calls.clone();
    window.state.update(&mut window.cx, move |state, cx| {
        state.set_update_installer(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(txt::installed(&Previous::Kept(Version::new(1, 0, 0)))) })
        });
        cx.notify();
    });
    window.click("install-update");
    window.until("installed update is stated", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.update_install_status()
                == Some("Update installed. Restart Junto to use the new version. Version 1.0.0 is kept for a rollback.")
        })
    });
    window.click("install-update");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
#[gpui::test]
fn expired_and_untrusted_metadata_are_visible_and_offer_no_install(cx: &mut TestAppContext) {
    let source = FakeSource::new(Err(UpdateError::Expired));
    let root = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, root.path(), source.clone(), Version::new(1, 0, 0));
    window.choose_name("Luca");
    window.until("expiry shown", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.version_view().check == CheckShown::Expired
        })
    });
    window.in_shell(|shell, window, cx| shell.open_version_settings(window, cx));
    assert!(!window.shows("install-update") && !window.shows("update-notice"));
    source.set(Err(UpdateError::UntrustedKeys));
    window.click("check-update");
    window.until("manual update shown", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.version_view().check == CheckShown::UntrustedKeys
        })
    });
    assert_eq!(
        txt::check_shown(&CheckShown::UntrustedKeys),
        txt::UNTRUSTED_KEYS
    );
    assert!(!window.shows("install-update") && !window.shows("update-notice"));
    assert!(!txt::UNTRUSTED_KEYS.contains("install the current version"));
    assert!(txt::UNTRUSTED_KEYS.contains("whoever runs your server"));
}

#[gpui::test]
fn untrusted_information_removes_every_install_and_release_page_action(cx: &mut TestAppContext) {
    let source = FakeSource::new(Ok(Release::new(
        Version::new(2, 0, 0),
        Some("https://release.invalid/"),
    )));
    let root = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, root.path(), source.clone(), Version::new(1, 0, 0));
    window.choose_name("Luca");
    window.until("authenticated update offered", |window| {
        window.shows("update-notice")
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    window.state.update(&mut window.cx, move |state, cx| {
        state.set_update_installer(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(String::new()) })
        });
        cx.notify();
    });
    window.in_shell(|shell, window, cx| shell.open_version_settings(window, cx));
    assert!(window.shows("install-update"));
    source.set(Err(UpdateError::UntrustedKeys));
    window.click("check-update");
    window.until("untrusted information stated", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.version_view().check == CheckShown::UntrustedKeys
        })
    });
    assert!(!window.shows("update-notice") && !window.shows("install-update"));
    assert!(!window.shows("release-page-help"));
    window
        .state
        .update(&mut window.cx, |state, cx| state.install_update(cx));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn line(window: &mut crate::harness::Window) -> junto_client::state::update_install::VersionLine {
    window
        .state
        .read_with(&window.cx, |state, _| state.version_line())
}

/// The operator's finding 29: the version of this copy below "Add server", and how to
/// update it (ADR 0780).
#[gpui::test]
fn the_server_book_states_the_version_of_this_copy_below_add_server(cx: &mut TestAppContext) {
    let source = FakeSource::new(Ok(Release::new(Version::new(1, 0, 0), None)));
    let root = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, root.path(), source, Version::new(1, 0, 0));
    window.choose_name("Luca");
    window.until("the check answered", |window| line(window).sentence == "Up to date.");
    assert!(window.shows("client-version"));
    assert!(!window.shows("client-version-action"));
    let add = window.cx.debug_bounds("address-field").unwrap();
    let version = window.cx.debug_bounds("client-version").unwrap();
    assert!(version.top() >= add.bottom(), "{version:?} below {add:?}");
    assert_eq!(line(&mut window).version, "Junto 1.0.0");
    // Collapsed, the server book keeps the version below the button that adds a server.
    window.click("collapse-server-book");
    let add = window.cx.debug_bounds("add-server").unwrap();
    let version = window.cx.debug_bounds("client-version").unwrap();
    assert!(version.top() >= add.bottom(), "{version:?} below {add:?}");
}

#[gpui::test]
fn an_update_from_the_server_book_installs_and_offers_the_restart(cx: &mut TestAppContext) {
    let source = FakeSource::new(Ok(Release::new(Version::new(2, 0, 0), None)));
    let root = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, root.path(), source, Version::new(1, 0, 0));
    window.choose_name("Luca");
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    window.state.update(&mut window.cx, move |state, cx| {
        state.set_update_installer(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(txt::installed(&Previous::Kept(Version::new(1, 0, 0)))) })
        });
        cx.notify();
    });
    window.until("the newer version is offered", |window| {
        window.shows("client-version-action")
    });
    assert_eq!(line(&mut window).sentence, "Junto 2.0.0 is available.");
    window.click("client-version-action");
    window.until("the update is installed", |window| {
        line(window).action == Some(junto_client::state::update_install::VersionAction::Restart)
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        line(&mut window).sentence,
        "Junto 2.0.0 is installed. Restart Junto to use it."
    );
    // The restart quits and starts the installed version, from the server book, the
    // notice of the channel column, or the settings.
    let mut restart = window.cx.expect_restart();
    window.click("client-version-action");
    assert!(matches!(restart.try_recv(), Ok(Some(_))));
    let mut restart = window.cx.expect_restart();
    window.click("update-notice");
    assert!(matches!(restart.try_recv(), Ok(Some(_))));
    window.in_shell(|shell, window, cx| shell.open_version_settings(window, cx));
    let mut restart = window.cx.expect_restart();
    window.click_in_settings("restart-after-update");
    assert!(matches!(restart.try_recv(), Ok(Some(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn a_copy_that_does_not_update_itself_states_its_steps_and_the_download_page(
    cx: &mut TestAppContext,
) {
    use junto_client::state::update_install::UpdateWay;
    let source = FakeSource::new(Ok(Release::new(Version::new(2, 0, 0), None)));
    let root = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, root.path(), source, Version::new(1, 0, 0));
    window.choose_name("Luca");
    window.state.update(&mut window.cx, |state, cx| {
        state.show_update_way_fixture(
            UpdateWay::SystemPackage,
            Some("https://releases.example.test/junto/"),
            false,
            cx,
        );
    });
    window.until("the newer version is offered", |window| {
        window.shows("client-version-action")
    });
    assert!(
        line(&mut window).sentence.contains("junto_2.0.0_amd64.deb"),
        "{}",
        line(&mut window).sentence
    );
    // The button opens the section "Version" with the steps, the command and the
    // address of the download page.
    window.click("client-version-action");
    assert!(window.in_shell(|shell, _, _| shell.is_settings_open()));
    for part in ["update-steps", "update-command", "download-page"] {
        assert!(window.shows(part), "{part}");
    }
    assert!(!window.shows("install-update"));
    window.click_in_settings("copy-update-command");
    assert_eq!(
        window
            .cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("sudo apt install ./junto_2.0.0_amd64.deb")
    );
    window.click_in_settings("copy-download-page");
    assert_eq!(
        window
            .cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("https://releases.example.test/junto/")
    );
    // A copy in a folder that it cannot write, and a copy that no package installed.
    for way in [UpdateWay::MoveFirst, UpdateWay::NotAPackage] {
        window.state.update(&mut window.cx, |state, cx| {
            state.show_update_way_fixture(
                way.clone(),
                Some("https://releases.example.test/junto/"),
                false,
                cx,
            );
        });
        assert!(window.shows("update-steps"), "{way:?}");
        assert!(!window.shows("install-update") && !window.shows("update-command"));
    }
}
