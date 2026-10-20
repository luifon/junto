//! Owners and access lists on Windows (ADR 0711), and the rename that never replaces a file,
//! which the standard library does not offer. A private directory is created with the user
//! of the process as its owner and a protected access list that has one entry: full access
//! for that user, inherited by everything inside. The security of an object is read from its
//! opened handle. This is the only module of the library with unsafe code. The tests of the
//! storage run it on Windows in continuous integration (ADR 0905).

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetTokenInformation, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid,
    WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, GetFileInformationByHandle, GetFullPathNameW,
    MoveFileExW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::policy::{Entry, Owner};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

/// Memory that the system allocated and that `LocalFree` releases.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a system call that documents `LocalFree`.
            unsafe { LocalFree(self.0) };
        }
    }
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle came from `OpenProcessToken` and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The security identifier of the user of this process, inside the buffer that holds it.
struct UserSid {
    buffer: Vec<u64>,
}

impl UserSid {
    fn of_this_process() -> io::Result<Self> {
        let mut handle: HANDLE = ptr::null_mut();
        // SAFETY: the pseudo handle of the process is always valid, and `handle` is a
        // valid place for the result.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Token(handle);
        let mut length = 0u32;
        // SAFETY: a null buffer of length 0 asks for the needed length only.
        unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut length) };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        // SAFETY: the buffer has at least `length` bytes and the alignment of a pointer.
        let filled = unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        };
        if filled == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn sid(&self) -> PSID {
        // SAFETY: `GetTokenInformation` wrote a `TOKEN_USER` at the start of the buffer,
        // and its pointer refers to memory inside the same buffer.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn text(&self) -> io::Result<String> {
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: the identifier is valid while `self` lives, and `text` receives memory
        // that `Local` releases.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = Local(text.cast());
        let mut length = 0;
        // SAFETY: the system returned a string that ends with a zero.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` units before the zero belong to the string.
        let units = unsafe { std::slice::from_raw_parts(text, length) };
        let result = String::from_utf16(units)
            .map_err(|_| io::Error::other("the security identifier is not text"));
        drop(owned);
        result
    }
}

const SEPARATOR: u16 = b'\\' as u16;
const OTHER_SEPARATOR: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;
const VERBATIM: &[u16] = &[SEPARATOR, SEPARATOR, b'?' as u16, SEPARATOR];
const NT: &[u16] = &[SEPARATOR, b'?' as u16, b'?' as u16, SEPARATOR];
const DEVICE: &[u16] = &[SEPARATOR, SEPARATOR, b'.' as u16, SEPARATOR];
const UNC: &[u16] = &[
    SEPARATOR,
    SEPARATOR,
    b'?' as u16,
    SEPARATOR,
    b'U' as u16,
    b'N' as u16,
    b'C' as u16,
    SEPARATOR,
];
/// The length from which `CreateDirectoryW` refuses a path that has no `\\?\` prefix.
const LEGACY_LIMIT: usize = 248;

/// A path for a call of the Windows API, ending with a zero. A path that is not short and
/// absolute becomes absolute with the prefix `\\?\`, which has no limit of 260 characters, as
/// the standard library converts the paths of its own calls.
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path contains a zero",
        ));
    }
    units.push(0);
    if units.starts_with(VERBATIM) || units.starts_with(NT) || units == [0] {
        return Ok(units);
    }
    if units.len() < LEGACY_LIMIT {
        match units.as_slice() {
            [drive, COLON, 0] | [drive, COLON, SEPARATOR | OTHER_SEPARATOR, ..]
                if *drive != SEPARATOR && *drive != OTHER_SEPARATOR =>
            {
                return Ok(units);
            }
            [SEPARATOR | OTHER_SEPARATOR, SEPARATOR | OTHER_SEPARATOR, ..] => return Ok(units),
            _ => {}
        }
    }
    let absolute = full_path(&units)?;
    let (prefix, rest): (&[u16], &[u16]) = match absolute.as_slice() {
        [_, COLON, SEPARATOR, ..] => (VERBATIM, &absolute),
        rest if rest.starts_with(DEVICE) => (VERBATIM, &rest[DEVICE.len()..]),
        rest if rest.starts_with(VERBATIM) || rest.starts_with(NT) => (&[], rest),
        [SEPARATOR, SEPARATOR, rest @ ..] => (UNC, rest),
        rest => (&[], rest),
    };
    let mut converted = Vec::with_capacity(prefix.len() + rest.len() + 1);
    converted.extend_from_slice(prefix);
    converted.extend_from_slice(rest);
    converted.push(0);
    Ok(converted)
}

