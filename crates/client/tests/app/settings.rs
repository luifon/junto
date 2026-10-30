//! The settings of the application: a list of sections at the left and one section at a
//! time, without empty space in a section (ADR 0703, ADR 0730).

use gpui::{Bounds, Pixels, TestAppContext, px};
use junto_client::shell::SettingsSection;
use junto_client::theme::{self, Theme};
use junto_client_core::settings::ThemeChoice;

use crate::harness::{Window, named, open, selector};

fn bounds(window: &mut Window, selector: &'static str) -> Bounds<Pixels> {
    window
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

#[gpui::test]
fn closing_the_main_window_keeps_the_client_and_the_tray_opens_it_again(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    let tray = window.state.read_with(cx, |state, _| state.kept_tray());
    window.simulate(|cx| cx.simulate_keystrokes("secondary-w"));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    cx.update(|cx| assert!(tray.press(junto_client::text::tray::OPEN_JUNTO, &window.state, cx)));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert_eq!(
        window
            .state
            .read_with(cx, |state, _| state.preset_name().to_owned()),
        "Member"
    );
}

#[gpui::test]
fn closing_in_the_background_can_be_disabled_and_the_choice_is_kept(cx: &mut TestAppContext) {
    let (directory, mut window) = named(cx, "Member");
    window.click("app-settings");
    window.click("settings-section-interface");
    assert!(window.shows("keep-running-after-close"));
    window.click("keep-running-after-close");
    window.run_tasks();
    let stored = std::fs::read_to_string(directory.path().join("junto/settings.json")).unwrap();
    assert!(
        stored.contains("\"keep_running_after_close\": false"),
        "{stored}"
    );
    window.close();
    let mut window = open(cx, directory.path());
    window.click("app-settings");
    window.click("settings-section-interface");
    window.click("keep-running-after-close");
    window.run_tasks();
    assert!(
        std::fs::read_to_string(directory.path().join("junto/settings.json"))
            .unwrap()
            .contains("\"keep_running_after_close\": true")
    );
    window.click("keep-running-after-close");
    window.simulate(|cx| cx.simulate_keystrokes("secondary-w"));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    cx.update(|cx| junto_client::notify::bring_to_front(None, cx));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}

fn page(section: SettingsSection) -> String {
    format!("settings-page-{}", section.selector())
}

#[gpui::test]
fn the_settings_show_one_section_at_a_time_from_a_list_of_sections(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Luca");
    window.click("app-settings");
    // The window of the settings is as wide as the administration, wider than a dialog.
    let panel = bounds(&mut window, "settings-dialog");
    assert!(panel.size.width > px(700.0), "{panel:?}");
    assert!(window.shows_named(page(SettingsSection::Profile)));
    let shown = [
        (SettingsSection::Profile, "name-field"),
        (SettingsSection::Picture, "preset-picture"),
        (SettingsSection::Interface, "scale-0"),
        (SettingsSection::Voice, "input-mode-0"),
        (SettingsSection::Quiet, "quiet-no-server"),
        (SettingsSection::Notifications, "notifications-state"),
        (SettingsSection::Devices, "settings-link-first"),
        (SettingsSection::Permissions, "check-permissions"),
        (SettingsSection::About, "version-section"),
    ];
    for (section, control) in shown {
        window.click(section.selector());
        assert!(window.shows(control), "{section:?} shows {control}");
        for (other, other_control) in shown {
            assert_eq!(
                window.shows_named(page(other)),
                other == section,
                "{other:?} while {section:?} is open"
            );
            if other != section {
                assert!(
                    !window.shows(other_control),
                    "{other_control} in {section:?}"
                );
            }
        }
        // The list keeps its place when another section opens.
        assert_eq!(bounds(&mut window, "settings-dialog"), panel);
    }
    // The settings open at their first section again.
    window.click("dialog-close");
    window.click("app-settings");
    assert!(window.shows_named(page(SettingsSection::Profile)));
}

/// The live test found a large empty space after the name, the picture, the scale and the
/// voice settings: each kept the space of a notice that had not arrived.
#[gpui::test]
fn a_section_ends_after_its_last_control_and_its_help(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Luca");
    window.click("app-settings");
    for (section, last) in [
        (SettingsSection::Profile, "save-name"),
        (SettingsSection::Interface, "theme-0"),
        (SettingsSection::Voice, "automatic-gain-row"),
        (SettingsSection::Notifications, "notifications-state"),
        // The switch of the last kind of sounds, with its help, ends the section (ADR 0707).
        (SettingsSection::Sounds, "sound-kind-hand-raised-row"),
    ] {
        window.click(section.selector());
        let page = bounds(&mut window, selector(page(section)));
        let last = bounds(&mut window, last);
        let below = page.bottom() - last.bottom();
        // One or two lines of help follow the last control, and nothing else.
        assert!(
            below < px(48.0),
            "{section:?}: {below:?} of the section under its last control"
        );
    }
}

#[gpui::test]
fn a_name_error_follows_the_controls_of_its_section_and_moves_none(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    window.click("app-settings");
    window.in_shell(|shell, window, cx| shell.type_name(" ", window, cx));
    let field = bounds(&mut window, "name-field");
    let save = bounds(&mut window, "save-name");
    window.click("save-name");
    window.until("the name error is drawn", |window| {
        window.shows("name-error")
    });
    assert_eq!(bounds(&mut window, "name-field"), field);
    assert_eq!(bounds(&mut window, "save-name"), save);
    assert!(bounds(&mut window, "name-error").top() > save.bottom());
}

#[gpui::test]
fn the_dark_theme_is_chosen_in_the_interface_section_and_kept(cx: &mut TestAppContext) {
    let (directory, mut window) = named(cx, "Luca");
    let colours = |window: &mut Window| window.cx.update(|_, cx| theme::theme(cx).colors);
    assert_eq!(colours(&mut window), Theme::stone().colors);
    window.click("app-settings");
    window.click("settings-section-interface");
    // Light, Dark and System, in this order.
    assert!(window.shows("theme-0") && window.shows("theme-1") && window.shows("theme-2"));
    window.click("theme-1");
    assert_eq!(colours(&mut window), Theme::broadcast().colors);
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.settings().theme),
        ThemeChoice::Dark
    );
    window.run_tasks();
    let stored = std::fs::read_to_string(directory.path().join("junto").join("settings.json"))
        .expect("the settings are written");
    assert!(stored.contains("\"theme\": \"dark\""), "{stored}");
    // A new start of the application draws its first window in the dark theme. The theme
    // of the test application is set back to the light one before, so that the window
    // shows what the start reads from the settings.
    window.close();
    cx.update(|cx| theme::apply(ThemeChoice::Light, cx));
    let mut window = crate::harness::open(cx, directory.path());
    assert_eq!(colours(&mut window), Theme::broadcast().colors);
    window.click("app-settings");
    window.click("settings-section-interface");
    window.click("theme-0");
    assert_eq!(colours(&mut window), Theme::stone().colors);
}

