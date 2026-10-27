//! The operations of Windows of the setup program (ADR 0728): the entry of the list of
//! installed programs, the Start menu shortcut, the questions to the person, and the
//! instances of Junto that run.
//! The shortcut (COM), the questions (`MessageBoxW`) and the list of processes call the
//! Windows API, so the module allows unsafe code; the decisions of the setup program are in
//! `setup.rs`, without unsafe code, and their tests run on every system.

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};

use super::setup::{SetupSystem, UNINSTALL_KEY, UninstallEntry};

/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`.
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

fn error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// The operations of the account that runs the setup program.
pub struct WindowsSetup;

impl SetupSystem for WindowsSetup {
    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> std::io::Result<()> {
        let key = windows_registry::CURRENT_USER
            .create(UNINSTALL_KEY)
            .map_err(error)?;
        for (name, value) in &entry.strings {
            key.set_string(*name, value).map_err(error)?;
        }
        for (name, value) in &entry.numbers {
            key.set_u32(*name, *value).map_err(error)?;
        }
        Ok(())
    }

    fn uninstall_entry_location(&self) -> std::io::Result<Option<PathBuf>> {
        match windows_registry::CURRENT_USER
            .options()
            .read()
            .open(UNINSTALL_KEY)
        {
            Ok(key) => Ok(key.get_string("InstallLocation").ok().map(PathBuf::from)),
            Err(failure) if failure.code().0 == NOT_FOUND => Ok(None),
            Err(failure) => Err(error(failure)),
        }
    }

    fn remove_uninstall_entry(&self) -> std::io::Result<()> {
        match windows_registry::CURRENT_USER.remove_tree(UNINSTALL_KEY) {
            Ok(()) => Ok(()),
            Err(failure) if failure.code().0 == NOT_FOUND => Ok(()),
            Err(failure) => Err(error(failure)),
        }
    }

    fn create_shortcut(&self, link: &Path, target: &Path) -> std::io::Result<()> {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        shortcut(link, target, crate::app_identity::APP_ID)
    }

    fn unregister_notifications(&self) -> std::io::Result<()> {
        crate::app_identity::unregister(crate::app_identity::APP_ID).map(|_| ())
    }
}

/// Writes a shortcut that starts `target` from its folder and names the application
/// identifier, so that the window of a start from the Start menu groups with its taskbar
/// button (ADR 0755).
fn shortcut(link: &Path, target: &Path, identifier: &str) -> std::io::Result<()> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSCoerceToCanonicalValue};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface as _};
    let directory = target.parent().unwrap_or(target);
    // SAFETY: COM is used on this thread between its initialisation and its end; every
    // string outlives the call that reads it.
    unsafe {
        let initialised = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> windows::core::Result<()> {
            let link_object: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link_object.SetPath(&HSTRING::from(target.as_os_str()))?;
            link_object.SetWorkingDirectory(&HSTRING::from(directory.as_os_str()))?;
            link_object.SetDescription(&HSTRING::from("Junto"))?;
            let store: IPropertyStore = link_object.cast()?;
            let mut value = PROPVARIANT::from(identifier);
            PSCoerceToCanonicalValue(&PKEY_AppUserModel_ID, &mut value)?;
            store.SetValue(&PKEY_AppUserModel_ID, &value)?;
            store.Commit()?;
            let file: IPersistFile = link_object.cast()?;
            file.Save(&HSTRING::from(link.as_os_str()), true)
        })();
        if initialised {
            CoUninitialize();
        }
        result.map_err(error)
    }
}

/// The answer of the person to a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

/// The buttons of a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    OkCancel,
    YesNo,
    RetryCancel,
}

/// Shows a message box of the system and returns the answer.
pub fn ask(title: &str, text: &str, buttons: Buttons, warning: bool) -> Answer {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDOK, IDRETRY, IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_OKCANCEL,
        MB_RETRYCANCEL, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
    };
    use windows::core::HSTRING;
    let style = match buttons {
        Buttons::Ok => MB_OK,
        Buttons::OkCancel => MB_OKCANCEL,
        Buttons::YesNo => MB_YESNO,
        Buttons::RetryCancel => MB_RETRYCANCEL,
    } | if warning {
        MB_ICONWARNING
    } else {
        MB_ICONINFORMATION
    } | MB_SETFOREGROUND;
    // SAFETY: the strings outlive the call, which has no owner window.
    let answer = unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), style) };
    if answer == IDOK || answer == IDYES || answer == IDRETRY {
        Answer::Yes
    } else if buttons == Buttons::YesNo {
        Answer::No
    } else {
        Answer::Cancel
    }
}

/// Whether a process runs from this program file.
pub fn runs(program: &Path) -> bool {
    let Ok(program) = program.canonicalize() else {
        return false;
    };
    processes()
        .into_iter()
        .any(|path| path.canonicalize().is_ok_and(|running| running == program))
}

/// The program files of the processes that the account can read.
fn processes() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    let mut found = Vec::new();
    // SAFETY: the snapshot and each process handle are closed after their use; the buffers
    // outlive the calls that fill them.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return found;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            if let Ok(process) = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                entry.th32ProcessID,
            ) {
                let mut name = vec![0u16; 32768];
                let mut length = name.len() as u32;
                if QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    windows::core::PWSTR(name.as_mut_ptr()),
                    &mut length,
                )
                .is_ok()
                {
                    found.push(PathBuf::from(std::ffi::OsString::from_wide(
                        &name[..length as usize],
                    )));
                }
                let _ = CloseHandle(process);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

/// Starts a program without a console, and does not wait for it.
pub fn start(program: &Path, arguments: &[&std::ffi::OsStr]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new(program);
    command.args(arguments).creation_flags(CREATE_NO_WINDOW);
    if let Some(folder) = program
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        command.current_dir(folder);
    }
    command.spawn().map(|_| ())
}

/// Waits until the process with this number has ended, at most `limit`. True when it ended
/// or does not exist.
pub fn wait_for_end(process: u32, limit: std::time::Duration) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let milliseconds = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: the handle is closed after the wait.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, process) else {
            return true;
        };
        let ended = WaitForSingleObject(handle, milliseconds) == WAIT_OBJECT_0;
        let _ = CloseHandle(handle);
        ended
    }
}

/// Removes `file` after the process with this number has ended, from a hidden Windows
/// PowerShell that waits for that end: a running program file cannot be removed. The path
/// and the number reach it as environment variables, so no quoting applies.
pub fn remove_after_end(file: &Path, process: u32) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let shell = Path::new(&system).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    std::process::Command::new(shell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Wait-Process -Id $env:JUNTO_REMOVE_AFTER -Timeout 120 -ErrorAction SilentlyContinue; \
             Remove-Item -LiteralPath $env:JUNTO_REMOVE_FILE -Force",
        ])
        .env("JUNTO_REMOVE_AFTER", process.to_string())
        .env("JUNTO_REMOVE_FILE", file)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// The entry of the list of installed programs states the version of the folder: an update
/// from inside the application replaced the folder (ADR 0728). Only an entry of this folder
/// is changed.
pub fn refresh_uninstall_version(folder: &Path, version: &str) -> std::io::Result<()> {
    let key = match windows_registry::CURRENT_USER
        .options()
        .read()
        .write()
        .open(UNINSTALL_KEY)
    {
        Ok(key) => key,
        Err(failure) if failure.code().0 == NOT_FOUND => return Ok(()),
        Err(failure) => return Err(error(failure)),
    };
    let location = key.get_string("InstallLocation").unwrap_or_default();
    let same = Path::new(&location)
        .canonicalize()
        .ok()
        .zip(folder.canonicalize().ok())
        .is_some_and(|(location, folder)| location == folder);
    if !same || key.get_string("DisplayVersion").ok().as_deref() == Some(version) {
        return Ok(());
    }
    key.set_string("DisplayVersion", version).map_err(error)
}
