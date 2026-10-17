//! The free space of a directory on Windows (ADR 0605). This is the only module of the
//! server with unsafe code.

#![allow(unsafe_code)]

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// The bytes that the user of the process can still write on the volume of `directory`,
/// after the disk quotas of that user.
pub fn free_bytes(directory: &Path) -> io::Result<u64> {
    let mut name: Vec<u16> = directory.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path of the directory contains a zero character",
        ));
    }
    name.push(0);
    let mut available: u64 = 0;
    // SAFETY: `name` is a wide string that ends with its only zero and lives until the call
    // returns; `available` is a valid place for one u64; the two totals are not asked for,
    // which the function allows with null pointers.
    let done = unsafe {
        GetDiskFreeSpaceExW(
            name.as_ptr(),
            &mut available,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if done == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}
