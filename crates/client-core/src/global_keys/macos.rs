//! The reader of the keyboard of macOS (ADR 0776): the state of each key as the HID system
//! holds it, which macOS gives a program with the permission Input Monitoring.

#![allow(unsafe_code)]

use super::{Key, KeyReader, Permission, ReadError};

/// `kCGEventSourceStateHIDSystemState`: the state of the keys of the hardware, whichever
/// application is in front.
const HID_SYSTEM_STATE: i32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    safe fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    safe fn CGPreflightListenEventAccess() -> bool;
    safe fn CGRequestListenEventAccess() -> bool;
}

pub(super) fn input_monitoring() -> Permission {
    if CGPreflightListenEventAccess() {
        Permission::Granted
    } else {
        Permission::NotGranted
    }
}

pub(super) fn request_input_monitoring() -> Permission {
    if CGRequestListenEventAccess() {
        Permission::Granted
    } else {
        Permission::NotGranted
    }
}

/// The virtual key codes of a key: both keys of a modifier.
fn codes(key: Key) -> &'static [u16] {
    static LETTERS: [u16; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F,
        0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    static DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    static FUNCTIONS: [u16; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71,
        0x6A, 0x40, 0x4F, 0x50, 0x5A,
    ];
    match key {
        Key::Control => &[0x3B, 0x3E],
        Key::Alt => &[0x3A, 0x3D],
        Key::Shift => &[0x38, 0x3C],
        Key::Meta => &[0x37, 0x36],
        Key::Letter(letter) => {
            let index = (letter as usize).wrapping_sub('A' as usize);
            LETTERS.get(index).map_or(&[], std::slice::from_ref)
        }
        Key::Digit(digit) => DIGITS
            .get(usize::from(digit))
            .map_or(&[], std::slice::from_ref),
        // macOS has no keys after F20.
        Key::Function(number) => FUNCTIONS
            .get(usize::from(number).wrapping_sub(1))
            .map_or(&[], std::slice::from_ref),
        Key::Up => &[0x7E],
        Key::Down => &[0x7D],
        Key::Left => &[0x7B],
        Key::Right => &[0x7C],
        // Return, and Enter of the number block.
        Key::Enter => &[0x24, 0x4C],
        Key::Escape => &[0x35],
        // The Delete key of a Mac keyboard deletes backwards.
        Key::Backspace => &[0x33],
        Key::PageUp => &[0x74],
        Key::PageDown => &[0x79],
        Key::Comma => &[0x2B],
        Key::Slash => &[0x2C],
    }
}

pub(super) struct Reader;

impl KeyReader for Reader {
    fn held(&mut self, keys: &[Key]) -> Result<Vec<Key>, ReadError> {
        // Without the permission the system gives every key as let go.
        if !CGPreflightListenEventAccess() {
            return Err(ReadError::NotAllowed);
        }
        Ok(keys
            .iter()
            .copied()
            .filter(|key| {
                codes(*key)
                    .iter()
                    .any(|code| CGEventSourceKeyState(HID_SYSTEM_STATE, *code))
            })
            .collect())
    }
}
