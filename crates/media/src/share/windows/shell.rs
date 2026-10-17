//! The start of the application of the system for a link or a file by the Windows shell, so
//! that the application is not in the process tree of the client (ADR 0706).
//!
//! A share with all system sound leaves out the sound of the client's process and of every
//! process that it started. A browser that the client started for a link would be one of
//! them, and its sound would be missing from the share. The shell process, which shows the
//! desktop, starts the application instead: the client asks the window of the desktop to
//! open the target, and the process that results is a child of the shell.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IDispatch, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    CSIDL_DESKTOP, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows,
    SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellExecuteW,
    ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use windows::core::{BSTR, HSTRING, Interface, w};

/// Which process started the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// The shell: the application is not in the client's tree.
    ByTheShell,
    /// The client itself, because the shell could not be asked: the application is in the
    /// client's tree.
    ByTheClient,
}

/// Opens a link or a file with the application of the system for it, started by the shell,
/// or by the client when no shell answers. Blocks until the start was asked for.
pub fn open(target: &OsStr) -> Result<Opened, String> {
    match open_through_the_shell(target, OsStr::new(""), true) {
        Ok(()) => Ok(Opened::ByTheShell),
        Err(reason) => {
            tracing::warn!(%reason, "the shell did not open the target; the client opens it");
            let target = HSTRING::from(target);
            // SAFETY: the strings outlive the call.
            let result =
                unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
            // A value above 32 is success.
            if result.0 as usize > 32 {
                Ok(Opened::ByTheClient)
            } else {
                Err(format!(
                    "the system did not open the target ({})",
                    result.0 as usize
                ))
            }
        }
    }
}

/// Asks the shell to open a target with the verb "open", with arguments for a program, shown
/// or hidden. Fails when no desktop of the shell runs in this session.
pub fn open_through_the_shell(
    target: &OsStr,
    arguments: &OsStr,
    shown: bool,
) -> Result<(), String> {
    // SAFETY: the apartment entered here is left before the return, after every interface of
    // the call has been released.
    let entered = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let result = ask_the_shell(target, arguments, shown).map_err(|error| error.message());
    if entered {
        // SAFETY: matches the successful entry above.
        unsafe { CoUninitialize() };
    }
    result
}

fn ask_the_shell(target: &OsStr, arguments: &OsStr, shown: bool) -> windows::core::Result<()> {
    // SAFETY: each call receives valid interfaces and values that outlive it; the interfaces
    // are released when dropped, before the apartment is left.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        let desktop = VARIANT::from(CSIDL_DESKTOP as i32);
        let mut window = 0i32;
        let found: IDispatch = windows.FindWindowSW(
            &desktop,
            &VARIANT::default(),
            SWC_DESKTOP,
            &mut window,
            SWFO_NEEDDISPATCH,
        )?;
        let provider: IServiceProvider = found.cast()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
        let view: IShellView = browser.QueryActiveShellView()?;
        let background: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
        let folder: IShellFolderViewDual = background.cast()?;
        let shell: IShellDispatch2 = folder.Application()?.cast()?;
        let bstr = |text: &OsStr| BSTR::from_wide(&text.encode_wide().collect::<Vec<u16>>());
        shell.ShellExecute(
            &bstr(target),
            &VARIANT::from(bstr(arguments)),
            &VARIANT::from(""),
            &VARIANT::from("open"),
            &VARIANT::from(if shown { SW_SHOWNORMAL.0 } else { 0 }),
        )
    }
}
