//! The wait for the end of another process on Windows (ADR 0008, ADR 0915). One of the two
//! modules of the server with unsafe code.

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

/// Returns when the process with the number `number` has ended. Returns at once when no such
/// process runs, or when it was created after this process: then the process that started this
/// one has ended, and Windows gave its number to another process.
pub fn wait_for_end(number: u32) {
    // SAFETY: OpenProcess takes no pointer; a null result is handled.
    let process = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            number,
        )
    };
    if process.is_null() {
        return;
    }
    // SAFETY: GetCurrentProcess takes no argument and returns a pseudo handle that needs no
    // closing.
    let own = unsafe { GetCurrentProcess() };
    if let (Some(created), Some(own_created)) = (creation_time(process), creation_time(own))
        && created < own_created
    {
        // SAFETY: `process` is an open handle with the right SYNCHRONIZE until it is closed
        // below.
        unsafe { WaitForSingleObject(process, INFINITE) };
    }
    // SAFETY: `process` is an open handle that this function owns and closes once.
    unsafe { CloseHandle(process) };
}

/// The creation time of a process in 100 ns units since 1601.
fn creation_time(process: HANDLE) -> Option<u64> {
    let empty = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let [mut created, mut exited, mut kernel, mut user] = [empty; 4];
    // SAFETY: `process` is a handle with PROCESS_QUERY_LIMITED_INFORMATION, and the four
    // pointers are valid places for one FILETIME each during the call.
    let done =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    (done != 0)
        .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}
