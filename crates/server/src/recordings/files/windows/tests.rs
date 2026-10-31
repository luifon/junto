#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::os::windows::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("junto-files-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn place(parts: &[&str]) -> Place {
    Place(parts.iter().map(|part| (*part).to_owned()).collect())
}

#[test]
fn writes_finishes_lists_moves_and_removes_keep_their_file_contract() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    create_directory(root, &place(&["recording", "pieces"])).unwrap();
    let mut held = create_file(root, &place(&["recording", "first.partial"])).unwrap();
    held.write_all(b"first").unwrap();
    held.sync_all().unwrap();
    finish(
        root,
        &place(&["recording", "first.partial"]),
        &place(&["recording", "video.mp4"]),
    )
    .unwrap();
    drop(held);
    write_new(root, &place(&["recording", "second.partial"]), b"second").unwrap();
    finish(
        root,
        &place(&["recording", "second.partial"]),
        &place(&["recording", "video.mp4"]),
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join("recording/video.mp4")).unwrap(),
        b"first"
    );
    assert!(
        listed(root, &place(&["recording"]), |name| name
            .ends_with(".partial"))
        .unwrap()
        .is_empty()
    );
    assert!(write_new(root, &place(&["recording", "video.mp4"]), b"replace").is_err());
    for name in ["part_10.ts", "part_2.ts", "other.ts"] {
        write_new(root, &place(&["recording", "pieces", name]), b"piece").unwrap();
    }
    assert_eq!(
        pieces(root, &place(&["recording", "pieces"])).unwrap(),
        ["part_2.ts", "part_10.ts"]
    );
    rename(
        root,
        &place(&["recording", "video.mp4"]),
        &place(&["recording", "saved.mp4"]),
    )
    .unwrap();
    assert!(open_file(root, &place(&["recording", "saved.mp4"])).is_ok());
    assert!(open_file(root, &place(&["recording", "pieces"])).is_err());
    remove_if_empty(root, &place(&["recording"])).unwrap();
    assert!(root.join("recording").is_dir());
    let entries = entries_after(root, &place(&[]), None, 1, |_| true).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["recording"]
    );
    remove(root, &place(&["recording"])).unwrap();
    assert!(!root.join("recording").exists());
}

#[test]
fn a_directory_cannot_be_replaced_while_its_path_is_in_use() {
    let scratch = Scratch::new();
    fs::create_dir(scratch.0.join("held")).unwrap();
    let held = directory(&scratch.0, &["held".to_owned()]).unwrap();
    assert!(fs::rename(scratch.0.join("held"), scratch.0.join("moved")).is_err());
    drop(held);
    fs::rename(scratch.0.join("held"), scratch.0.join("moved")).unwrap();
}

#[test]
fn path_components_refuse_windows_devices_and_path_escapes() {
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "a\\outside",
        "C:outside",
        "file:stream",
        "name.",
        "name ",
        "NUL",
        "con.txt",
        "COM1",
        "lpt9.txt",
    ] {
        assert!(component(name).is_err(), "{name:?}");
    }
    for name in [
        "recording",
        "file.json",
        "part_0001.ts",
        "a.partial",
        "1-abc",
    ] {
        assert!(component(name).is_ok(), "{name:?}");
    }
}

#[tokio::test]
async fn a_junction_cannot_be_followed_for_reads_writes_or_removal() {
    use std::os::windows::process::CommandExt as _;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let scratch = Scratch::new();
    let root = scratch.0.join("root");
    let outside = scratch.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("kept"), b"outside").unwrap();
    let mut command = tokio::process::Command::new("cmd.exe");
    command.args(["/D", "/V:OFF", "/C"]);
    command
        .as_std_mut()
        .raw_arg("mklink /J \"%JUNTO_FILES_LINK%\" \"%JUNTO_FILES_TARGET%\"")
        .creation_flags(CREATE_NO_WINDOW);
    command
        .env("JUNTO_FILES_LINK", root.join("linked"))
        .env("JUNTO_FILES_TARGET", &outside)
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(status) => status.unwrap(),
        Err(_) => {
            let _ = child.start_kill();
            tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .unwrap()
                .unwrap();
            panic!("the junction fixture did not finish");
        }
    };
    assert!(status.success(), "the junction fixture failed: {status}");
    assert!(open_file(&root, &place(&["linked", "kept"])).is_err());
    assert!(create_directory(&root, &place(&["linked", "new-directory"])).is_err());
    assert!(create_file(&root, &place(&["linked", "new-file"])).is_err());
    assert!(rename(&root, &place(&["linked", "kept"]), &place(&["taken"])).is_err());
    remove(&root, &place(&["linked"])).unwrap();
    assert_eq!(fs::read(outside.join("kept")).unwrap(), b"outside");
    assert!(!outside.join("new-directory").exists());
    assert!(!outside.join("new-file").exists());
}

