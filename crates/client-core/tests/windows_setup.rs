//! The Windows setup program on every system (ADR 0728): its file, the installation for one
//! account, a second installation over the first, and the removal. The operations of
//! Windows are replaced by a system that records them.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use junto_client_core::releases::setup::{self, Places, SetupSystem, UninstallEntry};
use junto_client_core::updates::Version;

#[derive(Default)]
struct Recorded {
    entry: RefCell<Option<UninstallEntry>>,
    shortcuts: RefCell<Vec<(PathBuf, PathBuf)>>,
    unregistered: RefCell<u32>,
}

impl SetupSystem for Recorded {
    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> std::io::Result<()> {
        *self.entry.borrow_mut() = Some(entry.clone());
        Ok(())
    }
    fn remove_uninstall_entry(&self) -> std::io::Result<()> {
        *self.entry.borrow_mut() = None;
        Ok(())
    }
    fn create_shortcut(&self, link: &Path, target: &Path) -> std::io::Result<()> {
        fs::create_dir_all(link.parent().unwrap())?;
        fs::write(link, b"shortcut")?;
        self.shortcuts
            .borrow_mut()
            .push((link.to_owned(), target.to_owned()));
        Ok(())
    }
    fn unregister_notifications(&self) -> std::io::Result<()> {
        *self.unregistered.borrow_mut() += 1;
        Ok(())
    }
}

/// A package directory of the Windows folder of `version`, with an uninstall program.
fn package(root: &Path, version: &str) -> PathBuf {
    let package = root.join(format!("package-{version}"));
    let folder = package.join("Junto");
    fs::create_dir_all(folder.join("licences")).unwrap();
    fs::write(folder.join("junto.exe"), format!("program {version}")).unwrap();
    fs::write(folder.join("junto-recover.exe"), "recover").unwrap();
    fs::write(folder.join("uninstall.exe"), "uninstall").unwrap();
    fs::write(folder.join("licences/Junto-LICENSE.txt"), "licence").unwrap();
    fs::write(
        folder.join("junto-package.json"),
        format!(
            r#"{{"format":1,"version":"{version}","system":"windows","package":"directory","architecture":"x86_64"}}"#
        ),
    )
    .unwrap();
    package
}

fn setup_program(root: &Path, version: &str) -> PathBuf {
    let installer = root.join("junto-installer.exe");
    if !installer.exists() {
        fs::write(&installer, b"MZ the installer program").unwrap();
    }
    let output = root.join(format!("Junto-{version}-windows-x86_64-setup.exe"));
    setup::write(&installer, &package(root, version), &output).unwrap();
    output
}

fn places(root: &Path) -> Places {
    Places::of_account(&root.join("Local"), &root.join("Roaming"))
}

#[test]
fn a_setup_program_holds_the_archive_of_the_folder_and_refuses_a_damaged_one() {
    let root = tempfile::tempdir().unwrap();
    let setup_file = setup_program(root.path(), "0.1.0");
    assert!(setup::holds_payload(&setup_file));
    assert!(!setup::holds_payload(&root.path().join("junto-installer.exe")));
    let bytes = fs::read(&setup_file).unwrap();
    assert!(bytes.starts_with(b"MZ the installer program"));
    assert!(!setup::payload(&setup_file).unwrap().is_empty());
    // A byte changed in the archive, and a download that ended early.
    let mut changed = bytes.clone();
    changed[40] ^= 1;
    fs::write(root.path().join("changed.exe"), &changed).unwrap();
    assert!(setup::payload(&root.path().join("changed.exe")).is_err());
    fs::write(root.path().join("short.exe"), &bytes[..bytes.len() - 10]).unwrap();
    assert!(setup::payload(&root.path().join("short.exe")).is_err());
    // A setup program is not written over a file, nor from a setup program.
    assert!(
        setup::write(
            &root.path().join("junto-installer.exe"),
            &package(root.path(), "0.1.1"),
            &setup_file,
        )
        .is_err()
    );
    assert!(
        setup::write(
            &setup_file,
            &package(root.path(), "0.1.2"),
            &root.path().join("again.exe"),
        )
        .is_err()
    );
}

