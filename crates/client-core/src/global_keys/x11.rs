//! The reader of the keyboard of an X11 session (ADR 0776): the keys that the X server holds
//! down, whichever window has the focus. A Wayland session gives no program the keys of
//! another one, also not through its X server, so the reader is for X11 sessions only.

use std::collections::HashMap;

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::{Key, KeyReader, ReadError};

/// True in an X11 session: an X display and no Wayland display.
pub fn available() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    set("DISPLAY") && !set("WAYLAND_DISPLAY")
}

/// The key symbols of a key: both keys of a modifier, and both cases of a letter.
fn symbols(key: Key) -> Vec<u32> {
    match key {
        Key::Control => vec![0xffe3, 0xffe4],
        Key::Alt => vec![0xffe9, 0xffea],
        Key::Shift => vec![0xffe1, 0xffe2],
        // Super, and Meta, which some layouts give the same keys.
        Key::Meta => vec![0xffeb, 0xffec, 0xffe7, 0xffe8],
        Key::Letter(letter) => vec![u32::from(letter), u32::from(letter.to_ascii_lowercase())],
        Key::Digit(digit) => vec![u32::from(b'0' + digit)],
        Key::Function(number) => vec![0xffbe + u32::from(number) - 1],
    }
}

pub struct Reader {
    connection: RustConnection,
    /// The key codes of each key symbol of the layout.
    codes: HashMap<u32, Vec<u8>>,
}

fn failed(error: impl std::fmt::Display) -> ReadError {
    ReadError::Failed(error.to_string())
}

impl Reader {
    /// Connects to the display of the session and reads its layout.
    pub fn connect() -> Result<Self, ReadError> {
        if !available() {
            return Err(ReadError::Failed("no X11 session".to_owned()));
        }
        let (connection, _) = x11rb::connect(None).map_err(failed)?;
        let setup = connection.setup();
        let (first, last) = (setup.min_keycode, setup.max_keycode);
        let mapping = connection
            .get_keyboard_mapping(first, last - first + 1)
            .map_err(failed)?
            .reply()
            .map_err(failed)?;
        let per_code = usize::from(mapping.keysyms_per_keycode.max(1));
        let mut codes: HashMap<u32, Vec<u8>> = HashMap::new();
        for (index, symbols) in mapping.keysyms.chunks(per_code).enumerate() {
            let Ok(code) = u8::try_from(usize::from(first) + index) else {
                continue;
            };
            for symbol in symbols.iter().filter(|symbol| **symbol != 0) {
                codes.entry(*symbol).or_default().push(code);
            }
        }
        Ok(Self { connection, codes })
    }
}

impl KeyReader for Reader {
    fn held(&mut self, keys: &[Key]) -> Result<Vec<Key>, ReadError> {
        let map = self
            .connection
            .query_keymap()
            .map_err(failed)?
            .reply()
            .map_err(failed)?
            .keys;
        let down = |code: u8| map[usize::from(code / 8)] & (1 << (code % 8)) != 0;
        Ok(keys
            .iter()
            .copied()
            .filter(|key| {
                symbols(*key)
                    .iter()
                    .filter_map(|symbol| self.codes.get(symbol))
                    .flatten()
                    .any(|code| down(*code))
            })
            .collect())
    }
}

/// The reader against the X server of the run. It does nothing without
/// `JUNTO_TEST_GLOBAL_KEYS`: `tools/linux/display.sh x11` gives the server, and `xdotool`
/// presses the keys through the XTEST extension, as a keyboard does.
#[cfg(test)]
mod tests {
    use super::*;

    fn xdotool(arguments: &[&str]) {
        let status = std::process::Command::new("xdotool")
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "xdotool {arguments:?}");
    }

    #[test]
    fn the_keys_that_the_x_server_holds_are_read_whichever_window_has_the_focus() {
        if std::env::var_os("JUNTO_TEST_GLOBAL_KEYS").is_none() {
            return;
        }
        let mut reader = Reader::connect().unwrap();
        let keys = [
            Key::Alt,
            Key::Control,
            Key::Shift,
            Key::Letter('M'),
            Key::Digit(7),
            Key::Function(13),
        ];
        assert_eq!(reader.held(&keys).unwrap(), []);
        xdotool(&["keydown", "alt"]);
        assert_eq!(reader.held(&keys).unwrap(), [Key::Alt]);
        xdotool(&["keydown", "ctrl", "keydown", "shift", "keydown", "m"]);
        assert_eq!(
            reader.held(&keys).unwrap(),
            [Key::Alt, Key::Control, Key::Shift, Key::Letter('M')]
        );
        xdotool(&[
            "keyup", "m", "keyup", "shift", "keyup", "ctrl", "keyup", "alt",
        ]);
        assert_eq!(reader.held(&keys).unwrap(), []);
        xdotool(&["keydown", "F13", "keydown", "7"]);
        assert_eq!(
            reader.held(&keys).unwrap(),
            [Key::Digit(7), Key::Function(13)]
        );
        xdotool(&["keyup", "7", "keyup", "F13"]);
        assert_eq!(reader.held(&keys).unwrap(), []);
    }
}
