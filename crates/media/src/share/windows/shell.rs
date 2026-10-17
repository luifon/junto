//! The start of the application of the system for a link or a file by the Windows shell, so
//! that the application is not in the process tree of the client (ADR 0706).
//!
//! A share with all system sound leaves out the sound of the client's process and of every
//! process that it started. A browser that the client started for a link would be one of
//! them, and its sound would be missing from the share. The shell process, which shows the
//! desktop, starts the application instead: the client asks the window of the desktop to
//! open the target, and the process that results is a child of the shell.
//!
//! The target is the file of the call and its arguments are empty, so no part of the target
//! becomes an argument of the call. The shell puts the target in the place of `%1` in the
//! command that the application registered; a link has the form of ADR 0740, which holds no
//! quote and no space that could end that argument.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use junto_client_core::link::Link;
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

/// What the client opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A link of a message, with the application of the system for its scheme.
    Link(Link),
    /// A file of this device, with the application of the system for its type.
    File(PathBuf),
}

impl Target {
    fn as_os_str(&self) -> &OsStr {
        match self {
            Self::Link(link) => OsStr::new(link.as_str()),
            Self::File(path) => path.as_os_str(),
        }
    }
}

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
pub fn open(target: &Target) -> Result<Opened, String> {
    let target = target.as_os_str();
    match shell_execute(target, OsStr::new(""), true) {
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

/// Asks the shell to start a program with these arguments, shown or hidden, as `open` asks it
/// to open a target. Fails when no desktop of the shell runs in this session. Each argument
/// reaches the program as it is: the command line is quoted by the rules of
/// `CommandLineToArgvW`.
#[cfg(feature = "test-support")]
pub fn open_through_the_shell(
    program: &OsStr,
    arguments: &[&OsStr],
    shown: bool,
) -> Result<(), String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    let arguments: Vec<Vec<u16>> = arguments
        .iter()
        .map(|argument| argument.encode_wide().collect())
        .collect();
    let line = OsString::from_wide(&super::rules::command_line(&arguments));
    shell_execute(program, &line, shown)
}

/// Asks the shell to open a target with the verb "open" and a command line of arguments.
fn shell_execute(target: &OsStr, arguments: &OsStr, shown: bool) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::UI::Shell::CommandLineToArgvW;
    use windows::core::PCWSTR;

    use super::super::rules::command_line;

    /// The arguments that the system reads from the command line of a program.
    fn read_back(arguments: &[&str]) -> Vec<String> {
        let units: Vec<Vec<u16>> = arguments
            .iter()
            .map(|argument| argument.encode_utf16().collect())
            .collect();
        let mut line: Vec<u16> = "program.exe ".encode_utf16().collect();
        line.extend(command_line(&units));
        line.push(0);
        let mut count = 0;
        // SAFETY: the line ends with a zero; the array that the call returns holds `count`
        // strings that end with a zero, and it is freed once, after they were copied.
        unsafe {
            let read = CommandLineToArgvW(PCWSTR(line.as_ptr()), &mut count);
            assert!(!read.is_null());
            let found = (1..count as usize)
                .map(|index| {
                    let argument = *read.add(index);
                    String::from_utf16_lossy(argument.as_wide())
                })
                .collect();
            let _ = LocalFree(Some(HLOCAL(read.cast())));
            found
        }
    }

    #[test]
    fn the_system_reads_each_quoted_argument_as_it_was() {
        let cases: [&[&str]; 6] = [
            &["--report", r"C:\Temp\a b\report"],
            &["", "x", ""],
            &[r#"a"b"#, r#"a\"b"#, r"a b\", r"a\b\"],
            &["a\tb", "two  spaces"],
            &[r#"x" --attach "C:\a"#],
            &[r#"\\server\share\"#, r#"""#, r"\"],
        ];
        for arguments in cases {
            assert_eq!(read_back(arguments), arguments, "{arguments:?}");
        }
    }
}
