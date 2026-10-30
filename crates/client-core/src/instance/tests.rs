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
fn a_partial_frame_has_one_deadline_even_while_bytes_keep_arriving() {
    let (_root, directory) = directory();
    let Start::Primary(first, mut receive) =
        Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!()
    };
    let mut stream = TcpStream::connect(first.address).unwrap();
    stream
        .write_all(&(MAX_REQUEST as u32).to_be_bytes())
        .unwrap();
    let mut writing = stream.try_clone().unwrap();
    let writer = thread::spawn(move || {
        let end = Instant::now() + 2 * WAIT;
        while Instant::now() < end {
            if writing.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(WAIT / 4);
        }
    });
    stream.set_read_timeout(Some(WAIT + WAIT / 2)).unwrap();
    let started = Instant::now();
    let result = stream.read(&mut [0]);
    let elapsed = started.elapsed();
    drop(stream);
    writer.join().unwrap();
    assert!(match result {
        Ok(0) => true,
        Err(error) => error.kind() == io::ErrorKind::ConnectionReset,
        _ => false,
    });
    assert!(elapsed < WAIT + WAIT / 2);
    assert!(receive.try_recv().is_err());
    assert!(matches!(
        Instance::start(&directory, Activation::default()).unwrap(),
        Start::Forwarded
    ));
}

#[test]
fn accepted_requests_wait_for_a_body_after_nonblocking_accept() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    accepted.set_nonblocking(true).unwrap();
    let token = [7; 32];
    let bytes = serde_json::to_vec(&Request {
        token,
        activation: Activation::default(),
    })
    .unwrap();
    client
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    let (send, mut activation) = mpsc::channel(1);
    let (ready, started) = stopping::sync_channel(0);
    let (done, result) = stopping::channel();
    let worker = thread::spawn(move || {
        ready.send(()).unwrap();
        let _ = done.send(receive_request(&mut accepted, &token, &send));
    });
    started.recv_timeout(WAIT).unwrap();
    let early = result.recv_timeout(WAIT / 4);
    let waited = matches!(early, Err(stopping::RecvTimeoutError::Timeout));
    let written = client.write_all(&bytes);
    let outcome = match early {
        Ok(outcome) => Ok(outcome),
        Err(_) => result.recv_timeout(WAIT),
    };
    drop(client);
    worker.join().unwrap();
    assert!(
        waited,
        "the accepted socket did not wait for its request body"
    );
    written.unwrap();
    outcome.unwrap().unwrap();
    assert_eq!(activation.try_recv().unwrap(), Activation::default());
}

#[test]
fn shutdown_interrupts_an_accepted_partial_frame() {
    let (_root, directory) = directory();
    let Start::Primary(first, _) = Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!()
    };
    let mut stream = TcpStream::connect(first.address).unwrap();
    stream
        .write_all(&(MAX_REQUEST as u32).to_be_bytes())
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while first.accepted.lock().unwrap().socket.is_none() {
        assert!(
            Instant::now() < deadline,
            "the listener must accept the partial frame"
        );
        thread::yield_now();
    }
    let (done, finished) = stopping::channel();
    thread::spawn(move || {
        drop(first);
        done.send(()).unwrap();
    });
    let result = finished.recv_timeout(WAIT / 2);
    drop(stream);
    assert!(result.is_ok(), "shutdown must interrupt the socket read");
}

#[test]
fn a_second_start_claims_the_profile_when_its_primary_exits_during_forwarding() {
    let (_root, directory) = directory();
    let lock = directory
        .files()
        .try_lock_file("desktop-instance.lock")
        .unwrap()
        .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    directory
        .files()
        .write(
            ENDPOINT,
            &serde_json::to_vec(&Endpoint {
                port: listener.local_addr().unwrap().port(),
                token: [1; 32],
            })
            .unwrap(),
        )
        .unwrap();
    let other = directory.clone();
    let starting = thread::spawn(move || Instance::start(&other, Activation::default()));
    let stream = accept_before_deadline(&listener);
    drop(lock);
    drop(stream);
    drop(listener);
    assert!(matches!(
        starting.join().unwrap().unwrap(),
        Start::Primary(..)
    ));
}

#[test]
fn forwarding_reloads_the_endpoint_after_a_failed_handshake() {
    let (_root, directory) = directory();
    let Start::Primary(_first, mut receive) =
        Instance::start(&directory, Activation::default()).unwrap()
    else {
        panic!()
    };
    let mut kept = [0; 1024];
    let count = directory
        .files()
        .read_private(ENDPOINT, &mut kept)
        .unwrap()
        .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    directory
        .files()
        .write(
            ENDPOINT,
            &serde_json::to_vec(&Endpoint {
                port: listener.local_addr().unwrap().port(),
                token: [1; 32],
            })
            .unwrap(),
        )
        .unwrap();
    let other = directory.clone();
    let starting = thread::spawn(move || Instance::start(&other, Activation::default()));
    let mut stream = accept_before_deadline(&listener);
    directory.files().write(ENDPOINT, &kept[..count]).unwrap();
    stream.write_all(b"no").unwrap();
    drop(stream);
    assert!(matches!(
        starting.join().unwrap().unwrap(),
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

fn accept_before_deadline(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            other => panic!("no activation connection before deadline: {other:?}"),
        }
    }
}
