use super::*;

fn combo(text: &str) -> KeyCombo {
    text.parse()
        .unwrap_or_else(|error| panic!("{text}: {error}"))
}

#[test]
fn every_action_has_a_name_and_the_defaults_have_no_conflict() {
    for macos in [false, true] {
        let mut seen: Vec<(KeyCombo, KeyAction)> = Vec::new();
        for action in KeyAction::ALL {
            assert!(!action.name().is_empty(), "{action:?}");
            if let Some(keys) = action.default_keys(macos) {
                assert!(keys.is_shortcut(), "{action:?} {keys}");
                if let Some((_, other)) = seen.iter().find(|(held, _)| *held == keys) {
                    panic!("{action:?} and {other:?} share {keys} (macOS {macos})");
                }
                seen.push((keys, action));
            }
        }
    }
    assert_eq!(
        KeyAction::ToggleMute.default_keys(false),
        Some(combo("control+shift+m"))
    );
    assert_eq!(KeyAction::Search.default_keys(true), Some(combo("meta+f")));
    assert_eq!(KeyAction::PushToTalk.default_keys(false), None);
}

/// No default takes the keys that later actions of the list take (the quick switcher and the
/// zoom).
#[test]
fn no_default_takes_a_reserved_combination() {
    for macos in [false, true] {
        let reserved = KeyAction::reserved(macos);
        assert_eq!(reserved.len(), 4);
        for action in KeyAction::ALL {
            if let Some(keys) = action.default_keys(macos) {
                assert!(!reserved.contains(&keys), "{action:?} takes {keys}");
            }
        }
    }
    assert!(KeyAction::reserved(false).contains(&combo("control+k")));
    assert!(KeyAction::reserved(true).contains(&combo("meta+equal")));
}

#[test]
fn a_change_a_conflict_and_a_reset_of_a_binding() {
    let mut keybinds = Keybinds::default();
    assert!(keybinds.conflicts(KeyAction::Search).is_empty());
    let search = keybinds.get(KeyAction::Search).keys.unwrap();
    keybinds
        .set(KeyAction::Notices, Some(search.clone()))
        .unwrap();
    assert!(keybinds.is_changed(KeyAction::Notices));
    assert_eq!(keybinds.conflicts(KeyAction::Search), [KeyAction::Notices]);
    assert_eq!(keybinds.conflicts(KeyAction::Notices), [KeyAction::Search]);
    // The first action in the order of the settings takes a shared key.
    assert_eq!(keybinds.action_for(&search), Some(KeyAction::Search));
    keybinds.reset(KeyAction::Notices);
    assert!(!keybinds.is_changed(KeyAction::Notices));
    assert!(keybinds.conflicts(KeyAction::Search).is_empty());
    // No key, then all reset.
    keybinds.set(KeyAction::Search, None).unwrap();
    assert_eq!(keybinds.get(KeyAction::Search).keys, None);
    assert_eq!(keybinds.action_for(&search), None);
    keybinds.reset_all();
    assert_eq!(keybinds, Keybinds::default());
}

#[test]
fn a_key_without_a_modifier_is_only_for_a_held_action_and_outside_only_for_a_call_action() {
    let mut keybinds = Keybinds::default();
    assert_eq!(
        keybinds.set(KeyAction::Search, Some(combo("f"))),
        Err(KeybindError::NeedsModifier)
    );
    keybinds
        .set(KeyAction::PushToTalk, Some(combo("f13")))
        .unwrap();
    keybinds
        .set(KeyAction::PushToMute, Some(combo("alt")))
        .unwrap();
    assert_eq!(
        keybinds.set_outside(KeyAction::Search, true),
        Err(KeybindError::NotOutside)
    );
    keybinds.set_outside(KeyAction::LeaveCall, true).unwrap();
    keybinds.set_outside(KeyAction::ToggleMute, false).unwrap();
    let outside: Vec<KeyAction> = keybinds
        .outside()
        .into_iter()
        .map(|(action, _)| action)
        .collect();
    assert_eq!(
        outside,
        [
            KeyAction::ToggleSoundOff,
            KeyAction::PushToTalk,
            KeyAction::PushToMute,
            KeyAction::LeaveCall
        ]
    );
}

#[test]
fn the_bindings_are_written_by_name_and_a_file_is_checked_when_it_is_read() {
    let mut keybinds = Keybinds::default();
    keybinds
        .set(KeyAction::ToggleCamera, Some(combo("control+alt+c")))
        .unwrap();
    keybinds.set(KeyAction::Search, None).unwrap();
    let text = serde_json::to_string(&keybinds).unwrap();
    assert_eq!(
        text,
        r#"{"toggle_camera":{"keys":"control+alt+c","outside":false},"search":{"keys":null,"outside":false}}"#
    );
    let read: Keybinds = serde_json::from_str(&text).unwrap();
    assert_eq!(read, keybinds);
    // A file that a person changed by hand: a key without a modifier for an action that is
    // not held, and an action of the window marked to work outside.
    let edited: Keybinds = serde_json::from_str(
        r#"{"search":{"keys":"f","outside":false},"notices":{"keys":"control+n","outside":true}}"#,
    )
    .unwrap();
    let checked = edited.checked();
    assert_eq!(
        checked.get(KeyAction::Search),
        Keybinds::default_binding(KeyAction::Search)
    );
    assert_eq!(
        checked.get(KeyAction::Notices),
        Binding {
            keys: Some(combo("control+n")),
            outside: false,
            inherited: false,
        }
    );
}
