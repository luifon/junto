//! The console of a program of the Windows subsystem (ADR 0755). Windows gives such a program
//! no console, so a text that it writes to a standard stream that its parent did not redirect
//! is lost. A program that writes a text for the person instead of opening a window, such as
//! `junto --version`, attaches to the console of the process that started it, when that
//! process has one. The other systems have no such distinction.

/// Attaches the standard streams to the console of the parent process when the parent gave
/// the process neither a standard output nor a standard error. A redirected stream stays.
/// Returns whether standard error then has a reader: a redirected stream or a console. A
/// program that Explorer started has neither.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn attach_to_parent() -> bool {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    let unset = |handle: HANDLE| handle.is_null() || handle == INVALID_HANDLE_VALUE;
    // SAFETY: the functions take no pointer, and a failure leaves the process as it was.
    unsafe {
        if !unset(GetStdHandle(STD_ERROR_HANDLE)) {
            return true;
        }
        if unset(GetStdHandle(STD_OUTPUT_HANDLE)) && AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            return !unset(GetStdHandle(STD_ERROR_HANDLE));
        }
        false
    }
}

#[cfg(not(windows))]
pub fn attach_to_parent() -> bool {
    true
}

/// Shows a message box of the system with an error, and returns when the person closed it. A
/// program of the Windows subsystem that fails before its window has no other way to tell a
/// person who started it from Explorer (ADR 0757).
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn show_error(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };
    let wide = |text: &str| {
        text.encode_utf16()
            .map(|unit| if unit == 0 { u16::from(b' ') } else { unit })
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings end with a zero and live until the call returns; no owner window.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
pub fn show_error(_title: &str, _text: &str) {}
