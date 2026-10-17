//! The process that created the fixture, read from the process list of the system.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

/// `<identifier> <path>` of the process that created this one; the path is empty when it
/// cannot be read.
pub fn creator() -> String {
    // SAFETY: the handles are closed before the function returns; each entry has the size
    // that its first field states, and the buffer outlives the call that writes into it.
    unsafe {
        let own = GetCurrentProcessId();
        let mut parent = 0;
        if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
            while more {
                if entry.th32ProcessID == own {
                    parent = entry.th32ParentProcessID;
                }
                more = Process32NextW(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        let mut path = String::new();
        if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, parent) {
            let mut buffer = vec![0u16; 32_768];
            let mut size = buffer.len() as u32;
            if QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
            .is_ok()
            {
                path = String::from_utf16_lossy(&buffer[..size as usize]);
            }
            let _ = CloseHandle(process);
        }
        format!("{parent} {path}")
    }
}
