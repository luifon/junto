//! Single-component operations relative to an already-open directory.
use super::*;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_DISPOSITION_INFORMATION, FILE_INFORMATION_CLASS,
    FILE_NAMES_INFORMATION, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
    FILE_SYNCHRONOUS_IO_NONALERT, FileDispositionInformation, FileNamesInformation,
    FileRenameInformation, NtCreateFile, NtQueryDirectoryFile, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

fn status(result: i32) -> io::Result<()> {
    if result >= 0 {
        return Ok(());
    }
    // SAFETY: conversion takes an NTSTATUS value and borrows no memory.
    Err(io::Error::from_raw_os_error(
        unsafe { RtlNtStatusToDosError(result) } as i32,
    ))
}

fn name(name: &OsStr) -> io::Result<Vec<u16>> {
    let name: Vec<_> = name.encode_wide().collect();
    if name.is_empty()
        || name == [46]
        || name == [46, 46]
        || name.iter().any(|v| [0, 47, 92, 58].contains(v))
    {
        return Err(invalid("not one stored entry name"));
    }
    Ok(name)
}

pub(super) fn open(
    parent: &File,
    child: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    sharing: u32,
) -> io::Result<File> {
    let mut name = name(child)?;
    let length = u16::try_from(name.len() * 2).map_err(|_| invalid("a stored name is too long"))?;
    let text = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &text,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let file_attributes = if options & FILE_DIRECTORY_FILE != 0 {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    let mut handle = std::ptr::null_mut();
    let mut completed = IO_STATUS_BLOCK::default();
    // SAFETY: the parent, UTF-16 name and output parameters live through this synchronous
    // call; one relative component with OPEN_REPARSE_POINT cannot traverse a child link.
    status(unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut completed,
            std::ptr::null(),
            file_attributes,
            sharing,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    })?;
    if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::other(
            "a successful stored-file open returned no handle",
        ));
    }
    // SAFETY: successful NtCreateFile transfers one owned file handle to the caller.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn set(
    file: &File,
    buffer: *const std::ffi::c_void,
    length: u32,
    class: FILE_INFORMATION_CLASS,
) -> io::Result<()> {
    let mut completed = IO_STATUS_BLOCK::default();
    // SAFETY: the callers provide an initialized structure of the specified class and
    // length, held until this operation on a synchronous file handle returns.
    status(unsafe {
        NtSetInformationFile(file.as_raw_handle(), &mut completed, buffer, length, class)
    })
}

pub(super) fn remove(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
    set(
        file,
        (&disposition as *const FILE_DISPOSITION_INFORMATION).cast(),
        size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
        FileDispositionInformation,
    )
}

pub(super) fn rename(file: &File, parent: &File, child: &OsStr, replace: bool) -> io::Result<()> {
    let name = name(child)?;
    let length = size_of::<FILE_RENAME_INFORMATION>() + name.len() * 2;
    let mut buffer = vec![0u64; length.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: the heap buffer is aligned and large enough for the header and UTF-16 tail;
    // the destination handle and buffer remain alive during the synchronous rename.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).RootDirectory = parent.as_raw_handle();
        (*info).FileNameLength =
            u32::try_from(name.len() * 2).map_err(|_| invalid("a stored name is too long"))?;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
    }
    set(
        file,
        buffer.as_ptr().cast(),
        u32::try_from(length).map_err(|_| invalid("a stored name is too long"))?,
        FileRenameInformation,
    )
}

fn number(bytes: &[u8], at: usize) -> io::Result<usize> {
    let bytes: [u8; 4] = bytes
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| invalid("an incomplete directory entry"))?;
    Ok(u32::from_le_bytes(bytes) as usize)
}

pub(super) fn for_each_name(directory: &File, mut visit: impl FnMut(OsString)) -> io::Result<()> {
    let mut buffer = vec![0u64; 8192];
    let mut first = true;
    loop {
        let mut completed = IO_STATUS_BLOCK::default();
        // SAFETY: the aligned heap buffer has 64 KiB; the directory is synchronous and
        // stays open through the call. The result length is checked before parsing.
        let result = unsafe {
            NtQueryDirectoryFile(
                directory.as_raw_handle(),
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut completed,
                buffer.as_mut_ptr().cast(),
                65536,
                FileNamesInformation,
                false,
                std::ptr::null(),
                first,
            )
        };
        first = false;
        if result == windows_sys::Win32::Foundation::STATUS_NO_MORE_FILES {
            break;
        }
        status(result)?;
        let used = completed.Information;
        if used == 0 || used > 65536 {
            return Err(invalid("an invalid directory result length"));
        }
        // SAFETY: NtQueryDirectoryFile initialized exactly `used` bytes within this buffer.
        let mut entries = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), used) };
        loop {
            let next = number(entries, offset_of!(FILE_NAMES_INFORMATION, NextEntryOffset))?;
            let length = number(entries, offset_of!(FILE_NAMES_INFORMATION, FileNameLength))?;
            let start = offset_of!(FILE_NAMES_INFORMATION, FileName);
            if length % 2 != 0 {
                return Err(invalid("an invalid stored name length"));
            }
            let text = entries
                .get(start..start + length)
                .ok_or_else(|| invalid("an incomplete stored name"))?;
            let text: Vec<_> = text
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            if text != [46] && text != [46, 46] {
                visit(OsString::from_wide(&text));
            }
            if next == 0 {
                break;
            }
            if next < start + length || next >= entries.len() {
                return Err(invalid("an invalid next directory entry"));
            }
            entries = &entries[next..];
        }
    }
    Ok(())
}

pub(super) fn names(directory: &File) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for_each_name(directory, |name| names.push(name))?;
    Ok(names)
}