fn turn_into_junction(path: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, FILE_SHARE_WRITE, FILE_WRITE_DATA,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    use windows_sys::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;

    let target = fs::canonicalize(target)?;
    let print: Vec<u16> = target.as_os_str().encode_wide().collect();
    assert_eq!(&print[..4], &[92, 92, 63, 92]);
    let mut substitute = vec![92, 63, 63, 92];
    substitute.extend_from_slice(&print[4..]);
    let substitute_bytes = u16::try_from(substitute.len() * 2).unwrap();
    let print_bytes = u16::try_from(print.len() * 2).unwrap();
    let mut data = Vec::new();
    data.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    data.extend_from_slice(&(8 + substitute_bytes + 2 + print_bytes + 2).to_le_bytes());
    for value in [0, 0, substitute_bytes, substitute_bytes + 2, print_bytes] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in substitute.into_iter().chain([0]).chain(print).chain([0]) {
        data.extend_from_slice(&value.to_le_bytes());
    }
    let file = OpenOptions::new()
        .access_mode(FILE_WRITE_DATA)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut returned = 0;
    // SAFETY: the input encodes a mount-point REPARSE_DATA_BUFFER; the synchronous call
    // borrows its bytes, the directory handle, and the returned-byte count until it returns.
    let result = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            u32::try_from(data.len()).unwrap(),
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[test]
fn a_checked_directory_cannot_be_redirected_by_an_in_place_junction() {
    let scratch = Scratch::new();
    let root = scratch.0.join("root");
    let outside = scratch.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::create_dir(root.join("checked")).unwrap();
    fs::write(outside.join("kept"), b"outside").unwrap();
    let held = directory(&root, &["checked".to_owned()]).unwrap();
    turn_into_junction(&root.join("checked"), &outside).unwrap();
    assert!(reparse(&held.file.metadata().unwrap()));
    assert!(held.names().is_err());
    assert!(
        held.open(
            OsStr::new("kept"),
            FILE_GENERIC_READ,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
            SHARING
        )
        .is_err()
    );
    assert!(open_file(&root, &place(&["checked", "kept"])).is_err());
    assert!(write_new(&root, &place(&["checked", "injected"]), b"injected").is_err());
    assert!(create_directory(&root, &place(&["checked", "injected-dir"])).is_err());
    assert!(rename(&root, &place(&["checked", "kept"]), &place(&["taken"])).is_err());
    assert!(!outside.join("injected").exists());
    assert!(!outside.join("injected-dir").exists());
    drop(held);
    remove(&root, &place(&["checked"])).unwrap();
    assert_eq!(fs::read(outside.join("kept")).unwrap(), b"outside");
}

#[test]
fn relative_child_opens_cannot_follow_a_parent_changed_after_its_check() {
    let scratch = Scratch::new();
    let outside = scratch.0.join("outside");
    let root = scratch.0.join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::create_dir(root.join("checked")).unwrap();
    fs::write(outside.join("kept"), b"outside").unwrap();
    write_new(&root, &place(&["moving"]), b"inside").unwrap();
    let parent = directory(&root, &[]).unwrap();
    let moving = parent
        .open(OsStr::new("moving"), DELETE, FILE_OPEN, 0, SHARING)
        .unwrap();
    let held = directory(&root, &["checked".to_owned()]).unwrap();
    held.validate().unwrap();
    turn_into_junction(&root.join("checked"), &outside).unwrap();
    assert!(
        native::open(
            &held.file,
            OsStr::new("kept"),
            FILE_GENERIC_READ,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
            SHARING
        )
        .is_err()
    );
    if let Ok(mut inside) = native::open(
        &held.file,
        OsStr::new("injected"),
        FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
        SHARING,
    ) {
        inside.write_all(b"inside").unwrap();
        inside.sync_all().unwrap();
        assert!(!outside.join("injected").exists());
        native::remove(&inside).unwrap();
    }
    if let Ok(names) = native::names(&held.file) {
        assert!(!names.contains(&OsString::from("kept")));
    }
    let _ = native::rename(&moving, &held.file, OsStr::new("moved"), true);
    assert!(!outside.join("moved").exists());
    native::remove(&moving).unwrap();
    drop(moving);
    drop(held);
    remove(&root, &place(&["checked"])).unwrap();
    assert_eq!(fs::read(outside.join("kept")).unwrap(), b"outside");
    assert!(!outside.join("injected").exists());
}
