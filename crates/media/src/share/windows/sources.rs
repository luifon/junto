//! The screens, windows and applications of Windows, read with the functions of the window
//! manager and of the process list. What is decided about them is in `rules`.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use super::rules::{self, FRAME_HOST, ListedProcess, ListedWindow};
use crate::share::listing::Own;
use crate::share::{ApplicationId, DisplaySource, ShareSources, WindowSource};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn text(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// The path of the program file of a process, or nothing for a process that this account
/// may not read or that has ended.
pub(super) fn process_path(process_id: u32) -> Option<String> {
    // SAFETY: the handle is closed before the function returns; the buffer outlives the
    // call that writes into it, and the call writes at most `size` units.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut buffer = vec![0u16; 32_768];
        let mut size = buffer.len() as u32;
        let read = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        read.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

/// The description in the version information of a program file.
pub(super) fn file_description(path: &str) -> Option<String> {
    let path = wide(path);
    // SAFETY: the version block is a buffer of the size that the system asked for, and the
    // pointers that `VerQueryValueW` gives point into it while it lives.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, block.as_mut_ptr().cast()).ok()?;
        let mut pointer: *mut c_void = std::ptr::null_mut();
        let mut length = 0u32;
        let translation = wide(r"\VarFileInfo\Translation");
        if !VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(translation.as_ptr()),
            &mut pointer,
            &mut length,
        )
        .as_bool()
            || length < 4
            || pointer.is_null()
        {
            return None;
        }
        let language = *pointer.cast::<u16>();
        let code_page = *pointer.cast::<u16>().add(1);
        let key = wide(&format!(
            r"\StringFileInfo\{language:04x}{code_page:04x}\FileDescription"
        ));
        if !VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut pointer,
            &mut length,
        )
        .as_bool()
            || length == 0
            || pointer.is_null()
        {
            return None;
        }
        let units = std::slice::from_raw_parts(pointer.cast::<u16>(), length as usize);
        Some(text(units))
    }
}

/// The client as the capture identifies it.
pub(super) fn own() -> Own {
    // SAFETY: the call reads the identifier of this process.
    let process_id = unsafe { GetCurrentProcessId() };
    let path = process_path(process_id).unwrap_or_default();
    rules::own(process_id, &path, file_description(&path).as_deref())
}

/// The processes of the system with the identifier of the process that created each.
fn processes() -> Vec<ListedProcess> {
    let mut processes = Vec::new();
    // SAFETY: the snapshot handle is closed before the function returns; each entry has the
    // size that its first field states.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return processes;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            processes.push(ListedProcess {
                id: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
            });
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    processes
}

/// The creation time of a process in units of 100 nanoseconds, or nothing for a process
/// that this account may not read or that has ended.
fn creation_time(process_id: u32) -> Option<u64> {
    // SAFETY: the handle is closed before the function returns; the call writes the four
    // times into the values.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let (mut created, mut ended, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let read = GetProcessTimes(process, &mut created, &mut ended, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        read.ok()?;
        Some(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
    }
}

/// The processes that started the client (ADR 0706).
pub(super) fn ancestors(own: &Own) -> Vec<u32> {
    rules::ancestors(own.process_id as u32, &processes(), creation_time)
}

/// The application of a process with its path and name.
pub(super) fn application_of(process_id: u32) -> Option<(ApplicationId, String)> {
    let path = process_path(process_id)?;
    Some(rules::application(
        process_id,
        &path,
        file_description(&path).as_deref(),
    ))
}

fn process_of(window: HWND) -> u32 {
    let mut process_id = 0u32;
    // SAFETY: the call writes the identifier of the process of the window into the value.
    unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    process_id
}

unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> BOOL {
    // SAFETY: `list` is the vector that the caller of the enumeration passed, and the
    // enumeration calls this function on the caller's thread during that call.
    let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
    list.push(window);
    TRUE
}

/// The process of the inner window of a frame of the store: the application that draws
/// into the frame. A suspended application has no inner window of another process.
fn inner_process(frame: HWND, frame_process: u32) -> Option<u32> {
    let mut children: Vec<HWND> = Vec::new();
    // SAFETY: `collect` receives the vector, which outlives the enumeration.
    unsafe {
        let _ = EnumChildWindows(
            Some(frame),
            Some(collect),
            LPARAM(&mut children as *mut Vec<HWND> as isize),
        );
    }
    children
        .into_iter()
        .map(process_of)
        .find(|process| *process != 0 && *process != frame_process)
}

