//! The reader of the keyboard of Windows (ADR 0776): the state of each key, which Windows
//! gives every program whichever window is in front.

#![allow(unsafe_code)]

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_F1, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use super::{Key, KeyReader, ReadError};

/// The virtual keys of a key: both keys of the Windows key.
fn codes(key: Key) -> Vec<i32> {
    match key {
        Key::Control => vec![i32::from(VK_CONTROL)],
        Key::Alt => vec![i32::from(VK_MENU)],
        Key::Shift => vec![i32::from(VK_SHIFT)],
        Key::Meta => vec![i32::from(VK_LWIN), i32::from(VK_RWIN)],
        Key::Letter(letter) => vec![letter as i32],
        Key::Digit(digit) => vec![i32::from(b'0' + digit)],
        Key::Function(number) => vec![i32::from(VK_F1) + i32::from(number) - 1],
        Key::Up => vec![0x26],
        Key::Down => vec![0x28],
        Key::Left => vec![0x25],
        Key::Right => vec![0x27],
        Key::Enter => vec![0x0D],
        Key::Escape => vec![0x1B],
        Key::Backspace => vec![0x08],
        Key::PageUp => vec![0x21],
        Key::PageDown => vec![0x22],
        // VK_OEM_COMMA, and VK_OEM_2, the key of the slash on a US keyboard.
        Key::Comma => vec![0xBC],
        Key::Slash => vec![0xBF],
    }
}

pub(super) struct Reader;

impl KeyReader for Reader {
    fn held(&mut self, keys: &[Key]) -> Result<Vec<Key>, ReadError> {
        Ok(keys
            .iter()
            .copied()
            .filter(|key| {
                codes(*key).into_iter().any(|code| {
                    // SAFETY: GetAsyncKeyState takes any virtual key code and reads state
                    // only. The high bit is set while the key is down.
                    let state = unsafe { GetAsyncKeyState(code) };
                    state < 0
                })
            })
            .collect())
    }
}

/// The reader against the keyboard of the desktop of the run. It does nothing without
/// `JUNTO_TEST_GLOBAL_KEYS`. It presses F24, which no application of the machine uses, through
/// `SendInput`, as a keyboard does.
#[cfg(test)]
mod tests {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_F24,
        VK_UP,
    };

    use super::*;

    fn press(key: u16, down: bool) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: one keyboard input of the size that SendInput is told.
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        assert_eq!(sent, 1, "SendInput");
    }

    /// A binding of named keys from the keyboard of the system (ADR 0777): Control with Up
    /// is one press of its action, whichever window is in front.
    #[test]
    fn a_binding_of_a_modifier_and_an_arrow_is_one_press() {
        use crate::global_keys::{CallKeys, Edges, KeyEvent};
        use crate::keybinds::KeyAction;
        if std::env::var_os("JUNTO_TEST_GLOBAL_KEYS").is_none() {
            return;
        }
        let keys = CallKeys {
            bindings: vec![(KeyAction::ToggleSoundOff, "control+up".parse().unwrap())],
        };
        let mut reader = Reader;
        let mut edges = Edges::default();
        assert_eq!(edges.read(&keys, &reader.held(&keys.keys()).unwrap()), []);
        press(VK_CONTROL, true);
        press(VK_UP, true);
        let held = reader.held(&keys.keys()).unwrap();
        press(VK_UP, false);
        press(VK_CONTROL, false);
        assert_eq!(held, [Key::Control, Key::Up]);
        assert_eq!(
            edges.read(&keys, &held),
            [KeyEvent::Pressed(KeyAction::ToggleSoundOff)]
        );
        assert_eq!(edges.read(&keys, &reader.held(&keys.keys()).unwrap()), []);
    }

    #[test]
    fn a_key_that_the_keyboard_holds_is_read_whichever_window_is_in_front() {
        if std::env::var_os("JUNTO_TEST_GLOBAL_KEYS").is_none() {
            return;
        }
        let mut reader = Reader;
        let keys = [Key::Function(24), Key::Alt];
        assert_eq!(reader.held(&keys).unwrap(), []);
        press(VK_F24, true);
        assert_eq!(reader.held(&keys).unwrap(), [Key::Function(24)]);
        press(VK_F24, false);
        assert_eq!(reader.held(&keys).unwrap(), []);
    }
}
