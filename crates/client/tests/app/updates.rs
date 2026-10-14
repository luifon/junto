//! Update controls use a substitute installer and never touch a system bundle.
use crate::harness::open_with_updates;
use gpui::TestAppContext;
use junto_client::state::book::CheckShown;
use junto_client::text::book as txt;
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
            Box::pin(async { Ok(()) })
        });
        cx.notify();
    });
    window.click("install-update");
    window.until("installed update is stated", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.update_install_status() == Some(txt::INSTALLED)
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
            Box::pin(async { Ok(()) })
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
