#![allow(clippy::unwrap_used)]
use ed25519_dalek::SigningKey;
use junto_client_core::releases::{self, Error, HttpSource, Manifest, TrustedKeys};
use junto_client_core::updates::{UpdateError, UpdateSource, Version};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn trust(keys: Vec<ed25519_dalek::VerifyingKey>) -> TrustedKeys {
    TrustedKeys::new(keys.clone()).with_freshness(TrustedKeys::new(keys))
}

async fn fixture(
    mode: &'static str,
) -> (
    tempfile::TempDir,
    HttpSource,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
) {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original");
    let content = if mode == "steady" {
        vec![b'a'; 128 * 1024]
    } else {
        b"archive".to_vec()
    };
    std::fs::write(&original, &content).unwrap();
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    let key = SigningKey::from_bytes(&seed);
    let mut metadata = Manifest {
        format: 1,
        version: "2.0.0".into(),
        system: "macos".into(),
        architecture: "aarch64".into(),
        archive: "archive".into(),
        size: 0,
        sha256: String::new(),
        expires: u64::MAX,
        archive_signatures: vec![],
    };
    releases::sign_archive(&original, &mut metadata, &key).unwrap();
    if mode == "bad-signature" {
        metadata.archive_signatures[0].signature =
            base64::Engine::encode(&base64::prelude::BASE64_STANDARD, [0; 64]);
    }
    if mode == "idle" {
        metadata.size = releases::MAX_ARCHIVE;
    }
    if mode == "expired" {
        metadata.expires = 1;
    }
    let wire = releases::sign_manifest(&metadata, &key, &key).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let source = HttpSource::new(
        &address,
        trust(if mode == "unknown-key" {
            vec![]
        } else {
            vec![key.verifying_key()]
        }),
        Version::new(if mode == "current" { 2 } else { 1 }, 0, 0),
        "macos",
        "aarch64",
    )
    .unwrap()
    .with_timeout(Duration::from_secs(if mode == "steady" { 2 } else { 1 }))
    .unwrap();
    let sent = Arc::new(AtomicUsize::new(0));
    let count = sent.clone();
    let destination = root.path().join("download");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request).await;
        if mode == "metadata-limit" {
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .await;
            for _ in 0..1024 {
                if stream.write_all(&[b' '; 8192]).await.is_err() {
                    break;
                }
                count.fetch_add(8192, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            return;
        }
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    wire.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&wire).await.unwrap();
        drop(stream);
        if ["expired", "unknown-key", "current"].contains(&mode) {
            return;
        }
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = stream.read(&mut request).await;
        assert!(String::from_utf8_lossy(&request).starts_with("GET /archive "));
        if mode == "redirect" {
            let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://{}/archive\r\nContent-Length: 0\r\n\r\n", target.local_addr().unwrap()).as_bytes()).await.unwrap();
            if let Ok(Ok((mut destination, _))) =
                tokio::time::timeout(Duration::from_secs(1), target.accept()).await
            {
                let _ = destination.read(&mut request).await;
                let _ = destination
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\narchive",
                    )
                    .await;
            }
            return;
        }
        let header = if mode == "truncated" {
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n"
        } else {
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"
        };
        stream.write_all(header.as_bytes()).await.unwrap();
        if mode == "oversize" {
            for _ in 0..1024 {
                if stream.write_all(&[0; 8192]).await.is_err() {
                    break;
                }
                count.fetch_add(8192, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            return;
        }
        if mode == "steady" || mode == "trickle" {
            for bytes in content.chunks(if mode == "steady" { 8192 } else { 1 }) {
                if stream.write_all(bytes).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(if mode == "trickle" {
                    400
                } else {
                    200
                }))
                .await;
            }
            return;
        }
        stream.write_all(b"arch").await.unwrap();
        if mode == "truncated" {
            return;
        }
        if mode == "idle" || mode == "cancel" {
            tokio::time::sleep(Duration::from_secs(10)).await;
            return;
        }
        if mode == "tamper" {
            loop {
                if std::fs::metadata(&destination).is_ok_and(|m| m.len() == 4) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            use std::io::Write;
            std::fs::OpenOptions::new()
                .write(true)
                .open(destination)
                .unwrap()
                .write_all(b"X")
                .unwrap();
        }
        stream.write_all(b"ive").await.unwrap();
    });
    (root, source, task, sent)
}
#[tokio::test]
async fn streaming_refusals_remove_partial_downloads_and_stop_oversize_bodies() {
    for mode in [
        "oversize",
        "truncated",
        "bad-signature",
        "redirect",
        "tamper",
    ] {
        let (root, source, task, sent) = fixture(mode).await;
        let path = root.path().join("download");
        let result = source.download(&path).await;
        match mode {
            "oversize" => assert!(matches!(result, Err(Error::Invalid("download size")))),
            "redirect" => assert!(matches!(result, Err(Error::Invalid("HTTP status")))),
            _ => assert!(result.is_err(), "{mode}"),
        }
        assert!(!path.exists(), "{mode}");
        task.await.unwrap();
        if mode == "oversize" {
            assert!(sent.load(Ordering::SeqCst) < 1024 * 1024);
        }
    }
}
#[tokio::test]
async fn metadata_stream_limit_and_source_error_states_are_explicit() {
    for (mode, expected) in [
        ("expired", UpdateError::Expired),
        ("unknown-key", UpdateError::UntrustedKeys),
        ("metadata-limit", UpdateError::Invalid),
    ] {
        let (_root, source, task, sent) = fixture(mode).await;
        assert_eq!(source.newest().await, Err(expected));
        task.await.unwrap();
        if mode == "metadata-limit" {
            assert!(sent.load(Ordering::SeqCst) < 1024 * 1024);
        }
    }
    let (_root, source, task, _) = fixture("current").await;
    assert_eq!(
        source.newest().await.unwrap().version,
        Version::new(2, 0, 0)
    );
    task.await.unwrap();
}
#[tokio::test]
async fn steady_download_can_exceed_idle_limit_but_stopped_download_times_out() {
    let (root, source, task, _) = fixture("steady").await;
    let path = root.path().join("download");
    let start = tokio::time::Instant::now();
    let archive = source.download(&path).await.unwrap();
    assert!(start.elapsed() > Duration::from_secs(2));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(archive);
    assert!(!path.exists());
    task.await.unwrap();
    let (root, source, task, _) = fixture("idle").await;
    let path = root.path().join("download");
    assert!(matches!(source.download(&path).await, Err(Error::Timeout)));
    assert!(!path.exists());
    task.abort();
    let _ = task.await;
}
#[tokio::test]
async fn cancellation_removes_the_partial_file() {
    let (root, source, server, _) = fixture("cancel").await;
    let path = root.path().join("download");
    let dest = path.clone();
    let download = tokio::spawn(async move { source.download(&dest).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    download.abort();
    let _ = download.await;
    assert!(!path.exists());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn total_deadline_limits_a_continuous_trickle() {
    let (root, source, task, _) = fixture("trickle").await;
    let path = root.path().join("download");
    assert!(matches!(source.download(&path).await, Err(Error::Timeout)));
    assert!(!path.exists());
    task.abort();
    let _ = task.await;
}