#[test]
fn the_setup_installs_for_the_account_and_a_second_setup_replaces_the_folder() {
    let root = tempfile::tempdir().unwrap();
    let places = places(root.path());
    let system = Recorded::default();
    let first = setup::payload(&setup_program(root.path(), "0.1.0")).unwrap();
    let installed = setup::install(&first, &places, false, &system).unwrap();
    assert_eq!(installed.version, Version::new(0, 1, 0));
    assert_eq!(installed.replaced, None);
    let folder = root.path().join("Local/Programs/Junto");
    assert_eq!(places.folder(), folder);
    assert_eq!(fs::read_to_string(folder.join("junto.exe")).unwrap(), "program 0.1.0");
    assert!(folder.join("uninstall.exe").is_file());
    assert_eq!(
        system.shortcuts.borrow().as_slice(),
        [(
            root.path()
                .join("Roaming/Microsoft/Windows/Start Menu/Programs/Junto.lnk"),
            folder.join("junto.exe")
        )]
    );
    let entry = system.entry.borrow().clone().unwrap();
    let value = |name: &str| {
        entry
            .strings
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(value("DisplayName"), "Junto");
    assert_eq!(value("DisplayVersion"), "0.1.0");
    assert_eq!(
        value("UninstallString"),
        format!("\"{}\"", folder.join("uninstall.exe").display())
    );
    assert_eq!(
        value("QuietUninstallString"),
        format!("\"{}\" --quiet", folder.join("uninstall.exe").display())
    );
    assert!(entry.numbers.contains(&("NoModify", 1)));
    // The update directory of ADR 0726 holds no staging directory after the setup.
    let updates = root.path().join("Local/Programs/.Junto.junto-update");
    assert!(
        fs::read_dir(&updates)
            .unwrap()
            .flatten()
            .all(|entry| entry.file_name() == "lock"),
        "{:?}",
        fs::read_dir(&updates).unwrap().flatten().collect::<Vec<_>>()
    );

    // A newer setup replaces the entries of the folder, as an update does.
    let second = setup::payload(&setup_program(root.path(), "0.1.1")).unwrap();
    let installed = setup::install(&second, &places, false, &system).unwrap();
    assert_eq!(installed.replaced, Some(Version::new(0, 1, 0)));
    assert_eq!(fs::read_to_string(folder.join("junto.exe")).unwrap(), "program 0.1.1");
    assert_eq!(places.installed(), Some(Version::new(0, 1, 1)));
    // An older setup keeps the newer installation unless the person allows it.
    assert!(setup::install(&first, &places, false, &system).is_err());
    assert_eq!(places.installed(), Some(Version::new(0, 1, 1)));
    setup::install(&first, &places, true, &system).unwrap();
    assert_eq!(places.installed(), Some(Version::new(0, 1, 0)));
}

#[test]
fn the_removal_takes_away_every_part_and_keeps_the_data_unless_asked() {
    let root = tempfile::tempdir().unwrap();
    let places = places(root.path());
    let system = Recorded::default();
    let archive = setup::payload(&setup_program(root.path(), "0.1.0")).unwrap();
    setup::install(&archive, &places, false, &system).unwrap();
    fs::create_dir_all(&places.data).unwrap();
    fs::write(places.data.join("book.json"), "{}").unwrap();
    setup::uninstall(&places, false, &system).unwrap();
    assert!(!places.folder().exists());
    assert!(!places.updates().exists());
    assert!(!places.shortcut().exists());
    assert!(system.entry.borrow().is_none());
    assert_eq!(*system.unregistered.borrow(), 1);
    assert!(places.data.join("book.json").is_file());
    // A second removal finds nothing and succeeds; with the data it removes the data.
    setup::uninstall(&places, true, &system).unwrap();
    assert!(!places.data.exists());
}

#[test]
fn a_folder_that_a_person_extracted_elsewhere_has_its_update_directory_beside_it() {
    let root = tempfile::tempdir().unwrap();
    let places = Places::of_folder(
        &root.path().join("Apps/Junto copy"),
        &root.path().join("Local"),
        &root.path().join("Roaming"),
    );
    assert_eq!(
        places.updates(),
        root.path().join("Apps/.Junto copy.junto-update")
    );
    assert_eq!(places.data, root.path().join("Local/Junto"));
}
