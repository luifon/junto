//! The console of a program of the Windows subsystem (ADR 0755). Windows gives such a program
//! no console, so a text that it writes to a standard stream that its parent did not redirect
//! is lost. A program that writes a text for the person instead of opening a window, such as
//! `junto --version`, attaches to the console of the process that started it, when that
//! process has one. The other systems have no such distinction.

/// Attaches the standard streams to the console of the parent process when the parent gave
/// the process neither a standard output nor a standard error. A redirected stream stays.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn attach_to_parent() {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    let unset = |handle: HANDLE| handle.is_null() || handle == INVALID_HANDLE_VALUE;
    // SAFETY: the functions take no pointer, and a failure leaves the process as it was.
    unsafe {
        if unset(GetStdHandle(STD_OUTPUT_HANDLE)) && unset(GetStdHandle(STD_ERROR_HANDLE)) {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
pub fn attach_to_parent() {}
