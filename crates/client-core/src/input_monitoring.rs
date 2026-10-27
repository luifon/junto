//! The Input Monitoring permission of macOS: a program that reads the keys while another
//! application is in front needs it (ADR 0708). Other systems keep no such permission.
//! The two functions of CoreGraphics are foreign functions, so the module allows unsafe code.

#![cfg_attr(target_os = "macos", allow(unsafe_code))]

/// Whether macOS lets this application read the keys of the system. It shows no question.
pub fn allowed() -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the function takes no argument and reads the permission of the process.
        unsafe { CGPreflightListenEventAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Makes macOS list this application in Input Monitoring and show its question when it
/// has not shown it before. Returns whether the permission is given now.
pub fn request() -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: as above; the system shows its question at most once.
        unsafe { CGRequestListenEventAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}