fn read_window(window: HWND) -> ListedWindow {
    // SAFETY: each call reads a property of a window handle that the enumeration gave. A
    // window that closes meanwhile makes the calls fail or give empty values, which the rule
    // of the list then refuses.
    unsafe {
        let style = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
        let mut cloaked = 0u32;
        let cloaked = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0;
        let mut bounds = RECT::default();
        let size = if DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut bounds as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
        {
            (
                (bounds.right - bounds.left).max(0) as u32,
                (bounds.bottom - bounds.top).max(0) as u32,
            )
        } else {
            (0, 0)
        };
        let length = GetWindowTextLengthW(window).max(0) as usize;
        let mut title = vec![0u16; length + 1];
        let written = GetWindowTextW(window, &mut title).max(0) as usize;
        let mut class = vec![0u16; 256];
        let class_length = GetClassNameW(window, &mut class).max(0) as usize;
        let mut process = process_of(window);
        let path = process_path(process);
        if path
            .as_deref()
            .is_some_and(|path| rules::program_identity(path).1 == FRAME_HOST)
        {
            process = inner_process(window, process).unwrap_or(0);
        }
        ListedWindow {
            id: rules::window_number(window.0 as isize),
            title: String::from_utf16_lossy(&title[..written.min(length)]),
            class: String::from_utf16_lossy(&class[..class_length.min(class.len())]),
            visible: IsWindowVisible(window).as_bool(),
            cloaked,
            minimized: IsIconic(window).as_bool(),
            tool: style & WS_EX_TOOLWINDOW.0 != 0,
            app_window: style & WS_EX_APPWINDOW.0 != 0,
            no_activate: style & WS_EX_NOACTIVATE.0 != 0,
            owned: GetWindow(window, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()),
            size,
            application: if process == 0 {
                None
            } else {
                application_of(process)
            },
        }
    }
}

/// The top-level windows in the order of the window manager, from the front.
fn top_level_windows() -> Vec<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `collect` receives the vector, which outlives the enumeration.
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        );
    }
    windows
}

/// The windows that a person can share now.
pub(super) fn windows(own: &Own) -> Vec<WindowSource> {
    let listed = top_level_windows().into_iter().map(read_window).collect();
    rules::shareable_windows(listed, own)
}

/// A screen with its handle.
pub(super) struct Display {
    pub source: DisplaySource,
    pub monitor: HMONITOR,
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    list: LPARAM,
) -> BOOL {
    // SAFETY: `list` is the vector that the caller of the enumeration passed, and the
    // enumeration calls this function on the caller's thread during that call.
    let list = unsafe { &mut *(list.0 as *mut Vec<HMONITOR>) };
    list.push(monitor);
    TRUE
}

/// The screens with their size in pixels, named by the number of their device.
pub(super) fn displays() -> Vec<Display> {
    let mut monitors: Vec<HMONITOR> = Vec::new();
    // SAFETY: `collect_monitor` receives the vector, which outlives the enumeration. The
    // information structures have the size that their first field states.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut monitors as *mut Vec<HMONITOR> as isize),
        );
        monitors
            .into_iter()
            .filter_map(|monitor| {
                let mut info = MONITORINFOEXW::default();
                info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                if !GetMonitorInfoW(
                    monitor,
                    (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
                )
                .as_bool()
                {
                    return None;
                }
                let id = rules::display_number(&text(&info.szDevice))?;
                // The mode of the device is in pixels, whatever the scale of the display
                // and whatever this process states about it.
                let mut mode = DEVMODEW {
                    dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                    ..Default::default()
                };
                if !EnumDisplaySettingsW(
                    PCWSTR(info.szDevice.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &mut mode,
                )
                .as_bool()
                    || mode.dmPelsWidth == 0
                    || mode.dmPelsHeight == 0
                {
                    return None;
                }
                Some(Display {
                    source: DisplaySource {
                        id,
                        width: mode.dmPelsWidth,
                        height: mode.dmPelsHeight,
                    },
                    monitor,
                })
            })
            .collect()
    }
}

/// What a person can share now. Process loopback, which carries both choices of sound,
/// exists on every version of Windows 11 (ADR 0705, ADR 0706).
pub(super) fn sources(own: &Own) -> ShareSources {
    let windows = windows(own);
    let (applications, started_the_client) = rules::sound_choices(&windows, own, &ancestors(own));
    ShareSources {
        displays: displays()
            .into_iter()
            .map(|display| display.source)
            .collect(),
        windows,
        applications,
        portal: Vec::new(),
        system_sound: true,
        started_the_client,
    }
}
