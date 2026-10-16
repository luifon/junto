#[path = "native/admission.rs"]
mod admission;

use admission::Client;
use std::path::{Path, PathBuf};

#[test]
fn only_a_confirmed_idle_device_admits_a_stage() {
    assert!(admission::idle(Some(0)));
    for state in [None, Some(1), Some(2), Some(u32::MAX)] {
        assert!(!admission::idle(state), "unsafe admission: {state:?}");
    }
}

fn client(pid: Option<i32>, executable: Option<&str>, devices: Option<&[u32]>) -> Client {
    Client {
        pid,
        executable: executable.map(PathBuf::from),
        devices: devices.map(<[u32]>::to_vec),
    }
}

#[test]
fn only_this_process_and_its_helpers_may_have_used_the_device() {
    let own = (40, Path::new("/invented/runner"));
    let accepted = || {
        vec![
            client(Some(40), Some("/invented/runner"), Some(&[7])),
            client(Some(41), Some("/invented/runner"), Some(&[7, 3])),
            client(Some(42), Some("/invented/player"), Some(&[3])),
            client(Some(43), None, Some(&[])),
        ]
    };
    assert_eq!(admission::exclusive(&accepted(), 7, own), Ok(()));
    for refused in [
        client(Some(44), Some("/invented/player"), Some(&[3, 7])),
        client(Some(45), None, Some(&[7])),
        client(None, Some("/invented/player"), Some(&[7])),
        client(Some(46), Some("/invented/player"), None),
    ] {
        let mut clients = accepted();
        clients.push(refused);
        assert!(
            admission::exclusive(&clients, 7, own).is_err(),
            "unsafe stage end: {clients:?}"
        );
    }
}
