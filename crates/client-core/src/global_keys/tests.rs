use super::*;
use crate::keybinds::KeyAction::{PushToTalk, ToggleCamera, ToggleMute};

fn combo(text: &str) -> KeyCombo {
    text.parse().unwrap()
}

#[test]
fn a_combination_is_written_in_one_order_and_read_back() {
    let mute = combo("shift+ctrl+M");
    assert_eq!(mute.to_string(), "control+shift+m");
    assert_eq!(mute.keys(), [Key::Control, Key::Shift, Key::Letter('M')]);
    assert!(mute.is_shortcut());
    assert_eq!(combo(&mute.to_string()), mute);
    assert_eq!(combo("option").keys(), [Key::Alt]);
    assert!(!combo("alt").is_shortcut());
    assert_eq!(combo("cmd+f13").keys(), [Key::Meta, Key::Function(13)]);
    assert_eq!(combo("alt+7").keys(), [Key::Alt, Key::Digit(7)]);
    let json = serde_json::to_string(&mute).unwrap();
    assert_eq!(json, "\"control+shift+m\"");
    assert_eq!(serde_json::from_str::<KeyCombo>(&json).unwrap(), mute);
    // Not combinations: two keys that are not modifiers, a key twice, nothing, and a name
    // that is no key.
    for text in ["a+b", "alt+alt", "", "f25", "f0", "control+tab", "control+"] {
        assert!(text.parse::<KeyCombo>().is_err(), "{text}");
    }
    assert!(serde_json::from_str::<KeyCombo>("\"a+b\"").is_err());
    assert_eq!(
        combo("control+alt+up").keys(),
        [Key::Control, Key::Alt, Key::Up]
    );
    assert_eq!(combo("meta+,").to_string(), "meta+comma");
    assert_eq!(combo("shift+pagedown").keys(), [Key::Shift, Key::PageDown]);
    assert_eq!(Key::all().count(), 4 + 26 + 10 + 24 + 15);
    assert_eq!(combo("meta+[").keys(), [Key::Meta, Key::BracketLeft]);
}

/// When the held keys hold two bound combinations and one holds the other, only the larger
/// one is a press.
#[test]
fn of_two_combinations_where_one_holds_the_other_only_the_larger_is_a_press() {
    let keys = CallKeys {
        bindings: vec![
            (ToggleCamera, combo("shift+m")),
            (ToggleMute, combo("control+shift+m")),
        ],
    };
    let mut edges = Edges::default();
    let (control, shift, m) = (Key::Control, Key::Shift, Key::Letter('M'));
    assert_eq!(
        edges.read(&keys, &[control, shift, m]),
        [KeyEvent::Pressed(ToggleMute)]
    );
    assert_eq!(edges.read(&keys, &[shift, m]), []);
    assert_eq!(edges.read(&keys, &[]), []);
    assert_eq!(
        edges.read(&keys, &[shift, m]),
        [KeyEvent::Pressed(ToggleCamera)]
    );
}

#[test]
fn a_held_key_of_push_to_talk_gives_its_start_and_its_end() {
    let keys = CallKeys {
        bindings: vec![(PushToTalk, combo("alt"))],
    };
    let mut edges = Edges::default();
    assert_eq!(edges.read(&keys, &[]), []);
    assert_eq!(
        edges.read(&keys, &[Key::Alt]),
        [KeyEvent::Held(PushToTalk, true)]
    );
    // Held: nothing new, also with another key held beside it.
    assert_eq!(edges.read(&keys, &[Key::Alt, Key::Letter('K')]), []);
    assert_eq!(
        edges.read(&keys, &[Key::Letter('K')]),
        [KeyEvent::Held(PushToTalk, false)]
    );
    assert_eq!(
        edges.read(&keys, &[Key::Alt]),
        [KeyEvent::Held(PushToTalk, true)]
    );
    // The readings end while the key is held: the hold ends.
    assert_eq!(edges.end(), [KeyEvent::Held(PushToTalk, false)]);
    assert_eq!(edges.end(), []);
}

