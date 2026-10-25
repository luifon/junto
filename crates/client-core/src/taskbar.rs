//! The properties of a window for the taskbar of Windows (ADR 0755). Windows groups a window
//! and the pins of the taskbar by the application user model identifier. A program that names
//! one for its process (ADR 0742) gets a taskbar button of its own, and a pin that Explorer made
//! from the program file does not have that identifier. The main window therefore states the
//! identifier, and the command, the name and the icon of a pin made from its taskbar button,
//! so that such a pin starts the program and stays one button with its running window.
//! Windows asks that the properties are taken away before the window is destroyed.

#![allow(unsafe_code)]

use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand,
    PKEY_AppUserModel_RelaunchDisplayNameResource, PKEY_AppUserModel_RelaunchIconResource,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyStore, PSCoerceToCanonicalValue, SHGetPropertyStoreForWindow,
};

/// What a pin made from the taskbar button of a window holds.
pub struct Relaunch<'a> {
    pub identifier: &'a str,
    /// The command line that the pin runs.
    pub command: &'a str,
    pub name: &'a str,
    /// A program and the index of its icon, as `C:\path\program.exe,0`.
    pub icon: &'a str,
}

/// The relaunch properties come before the identifier, which makes Windows read them.
const KEYS: [&PROPERTYKEY; 4] = [
    &PKEY_AppUserModel_RelaunchCommand,
    &PKEY_AppUserModel_RelaunchDisplayNameResource,
    &PKEY_AppUserModel_RelaunchIconResource,
    &PKEY_AppUserModel_ID,
];

fn error(error: windows::core::Error) -> std::io::Error {
    std::io::Error::other(error.message())
}

fn store(window: isize) -> std::io::Result<IPropertyStore> {
    // SAFETY: the caller gives the handle of a window of this process that exists during the
    // call; the function reads no other memory.
    unsafe { SHGetPropertyStoreForWindow(HWND(window as *mut core::ffi::c_void)) }.map_err(error)
}

/// Gives the window `window` (its `HWND`) the identifier and the relaunch properties.
pub fn set(window: isize, relaunch: &Relaunch<'_>) -> std::io::Result<()> {
    let store = store(window)?;
    let values = [
        relaunch.command,
        relaunch.name,
        relaunch.icon,
        relaunch.identifier,
    ];
    for (key, value) in KEYS.into_iter().zip(values) {
        let mut value = PROPVARIANT::from(value);
        // SAFETY: `key` and `value` are valid for the calls; the store copies the value.
        unsafe {
            PSCoerceToCanonicalValue(key, &mut value).map_err(error)?;
            store.SetValue(key, &value).map_err(error)?;
        }
    }
    // SAFETY: a call on a valid store.
    unsafe { store.Commit() }.map_err(error)
}

/// Takes the properties of `set` away from the window, before it is destroyed.
pub fn clear(window: isize) -> std::io::Result<()> {
    let store = store(window)?;
    let empty = PROPVARIANT::default();
    for key in KEYS.into_iter().rev() {
        // SAFETY: `key` and `empty` are valid for the call.
        unsafe { store.SetValue(key, &empty) }.map_err(error)?;
    }
    // SAFETY: a call on a valid store.
    unsafe { store.Commit() }.map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    fn read(window: isize, key: &PROPERTYKEY) -> String {
        let value = unsafe { store(window).unwrap().GetValue(key) }.unwrap();
        value.to_string()
    }

    #[test]
    fn a_window_states_the_identifier_and_the_pin_and_takes_them_away() {
        // SAFETY: the window is created hidden, used and destroyed on this thread.
        let window = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("taskbar test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                100,
                100,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let handle = window.0 as isize;
        let relaunch = Relaunch {
            identifier: "chat.junto.Test",
            command: r#""C:\Programs\Junto\junto.exe""#,
            name: "Junto",
            icon: r"C:\Programs\Junto\junto.exe,0",
        };
        set(handle, &relaunch).unwrap();
        assert_eq!(read(handle, &PKEY_AppUserModel_ID), "chat.junto.Test");
        assert_eq!(
            read(handle, &PKEY_AppUserModel_RelaunchCommand),
            relaunch.command
        );
        assert_eq!(
            read(handle, &PKEY_AppUserModel_RelaunchDisplayNameResource),
            "Junto"
        );
        assert_eq!(
            read(handle, &PKEY_AppUserModel_RelaunchIconResource),
            relaunch.icon
        );
        clear(handle).unwrap();
        assert_eq!(read(handle, &PKEY_AppUserModel_ID), "");
        assert_eq!(read(handle, &PKEY_AppUserModel_RelaunchCommand), "");
        unsafe { DestroyWindow(window) }.unwrap();
    }
}