/// The sounds have a switch for all of them and one per kind (ADR 0707). All sounds off
/// holds every kind off and keeps the choice of each kind, and the choices are kept over a
/// restart.
#[gpui::test]
fn the_switches_of_the_sounds_are_kept_and_all_sounds_holds_every_kind(cx: &mut TestAppContext) {
    use junto_client::shell::sound_kind_selector;
    use junto_client_core::voice::SoundKind;
    let sounds = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.voice().sounds.clone())
    };
    let (directory, mut window) = named(cx, "Luca");
    window.click("app-settings");
    window.click(SettingsSection::Sounds.selector());
    assert!(window.shows("all-sounds"));
    for kind in SoundKind::ALL {
        assert!(window.shows(sound_kind_selector(kind)), "{kind:?}");
    }
    window.click_in_settings(sound_kind_selector(SoundKind::PersonJoined));
    window.until("the sounds of people are off", |window| {
        !sounds(window).kind_on(SoundKind::PersonJoined)
    });
    window.click_in_settings("all-sounds");
    window.until("all sounds are off", |window| !sounds(window).all);
    let off = sounds(&mut window);
    assert!(SoundKind::ALL.into_iter().all(|kind| !off.plays(kind)));
    // While all sounds are off, a switch of a kind takes no press and keeps its choice.
    window.click_in_settings(sound_kind_selector(SoundKind::Messages));
    window.settle();
    assert_eq!(sounds(&mut window), off);
    window.close();

    let mut window = open(cx, directory.path());
    assert_eq!(sounds(&mut window), off);
    window.click("app-settings");
    window.click(SettingsSection::Sounds.selector());
    window.click_in_settings("all-sounds");
    window.until("all sounds are on again", |window| sounds(window).all);
    let again = sounds(&mut window);
    assert!(!again.plays(SoundKind::PersonJoined) && again.plays(SoundKind::Messages));
}

#[gpui::test]
fn startup_at_login_is_available_in_the_interface_settings(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    window.click("app-settings");
    window.click("settings-section-interface");
    assert!(window.shows("start-at-login"));
}

