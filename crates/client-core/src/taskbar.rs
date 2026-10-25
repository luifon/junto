//! The properties of a window for the taskbar of Windows (ADR 0755). Windows groups a window
//! and the pins of the taskbar by the application user model identifier. A program that names
//! one for its process (ADR 0742) gets a taskbar button of its own, and a pin that Explorer made
//! from the program file does not have that identifier. The main window therefore states the
//! identifier, and the command, the name and the icon of a pin made from its taskbar button,
//! so that such a pin starts the program and stays one button with its running window.
//! Windows asks that the properties are taken away before the window is destroyed. A pin that
//! Explorer made from the program file is a shortcut without the identifier; the program gives
//! it the identifier at its start ([`mark_pins`]), as the shortcut of a pin made from the
//! running window has it.

#![allow(unsafe_code)]

use std::os::windows::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

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

/// The folder in which Explorer keeps the shortcuts of the taskbar pins of the account.
pub fn pins_directory() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|roaming| {
        PathBuf::from(roaming).join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar")
    })
}

/// Gives each shortcut in `directory` that starts `program` the identifier `identifier`, so
/// that the taskbar groups the pin with the running window. Returns the number of shortcuts
/// that were changed. A shortcut that cannot be read is left as it is. The calling thread must
/// be in an apartment of COM.
pub fn mark_pins(directory: &Path, program: &Path, identifier: &str) -> std::io::Result<usize> {
    let program = std::fs::canonicalize(program)?;
    let mut changed = 0;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            continue;
        }
        match mark_pin(&path, &program, identifier) {
            Ok(true) => changed += 1,
            Ok(false) => {}
            Err(error) => tracing::debug!(?path, %error, "a taskbar pin was not read"),
        }
    }
    Ok(changed)
}

/// Runs [`mark_pins`] for the pins of the account and the running program, on a thread of its
/// own in the multithreaded apartment, which it leaves at the end.
pub fn mark_pins_of_this_program(identifier: &'static str) {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    let (Some(directory), Ok(program)) = (pins_directory(), std::env::current_exe()) else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("junto-pins".to_owned())
        .spawn(move || {
            // SAFETY: the thread enters the apartment, uses COM and leaves it.
            if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
                return;
            }
            match mark_pins(&directory, &program, identifier) {
                Ok(0) => {}
                Ok(changed) => tracing::info!(changed, "taskbar pins of the program file named"),
                Err(error) => tracing::debug!(%error, "the taskbar pins were not read"),
            }
            // SAFETY: balances the successful call above on the same thread.
            unsafe { CoUninitialize() };
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the taskbar pins were not read");
    }
}

fn mark_pin(path: &Path, program: &Path, identifier: &str) -> std::io::Result<bool> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READWRITE,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface as _, PCWSTR};
    let name = HSTRING::from(path.as_os_str());
    // SAFETY: the interfaces are created and used on this thread, which the caller put in an
    // apartment; the buffer outlives the call that fills it.
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(error)?;
        let file: IPersistFile = link.cast().map_err(error)?;
        file.Load(&name, STGM_READWRITE).map_err(error)?;
        let mut target = vec![0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), 0)
            .map_err(error)?;
        let end = target.iter().position(|unit| *unit == 0).unwrap_or(0);
        let target = PathBuf::from(std::ffi::OsString::from_wide(&target[..end]));
        if std::fs::canonicalize(target).ok().as_deref() != Some(program) {
            return Ok(false);
        }
        let store: IPropertyStore = link.cast().map_err(error)?;
        if store
            .GetValue(&PKEY_AppUserModel_ID)
            .map_err(error)?
            .to_string()
            == identifier
        {
            return Ok(false);
        }
        let mut value = PROPVARIANT::from(identifier);
        PSCoerceToCanonicalValue(&PKEY_AppUserModel_ID, &mut value).map_err(error)?;
        store
            .SetValue(&PKEY_AppUserModel_ID, &value)
            .map_err(error)?;
        store.Commit().map_err(error)?;
        file.Save(PCWSTR::null(), true).map_err(error)?;
    }
    Ok(true)
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

    /// A shortcut that starts `target`, as Explorer writes a pin made from a program file.
    fn shortcut(path: &Path, target: &Path) {
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::core::{HSTRING, Interface as _};
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Save(&HSTRING::from(path.as_os_str()), true).unwrap();
        }
    }

    fn identifier_of(path: &Path) -> String {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
        };
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::core::{HSTRING, Interface as _};
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Load(&HSTRING::from(path.as_os_str()), STGM_READ)
                .unwrap();
            let store: IPropertyStore = link.cast().unwrap();
            store.GetValue(&PKEY_AppUserModel_ID).unwrap().to_string()
        }
    }

    #[test]
    fn a_pin_of_the_program_file_gets_the_identifier_and_other_pins_stay() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let pins = tempfile::tempdir().unwrap();
        let programs = tempfile::tempdir().unwrap();
        let (program, other) = (
            programs.path().join("junto.exe"),
            programs.path().join("other.exe"),
        );
        std::fs::write(&program, b"").unwrap();
        std::fs::write(&other, b"").unwrap();
        let (ours, theirs) = (pins.path().join("Junto.lnk"), pins.path().join("Other.lnk"));
        shortcut(&ours, &program);
        shortcut(&theirs, &other);
        assert_eq!(identifier_of(&ours), "");
        assert_eq!(
            mark_pins(pins.path(), &program, "chat.junto.Test").unwrap(),
            1
        );
        assert_eq!(identifier_of(&ours), "chat.junto.Test");
        assert_eq!(identifier_of(&theirs), "");
        assert_eq!(
            mark_pins(pins.path(), &program, "chat.junto.Test").unwrap(),
            0
        );
    }
}