/// The absolute and normalized form of a path that ends with a zero, without the zero.
fn full_path(path: &[u16]) -> io::Result<Vec<u16>> {
    let mut buffer = vec![0u16; 512];
    loop {
        let size = u32::try_from(buffer.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: the path ends with a zero, and the buffer has `size` units.
        let length =
            unsafe { GetFullPathNameW(path.as_ptr(), size, buffer.as_mut_ptr(), ptr::null_mut()) }
                as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < buffer.len() {
            buffer.truncate(length);
            return Ok(buffer);
        }
        buffer.resize(length, 0);
    }
}

/// Creates one directory that the user of this process owns and whose access list has a
/// single entry: full access for that user, inherited by the files and directories inside.
/// The list is protected, so nothing is inherited from the parent. Fails when the
/// directory exists.
pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    let user = UserSid::of_this_process()?;
    let sid = user.text()?;
    let description: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the description ends with a zero, and `descriptor` receives memory that
    // `Local` releases.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            description.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = Local(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let path = wide(path)?;
    // SAFETY: the path ends with a zero, and the attributes and the descriptor live until
    // the call returns.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Renames a file and fails when a file has the new name.
pub(super) fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let from = wide(from)?;
    let to = wide(to)?;
    // SAFETY: both paths end with a zero and live until the call returns.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The owner and the access list of an opened file or directory.
pub(super) struct Security {
    pub(super) owner: Owner,
    /// `None` when the object has no access list, which gives everybody access.
    pub(super) entries: Option<Vec<Entry>>,
}

/// Reads the security of the object behind `file` from its handle, so that the answer is
/// about the object that was opened and not about what its path names now.
pub(super) fn security_of(file: &File) -> io::Result<Security> {
    let user = UserSid::of_this_process()?;
    let mut owner: PSID = ptr::null_mut();
    let mut list: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle is valid while `file` lives. `owner` and `list` point into
    // `descriptor`, which `Local` releases after their last use.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut list,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = Local(descriptor);
    if owner.is_null() {
        return Err(io::Error::other("the object has no owner"));
    }
    // SAFETY: both identifiers are valid: one inside the descriptor, one inside `user`.
    let owner = if unsafe { EqualSid(owner, user.sid()) } != 0 {
        Owner::User
    // SAFETY: the identifier is valid and the second argument is a constant of the system.
    } else if unsafe { IsWellKnownSid(owner, WinBuiltinAdministratorsSid) } != 0
        // SAFETY: as above.
        || unsafe { IsWellKnownSid(owner, WinLocalSystemSid) } != 0
    {
        Owner::Administration
    } else {
        Owner::Other
    };
    if list.is_null() {
        return Ok(Security {
            owner,
            entries: None,
        });
    }
    // SAFETY: the system returned a valid access list.
    let count = unsafe { (*list).AceCount };
    let mut entries = Vec::with_capacity(usize::from(count));
    for index in 0..u32::from(count) {
        let mut entry: *mut c_void = ptr::null_mut();
        // SAFETY: `index` is below the number of entries of the list.
        if unsafe { GetAce(list, index, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: every entry starts with a header.
        let header = unsafe { *entry.cast::<ACE_HEADER>() };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            entries.push(Entry {
                allows: false,
                for_user: false,
            });
            continue;
        }
        // SAFETY: an entry of this type is an `ACCESS_ALLOWED_ACE`, whose identifier
        // starts at the field `SidStart`.
        let sid: PSID =
            unsafe { ptr::addr_of_mut!((*entry.cast::<ACCESS_ALLOWED_ACE>()).SidStart) }.cast();
        entries.push(Entry {
            allows: true,
            // SAFETY: both identifiers are valid: one inside the descriptor, one inside
            // `user`.
            for_user: unsafe { EqualSid(sid, user.sid()) } != 0,
        });
    }
    Ok(Security {
        owner,
        entries: Some(entries),
    })
}

/// The volume serial number and the file index of an opened file or directory. The two
/// name one object on this machine, whatever path was used to open it.
pub(super) fn identity_of(file: &File) -> io::Result<(u64, u64)> {
    // SAFETY: the structure is plain data, for which all zeros is a valid value.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is valid while `file` lives, and `information` is a valid place
    // for the result.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((u64::from(information.dwVolumeSerialNumber), index))
}
