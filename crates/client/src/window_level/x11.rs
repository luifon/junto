//! The request to an X11 window manager for the window state "above" (ADR 0507).
//!
//! gpui does not give its connection to the display. The request is valid from any client,
//! so it is sent on a connection of its own, which ends with the call.

use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

fn text(failure: impl std::fmt::Display) -> String {
    failure.to_string()
}

fn atom(connection: &RustConnection, name: &[u8]) -> Result<u32, String> {
    Ok(connection
        .intern_atom(false, name)
        .map_err(text)?
        .reply()
        .map_err(text)?
        .atom)
}

/// The 32-bit values of a window property.
fn values(connection: &RustConnection, window: u32, property: u32) -> Result<Vec<u32>, String> {
    let reply = connection
        .get_property(false, window, property, AtomEnum::ANY, 0, u32::MAX)
        .map_err(text)?
        .reply()
        .map_err(text)?;
    Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
}

/// Sends the request. `Ok(false)`: the window manager does not state the state "above".
pub(super) fn set_above(display: Option<&str>, window: u32, above: bool) -> Result<bool, String> {
    let (connection, _) = RustConnection::connect(display).map_err(text)?;
    let root = connection
        .get_geometry(window)
        .map_err(text)?
        .reply()
        .map_err(text)?
        .root;
    let state = atom(&connection, b"_NET_WM_STATE")?;
    let state_above = atom(&connection, b"_NET_WM_STATE_ABOVE")?;
    let supported = atom(&connection, b"_NET_SUPPORTED")?;
    if !values(&connection, root, supported)?.contains(&state_above) {
        return Ok(false);
    }
    // A window that the window manager has not taken yet gets the state as a property,
    // which the window manager reads when it takes the window. The request below is then
    // without effect, or it repeats the state.
    let managed = atom(&connection, b"WM_STATE")?;
    if values(&connection, window, managed)?.is_empty() {
        let mut states = values(&connection, window, state)?;
        states.retain(|held| *held != state_above);
        if above {
            states.push(state_above);
        }
        connection
            .change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &states)
            .map_err(text)?
            .check()
            .map_err(text)?;
    }
    // The first value adds (1) or removes (0) the state; the fourth states that an
    // application asks. The window manager changes the stack and leaves the focus.
    let request =
        ClientMessageEvent::new(32, window, state, [u32::from(above), state_above, 0, 1, 0]);
    connection
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            request,
        )
        .map_err(text)?
        .check()
        .map_err(text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::{Duration, Instant};

    use x11rb::connection::Connection as _;

    use x11rb::protocol::xproto::{ConfigureWindowAux, CreateWindowAux, StackMode, WindowClass};

    use super::*;

    fn until(what: &str, mut holds: impl FnMut() -> bool) {
        let started = Instant::now();
        while !holds() {
            assert!(started.elapsed() < Duration::from_secs(20), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A window that is not shown yet.
    fn hidden(connection: &RustConnection, root: u32) -> u32 {
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                40,
                40,
                300,
                200,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new(),
            )
            .unwrap();
        connection.flush().unwrap();
        window
    }

    /// Shows a window and waits until the window manager has taken it.
    fn show(connection: &RustConnection, window: u32) {
        connection.map_window(window).unwrap();
        connection.flush().unwrap();
        let managed = atom(connection, b"WM_STATE").unwrap();
        until("the window manager takes the window", || {
            !values(connection, window, managed).unwrap().is_empty()
        });
    }

    fn window(connection: &RustConnection, root: u32) -> u32 {
        let window = hidden(connection, root);
        show(connection, window);
        window
    }

    #[test]
    #[ignore = "needs the private X11 display: tools/linux/display.sh x11"]
    fn a_window_that_is_not_shown_yet_gets_the_state_when_the_window_manager_takes_it() {
        assert_eq!(std::env::var("DISPLAY").as_deref(), Ok(":99"));
        let (connection, screen) = RustConnection::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let state = atom(&connection, b"_NET_WM_STATE").unwrap();
        let state_above = atom(&connection, b"_NET_WM_STATE_ABOVE").unwrap();
        let pinned = hidden(&connection, root);
        assert_eq!(set_above(None, pinned, true), Ok(true));
        show(&connection, pinned);
        until("the window has the state", || {
            values(&connection, pinned, state)
                .unwrap()
                .contains(&state_above)
        });
    }

    #[test]
    #[ignore = "needs the private X11 display: tools/linux/display.sh x11"]
    fn the_window_manager_keeps_a_window_above_another_without_giving_it_the_focus() {
        assert_eq!(std::env::var("DISPLAY").as_deref(), Ok(":99"));
        let (connection, screen) = RustConnection::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let state = atom(&connection, b"_NET_WM_STATE").unwrap();
        let state_above = atom(&connection, b"_NET_WM_STATE_ABOVE").unwrap();
        let stacking = atom(&connection, b"_NET_CLIENT_LIST_STACKING").unwrap();
        let active = atom(&connection, b"_NET_ACTIVE_WINDOW").unwrap();
        let pinned = window(&connection, root);
        let other = window(&connection, root);
        // The stack is listed from the lowest window to the highest.
        let above = |first: u32, second: u32| {
            let stack = values(&connection, root, stacking).unwrap();
            let place = |window| stack.iter().position(|held| *held == window);
            place(first) > place(second) && place(second).is_some()
        };
        until("the newer window is above", || above(other, pinned));
        // The window manager has finished its work for the two new windows.
        std::thread::sleep(Duration::from_millis(500));
        let focused = values(&connection, root, active).unwrap();

        assert_eq!(set_above(None, pinned, true), Ok(true));
        until("the window has the state", || {
            values(&connection, pinned, state)
                .unwrap()
                .contains(&state_above)
        });
        until("the window is above the newer one", || above(pinned, other));
        assert_eq!(values(&connection, root, active).unwrap(), focused);
        // A raise of the other window does not put it above the kept one.
        connection
            .configure_window(
                other,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .unwrap();
        connection.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(above(pinned, other));

        assert_eq!(set_above(None, pinned, false), Ok(true));
        until("the window lost the state", || {
            !values(&connection, pinned, state)
                .unwrap()
                .contains(&state_above)
        });
        assert_eq!(values(&connection, root, active).unwrap(), focused);
    }
}
