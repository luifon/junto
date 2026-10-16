#![allow(clippy::unwrap_used)]
#[path = "native/protocol.rs"]
mod protocol;
use protocol::{Geometry, acknowledged};

#[test]
fn fixture_geometry_must_fit_the_selected_physical_display() {
    let found = Geometry::parse("ready 5 9 40 60 800 600 2000 1200\n").unwrap();
    assert_eq!(found.window, 5);
    assert_eq!(found.display, 9);
    assert_eq!(
        (found.x, found.y, found.width, found.height),
        (40, 60, 800, 600)
    );
    assert_eq!((found.display_width, found.display_height), (2000, 1200));
    for invalid in [
        "",
        "ready 0 9 0 0 1 1 2 2",
        "ready 5 0 0 0 1 1 2 2",
        "ready 5 9 0 0 0 1 2 2",
        "ready 5 9 10 0 1600 1200 1000 800",
        "ready 5 9 4294967295 0 1 1 100 100",
        "ready 5 9 0 0 1 1 2 2 extra",
    ] {
        assert!(Geometry::parse(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn a_redraw_acknowledgement_must_match_the_requested_marker() {
    assert!(acknowledged("pattern 19\n", 19));
    assert!(!acknowledged("pattern 18\n", 19));
    assert!(!acknowledged("ready 19\n", 19));
    assert!(!acknowledged("pattern 19 extra\n", 19));
}