#[gpui::test]
fn startup_at_login_follows_the_system_adapter_and_refreshes_external_changes(
    cx: &mut TestAppContext,
) {
    use junto_client_core::startup::{KeptStartup, Startup, Status};
    let (directory, mut window) = named(cx, "Member");
    let system = std::sync::Arc::new(KeptStartup::default());
    window.state.update(&mut window.cx, |state, cx| {
        state.use_startup_service(system.clone(), cx)
    });
    window.click("app-settings");
    window.click("settings-section-interface");
    window.click("start-at-login");
    window.until("the login setting is on", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.startup().status == Some(Status::On) && !state.startup().busy
        })
    });
    assert_eq!(system.status().unwrap(), Status::On);
    window.close();
    let mut window = open(cx, directory.path());
    window.state.update(&mut window.cx, |state, cx| {
        state.use_startup_service(system.clone(), cx)
    });
    window.click("app-settings");
    window.click("settings-section-interface");
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.startup().status),
        Some(Status::On)
    );
    system.show(Status::Off);
    window.state.update(&mut window.cx, |state, cx| {
        state.set_window_active(false, cx);
        state.set_window_active(true, cx);
    });
    window.until("an external change is read", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.startup().status == Some(Status::Off) && !state.startup().busy
        })
    });
}

#[gpui::test]
fn startup_at_login_keeps_a_failed_write_and_retries_it(cx: &mut TestAppContext) {
    use junto_client_core::startup::{KeptStartup, Startup, Status};
    struct Refusal {
        saved: KeptStartup,
        refuse: std::sync::atomic::AtomicBool,
    }
    impl Startup for Refusal {
        fn status(&self) -> std::io::Result<Status> {
            self.saved.status()
        }
        fn set(&self, enabled: bool) -> std::io::Result<Status> {
            if self.refuse.load(std::sync::atomic::Ordering::Relaxed) {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "test refusal",
                ))
            } else {
                self.saved.set(enabled)
            }
        }
    }
    let (_directory, mut window) = named(cx, "Member");
    let system = std::sync::Arc::new(Refusal {
        saved: KeptStartup::default(),
        refuse: true.into(),
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.use_startup_service(system.clone(), cx)
    });
    window.click("app-settings");
    window.click("settings-section-interface");
    window.click("start-at-login");
    window.until("the login failure is shown", |window| {
        window.shows("startup-error")
    });
    assert_eq!(system.status().unwrap(), Status::Off);
    system
        .refuse
        .store(false, std::sync::atomic::Ordering::Relaxed);
    window.click_in_settings("startup-retry");
    window.until("the failed write is retried", |window| {
        system.status().unwrap() == Status::On && !window.shows("startup-error")
    });
}

#[gpui::test]
fn startup_at_login_shows_approval_and_unavailable_statuses(cx: &mut TestAppContext) {
    use junto_client_core::startup::{KeptStartup, Startup, Status, Unavailable};
    struct Approval(KeptStartup, std::sync::atomic::AtomicBool);
    impl Startup for Approval {
        fn status(&self) -> std::io::Result<Status> {
            self.0.status()
        }
        fn set(&self, enabled: bool) -> std::io::Result<Status> {
            self.0.set(enabled)
        }
        fn open_approval(&self) -> std::io::Result<()> {
            self.1.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    }
    let (_directory, mut window) = named(cx, "Member");
    let system = std::sync::Arc::new(Approval(KeptStartup::default(), false.into()));
    system.0.show(Status::RequiresApproval);
    window.state.update(&mut window.cx, |state, cx| {
        state.use_startup_service(system.clone(), cx)
    });
    window.click("app-settings");
    window.click("settings-section-interface");
    window.until("the approval requirement is shown", |window| {
        window.shows("startup-approval")
    });
    window.click_in_settings("startup-open-approval");
    window.until("the approval settings are opened", |_| {
        system.1.load(std::sync::atomic::Ordering::Relaxed)
    });
    system
        .0
        .show(Status::Unavailable(Unavailable::CustomProfile));
    window
        .state
        .update(&mut window.cx, |state, cx| state.refresh_startup(cx));
    window.until("the unsupported profile is shown", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.startup().status)
            == Some(Status::Unavailable(Unavailable::CustomProfile))
    });
    assert!(!window.shows("startup-approval"));
    window.click_in_settings("start-at-login");
    assert_eq!(
        system.status().unwrap(),
        Status::Unavailable(Unavailable::CustomProfile)
    );
}
