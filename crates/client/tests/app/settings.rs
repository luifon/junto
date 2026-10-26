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
        (SettingsSection::Sounds, "sound-kind-hands-row"),
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
    window.click_in_settings(sound_kind_selector(SoundKind::People));
    window.until("the sounds of people are off", |window| {
        !sounds(window).kind_on(SoundKind::People)
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
    assert!(!again.plays(SoundKind::People) && again.plays(SoundKind::Messages));
}
