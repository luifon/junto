use super::*;

fn entry(root: &Path, program: &str) -> Autostart {
    use std::os::unix::fs::PermissionsExt;
    let program = root.join(program);
    fs::create_dir_all(program.parent().unwrap()).unwrap();
    fs::write(&program, b"program").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    Autostart {
        path: root.join("autostart/entry.desktop"),
        program,
        inherited: Vec::new(),
        desktops: vec!["Test".to_owned()],
    }
}

#[test]
fn choosing_login_preserves_other_keys_and_honours_an_external_disable() {
    let root = tempfile::tempdir().unwrap();
    let startup = entry(root.path(), "Junto/junto");
    assert_eq!(startup.status().unwrap(), Status::Off);
    assert_eq!(startup.set(true).unwrap(), Status::On);
    let text = fs::read_to_string(&startup.path).unwrap();
    assert!(text.contains(&format!("TryExec={}\n", startup.program.display())));
    fs::write(
        &startup.path,
        format!("{text}Comment=Kept\n[Other]\nValue=untouched\n")
            .replace("Hidden=false", "Hidden=true"),
    )
    .unwrap();
    assert_eq!(startup.status().unwrap(), Status::Off);
    assert_eq!(startup.set(true).unwrap(), Status::On);
    let text = fs::read_to_string(&startup.path).unwrap();
    assert_eq!(field(&text, "Comment"), Some("Kept"));
    assert!(text.ends_with("[Other]\nValue=untouched\n"));
    assert_eq!(startup.set(false).unwrap(), Status::Off);
    assert!(
        fs::read_dir(startup.path.parent().unwrap())
            .unwrap()
            .all(|file| file
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "desktop"))
    );
}

#[test]
fn an_old_copy_cannot_disable_the_entry_of_another_installation() {
    let root = tempfile::tempdir().unwrap();
    let old = entry(root.path(), "Old/junto");
    let new = entry(root.path(), "New/junto");
    new.set(true).unwrap();
    assert_eq!(old.status().unwrap(), Status::Off);
    old.set(false).unwrap();
    assert_eq!(new.status().unwrap(), Status::On);
}

#[test]
fn a_link_at_the_entry_is_refused_without_changing_its_target() {
    let root = tempfile::tempdir().unwrap();
    let startup = entry(root.path(), "junto");
    fs::create_dir_all(startup.path.parent().unwrap()).unwrap();
    let outside = root.path().join("outside");
    fs::write(&outside, "unchanged").unwrap();
    std::os::unix::fs::symlink(&outside, &startup.path).unwrap();
    assert!(startup.set(true).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "unchanged");
}

#[test]
fn the_desktop_launcher_opens_a_path_with_reserved_characters_literally() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let startup = entry(root.path(), "app ' \" $() ` %% \\ name ");
    fs::write(&startup.program, "#!/usr/bin/python3\nimport os, sys\noutput = os.environ['JUNTO_LOGIN_TEST_OUTPUT']\nwith open(output + '.tmp', 'w') as file:\n    file.write('\\n'.join(sys.argv) + '\\n')\nos.replace(output + '.tmp', output)\n").unwrap();
    fs::set_permissions(&startup.program, fs::Permissions::from_mode(0o700)).unwrap();
    startup.set(true).unwrap();
    let output = root.path().join("arguments");
    let mut child = Command::new("gio")
        .arg("launch")
        .arg(&startup.path)
        .env("JUNTO_LOGIN_TEST_OUTPUT", &output)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("the desktop launcher did not exit in five seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    while !output.exists() {
        assert!(
            Instant::now() < deadline,
            "the desktop launcher did not run the program"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let arguments = fs::read_to_string(output).unwrap();
    assert_eq!(
        arguments,
        format!("{}\n--no-activate\n", startup.program.display())
    );
}

#[test]
fn a_person_can_disable_an_inherited_entry_and_change_a_desktop_restriction() {
    let root = tempfile::tempdir().unwrap();
    let mut user = entry(root.path(), "junto");
    let mut system = entry(root.path(), "junto");
    system.path = root.path().join("system/autostart/entry.desktop");
    system.set(true).unwrap();
    user.inherited.push(system.path.clone());
    assert_eq!(user.status().unwrap(), Status::On);
    user.set(false).unwrap();
    assert_eq!(user.status().unwrap(), Status::Off);
    assert_eq!(system.status().unwrap(), Status::On);
    user.set(true).unwrap();
    let text = fs::read_to_string(&user.path).unwrap();
    fs::write(&user.path, format!("{text}OnlyShowIn=Other;\n")).unwrap();
    assert_eq!(user.status().unwrap(), Status::Off);
    user.set(true).unwrap();
    assert_eq!(user.status().unwrap(), Status::On);
    let text = fs::read_to_string(&user.path).unwrap();
    fs::write(&user.path, format!("{text}NotShowIn=Test;\n")).unwrap();
    assert_eq!(user.status().unwrap(), Status::Off);
    user.set(true).unwrap();
    let text = fs::read_to_string(&user.path).unwrap();
    fs::write(&user.path, format!("{text}TryExec=/not-installed/junto\n")).unwrap();
    assert_eq!(user.status().unwrap(), Status::Off);
    user.set(true).unwrap();
    assert_eq!(user.status().unwrap(), Status::On);
    fs::remove_file(&user.program).unwrap();
    assert_eq!(system.status().unwrap(), Status::Off);
}

#[test]
fn removing_this_installation_clears_only_its_own_enabled_registration() {
    let root = tempfile::tempdir().unwrap();
    let current = entry(root.path(), "Current/junto");
    let another = entry(root.path(), "Another/junto");
    current.set(true).unwrap();
    current.remove().unwrap();
    assert!(!current.path.exists());
    assert_eq!(current.status().unwrap(), Status::Off);
    current.remove().unwrap();
    another.set(true).unwrap();
    let before = fs::read(&another.path).unwrap();
    current.remove().unwrap();
    assert_eq!(fs::read(&another.path).unwrap(), before);
    assert_eq!(another.status().unwrap(), Status::On);
}

#[test]
fn removing_an_inherited_registration_disables_it_without_editing_the_system_entry() {
    let root = tempfile::tempdir().unwrap();
    let mut current = entry(root.path(), "junto");
    let mut system = entry(root.path(), "junto");
    system.path = root.path().join("system/entry.desktop");
    system.set(true).unwrap();
    let before = fs::read(&system.path).unwrap();
    current.inherited.push(system.path.clone());
    current.remove().unwrap();
    assert_eq!(current.status().unwrap(), Status::Off);
    assert_eq!(fs::read(&system.path).unwrap(), before);
    assert_eq!(system.status().unwrap(), Status::On);
}