#[test]
fn the_key_that_mutes_fires_once_for_each_press() {
    let keys = CallKeys {
        bindings: vec![(ToggleMute, combo("control+shift+m"))],
    };
    let mut edges = Edges::default();
    let (control, shift, m) = (Key::Control, Key::Shift, Key::Letter('M'));
    assert_eq!(edges.read(&keys, &[control, shift]), []);
    assert_eq!(
        edges.read(&keys, &[control, shift, m]),
        [KeyEvent::Pressed(ToggleMute)]
    );
    assert_eq!(edges.read(&keys, &[control, shift, m]), []);
    assert_eq!(edges.read(&keys, &[control, shift]), []);
    assert_eq!(
        edges.read(&keys, &[control, shift, m]),
        [KeyEvent::Pressed(ToggleMute)]
    );
    // The letter alone, or with one of the modifiers, is no press.
    assert_eq!(edges.read(&keys, &[m]), []);
    assert_eq!(edges.read(&keys, &[control, m]), []);
    // Both keys of a call in one reading.
    let both = CallKeys {
        bindings: vec![
            (PushToTalk, combo("alt")),
            (ToggleMute, combo("control+shift+m")),
        ],
    };
    let mut edges = Edges::default();
    assert_eq!(
        edges.read(&both, &[Key::Alt, control, shift, m]),
        [
            KeyEvent::Held(PushToTalk, true),
            KeyEvent::Pressed(ToggleMute)
        ]
    );
    assert_eq!(both.keys(), [control, Key::Alt, shift, m]);
}

/// Waits until `events` holds `count` events, for at most two seconds.
fn until_events(
    events: &std::sync::Arc<std::sync::Mutex<Vec<KeyEvent>>>,
    count: usize,
) -> Vec<KeyEvent> {
    let started = std::time::Instant::now();
    loop {
        let held = events.lock().unwrap().clone();
        if held.len() >= count {
            return held;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "after two seconds the events are {held:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_poller_reads_only_while_a_call_needs_the_keys_and_tells_a_refusal() {
    let reader = HeldKeys::default();
    let events: std::sync::Arc<std::sync::Mutex<Vec<KeyEvent>>> = std::sync::Arc::default();
    let told = events.clone();
    let poller = Poller::start(Box::new(reader.clone()), move |event| {
        told.lock().unwrap().push(event);
    });
    // No call: the keys are not read, so a held key gives nothing.
    reader.hold(Ok(vec![Key::Alt]));
    std::thread::sleep(INTERVAL * 6);
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(reader.readings(), 0);

    poller.set(Some(CallKeys {
        bindings: vec![(PushToTalk, combo("alt")), (ToggleMute, combo("control+m"))],
    }));
    assert_eq!(until_events(&events, 1), [KeyEvent::Held(PushToTalk, true)]);
    reader.hold(Ok(vec![Key::Control, Key::Letter('M')]));
    assert_eq!(
        until_events(&events, 3),
        [
            KeyEvent::Held(PushToTalk, true),
            KeyEvent::Held(PushToTalk, false),
            KeyEvent::Pressed(ToggleMute)
        ]
    );
    // The system refuses: the hold ends, and the poller tells why.
    reader.hold(Ok(vec![Key::Alt]));
    until_events(&events, 4);
    reader.hold(Err(ReadError::NotAllowed));
    assert_eq!(
        until_events(&events, 5)[4],
        KeyEvent::Held(PushToTalk, false)
    );
    assert_eq!(poller.failure(), Some(ReadError::NotAllowed));
    reader.hold(Ok(Vec::new()));
    let started = std::time::Instant::now();
    while poller.failure().is_some() {
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        std::thread::sleep(INTERVAL);
    }
    // The call ends: the readings stop.
    poller.set(None);
    std::thread::sleep(INTERVAL * 4);
    let readings = reader.readings();
    std::thread::sleep(INTERVAL * 6);
    assert_eq!(reader.readings(), readings);
}

#[test]
fn equal_global_bindings_dispatch_only_the_first_action_of_the_registry() {
    let combo: KeyCombo = "control+shift+m".parse().unwrap();
    let keys = CallKeys {
        bindings: vec![
            (KeyAction::ToggleSoundOff, combo.clone()),
            (KeyAction::ToggleMute, combo.clone()),
        ],
    };
    let held = HeldKeys::default();
    let (send, events) = std::sync::mpsc::channel();
    let poller = Poller::start(Box::new(held.clone()), move |event| {
        send.send(event).unwrap();
    });
    poller.set(Some(keys));
    held.hold(Ok(combo.keys().to_vec()));
    assert_eq!(
        events
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        KeyEvent::Pressed(KeyAction::ToggleMute)
    );
    assert!(events.recv_timeout(INTERVAL * 4).is_err());
}
