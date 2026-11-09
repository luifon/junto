//! The release notes of the versions of the client in the window: once at the first start
//! after an update, the notes of every version since the one that started last, and from
//! "What is new" in the settings (ADR 0727). The notes are fixtures of the test.

use gpui::TestAppContext;
use junto_client::release_notes::ReleaseNotes;
use junto_client_core::updates::Version;

use crate::harness::open_as_version;

fn version(minor: u32) -> Version {
    Version::new(0, minor, 0)
}

fn notes() -> ReleaseNotes {
    ReleaseNotes::new([
        (version(1), "The first release."),
        (version(2), "### Changes\n\n- Favourites\n- Release notes"),
        (version(3), "```\njunto --version\n```"),
        (version(4), "1. Logos\n2. Photos"),
    ])
}

fn shown(window: &mut crate::harness::Window, of: u32) -> Option<gpui::Bounds<gpui::Pixels>> {
    window.settle();
    window
        .cx
        .debug_bounds(crate::harness::selector(format!("release-notes-0.{of}.0")))
}

#[gpui::test]
fn the_first_start_after_an_update_shows_its_notes_once(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    // A first install shows no notes, and stores its version.
    let mut window = open_as_version(cx, directory.path(), version(2), notes());
    window.choose_name("Luca");
    assert!(!window.shows("release-notes"));
    window.close();
    let stored = std::fs::read_to_string(directory.path().join("junto/settings.json")).unwrap();
    assert!(stored.contains("\"last_build\": \"0.2.0\""), "{stored}");

    // The first start of the next version shows its notes; Escape closes them.
    let mut window = open_as_version(cx, directory.path(), version(3), notes());
    assert!(window.shows("release-notes"));
    assert!(shown(&mut window, 3).is_some());
    assert!(shown(&mut window, 2).is_none());
    window.press("escape");
    assert!(!window.shows("release-notes"));
    window.close();

    // The next start of the same version shows nothing.
    let mut window = open_as_version(cx, directory.path(), version(3), notes());
    assert!(!window.shows("release-notes"));
    window.close();
}

#[gpui::test]
fn an_update_over_several_versions_shows_the_notes_of_each_newest_first(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open_as_version(cx, directory.path(), version(1), notes());
    window.choose_name("Luca");
    window.close();
    let mut window = open_as_version(cx, directory.path(), version(4), notes());
    assert!(window.shows("release-notes"));
    let (four, three, two) = (
        shown(&mut window, 4).unwrap(),
        shown(&mut window, 3).unwrap(),
        shown(&mut window, 2).unwrap(),
    );
    assert!(four.top() < three.top() && three.top() < two.top());
    assert!(
        shown(&mut window, 1).is_none(),
        "the version that ran before"
    );
    window.click("release-notes-close");
    assert!(!window.shows("release-notes"));
    window.close();
}

/// 0.1.0 stores no version: its data directory, with a profile, gets the notes since 0.1.0.
#[gpui::test]
fn the_first_start_after_0_1_0_shows_the_notes_since_it(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open_as_version(cx, directory.path(), version(1), notes());
    window.choose_name("Luca");
    window.close();
    let file = directory.path().join("junto/settings.json");
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    stored.as_object_mut().unwrap().remove("last_build");
    std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    let mut window = open_as_version(cx, directory.path(), version(3), notes());
    assert!(shown(&mut window, 3).is_some() && shown(&mut window, 2).is_some());
    assert!(shown(&mut window, 1).is_none());
    window.close();
}

#[gpui::test]
fn settings_show_the_notes_of_this_version(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open_as_version(cx, directory.path(), version(3), notes());
    window.choose_name("Luca");
    window.click("app-settings");
    window.click("settings-section-about");
    window.click("release-notes-open");
    assert!(shown(&mut window, 3).is_some());
    assert!(shown(&mut window, 2).is_none());
    window.click("release-notes-close");
    assert!(window.shows("settings-dialog"), "the settings open again");
    window.close();

    // A build without notes of its version offers none; its first start shows those of 0.4.0.
    let mut window = open_as_version(cx, directory.path(), version(5), notes());
    assert!(shown(&mut window, 4).is_some());
    window.press("escape");
    window.click("app-settings");
    window.click("settings-section-about");
    assert!(!window.shows("release-notes-open"));
    window.close();
}
