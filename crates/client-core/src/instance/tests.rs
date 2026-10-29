use super::*;

fn directory() -> (tempfile::TempDir, DataDir) {
    let root = tempfile::tempdir().unwrap();
    let directory = DataDir::open(root.path().join("profile")).unwrap();
    (root, directory)
}

#[test]
fn later_starts_forward_activation_and_links_to_the_same_profile() {
    let (_root, directory) = directory();
    let Start::Primary(first, mut receive) =
        Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!("the first process owns the profile");
    };
    assert!(matches!(
        Instance::start(&directory, Activation::default()).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(receive.try_recv().unwrap(), Activation::default());
    let link = "junto://invite?server=https%3A%2F%2Fchat.example.test&code=ABC";
    assert!(matches!(
        Instance::start(
            &directory,
            Activation {
                link: Some(link.into()),
                activation_token: Some("desktop-token".into())
            }
        )
        .unwrap(),
        Start::Forwarded
    ));
    assert_eq!(
        receive.try_recv().unwrap(),
        Activation {
            link: Some(link.into()),
            activation_token: Some("desktop-token".into())
        }
    );
    drop(first);
    let Start::Primary(second, _) = Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!("the profile is released when the process exits");
    };
    drop(second);
}

#[test]
fn independent_profiles_run_at_the_same_time() {
    let (_a, a) = directory();
    let (_b, b) = directory();
    let Start::Primary(a, _) = Instance::start(&a, Activation::default()).unwrap() else {
        panic!()
    };
    let Start::Primary(b, _) = Instance::start(&b, Activation::default()).unwrap() else {
        panic!()
    };
    assert_ne!(a.address, b.address);
}

#[test]
fn simultaneous_starts_have_one_owner() {
    let (_root, directory) = directory();
    let mut release = Vec::new();
    let starts: Vec<_> = (0..2)
        .map(|_| {
            let directory = directory.clone();
            let (start, ready) = std::sync::mpsc::channel();
            release.push(start);
            thread::spawn(move || {
                ready.recv_timeout(Duration::from_secs(3)).unwrap();
                Instance::start(&directory, Activation::default()).unwrap()
            })
        })
        .collect();
    for start in release {
        start.send(()).unwrap();
    }
    let results: Vec<_> = starts
        .into_iter()
        .map(|start| start.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Start::Primary(..)))
            .count(),
        1
    );
}

#[test]
fn an_unauthenticated_request_does_not_activate_the_window() {
    let (_root, directory) = directory();
    let Start::Primary(first, mut receive) =
        Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!()
    };
    let mut stream = TcpStream::connect(first.address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let bytes = serde_json::to_vec(&Request {
        token: [0; 32],
        activation: Activation::default(),
    })
    .unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
    let mut answer = [0; 2];
    assert!(stream.read_exact(&mut answer).is_err());
    assert!(receive.try_recv().is_err());
    assert!(matches!(
        Instance::start(&directory, Activation::default()).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(receive.try_recv().unwrap(), Activation::default());
}

#[test]
fn oversized_links_are_refused_before_claiming_the_profile() {
    let (_root, directory) = directory();
    assert!(
        Instance::start(
            &directory,
            Activation {
                link: Some("x".repeat(MAX_REQUEST)),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(matches!(
        Instance::start(&directory, Activation::default()).unwrap(),
        Start::Primary(..)
    ));
}

#[test]
fn another_spelling_of_the_same_directory_keeps_the_same_owner() {
    let (_root, directory) = directory();
    let other = DataDir::open(directory.root().join(".")).unwrap();
    let Start::Primary(_first, mut receive) =
        Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        Instance::start(&other, Activation::default()).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(receive.try_recv().unwrap(), Activation::default());
}

#[cfg(unix)]
#[test]
fn a_link_or_a_public_lock_file_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let (root, directory) = directory();
    let outside = root.path().join("outside");
    std::fs::write(&outside, b"unchanged").unwrap();
    let lock = directory.root().join("desktop-instance.lock");
    std::os::unix::fs::symlink(&outside, &lock).unwrap();
    assert!(Instance::start(&directory, Activation::default()).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
    std::fs::remove_file(&lock).unwrap();
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Instance::start(&directory, Activation::default()),
        Err(Error::Storage(StorageError::NotPrivate { .. }))
    ));
}
