use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn telemetry_refuses_redirects() {
    let destination = Collector::start(false).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().fallback({
        let calls = calls.clone();
        let destination = destination.endpoint.clone();
        move || {
            calls.fetch_add(1, Ordering::Relaxed);
            let destination = destination.clone();
            async move {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", format!("{destination}/v1/traces"))],
                )
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let telemetry = Telemetry::start(Some(endpoint)).await.unwrap().unwrap();
    telemetry.instruments.tracer.start("request").end();
    telemetry.instruments.requests.add(1, &[]);
    telemetry.shutdown().await;
    assert!(calls.load(Ordering::Relaxed) >= 2);
    assert!(destination.requests.lock().await.is_empty());
    task.abort();
}

#[tokio::test]
async fn telemetry_bypasses_environment_proxies() {
    const CHILD: &str = "JUNTO_TEST_OTEL_PROXY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let proxy = Collector::start(false).await;
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "telemetry::tests::transport::telemetry_bypasses_environment_proxies",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .kill_on_drop(true);
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            command.env(key, &proxy.endpoint);
        }
        run_child(command).await;
        assert!(proxy.requests.lock().await.is_empty());
        return;
    }
    let collector = Collector::start(false).await;
    let telemetry = runtime(&collector).await;
    telemetry.instruments.tracer.start("request").end();
    telemetry.instruments.requests.add(1, &[]);
    telemetry.shutdown().await;
    assert_both_signals(&collector.requests.lock().await);
}

#[tokio::test]
async fn telemetry_https_requires_trusted_ca() {
    use base64::Engine;
    use tokio_rustls::rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    };
    let authority = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../client-core/tests/data/stream-tls/authority.der"
    ));
    let certificate = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../client-core/tests/data/stream-tls/server.der"
    ));
    let key = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../client-core/tests/data/stream-tls/server-key.der"
    ));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(certificate.to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_vec())),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("https://{}", listener.local_addr().unwrap())).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let received = requests.clone();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (socket, _) = accepted.unwrap();
                    let acceptor = acceptor.clone();
                    let received = received.clone();
                    connections.spawn(async move {
                        let Ok(mut stream) = acceptor.accept(socket).await else { return };
                        let (line, body) = read_request(&mut stream).await.unwrap();
                        received.lock().await.push((line, serde_json::from_slice::<Value>(&body).unwrap()));
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
                        stream.shutdown().await.unwrap();
                    });
                }
                Some(result) = connections.join_next(), if !connections.is_empty() => { result.unwrap(); }
            }
        }
    });
    let telemetry = Telemetry::start(Some(endpoint.clone()))
        .await
        .unwrap()
        .unwrap();
    telemetry.instruments.tracer.start("request").end();
    telemetry.instruments.requests.add(1, &[]);
    telemetry.shutdown().await;
    assert!(
        requests.lock().await.is_empty(),
        "the test CA is not in the system trust store"
    );
    let file =
        TestCa(std::env::temp_dir().join(format!("junto-otel-ca-{}.pem", uuid::Uuid::new_v4())));
    let pem = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        base64::engine::general_purpose::STANDARD.encode(authority)
    );
    std::fs::write(&file.0, pem).unwrap();
    let telemetry = Telemetry::start(Some(endpoint.with_ca_file(file.0.clone())))
        .await
        .unwrap()
        .unwrap();
    drop(file);
    telemetry.instruments.tracer.start("request").end();
    telemetry.instruments.requests.add(1, &[]);
    telemetry.shutdown().await;
    let exports = requests.lock().await;
    assert!(
        exports
            .iter()
            .any(|(path, body)| path.contains("/v1/traces") && body["resourceSpans"].is_array())
    );
    assert!(
        exports
            .iter()
            .any(|(path, body)| path.contains("/v1/metrics") && body["resourceMetrics"].is_array())
    );
    task.abort();
}

struct TestCa(std::path::PathBuf);
impl Drop for TestCa {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
async fn telemetry_rejects_invalid_ca_bundles_without_the_path() {
    for contents in [
        "not a certificate",
        "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n",
    ] {
        let file = TestCa(
            std::env::temp_dir().join(format!("junto-otel-secret-ca-{}.pem", uuid::Uuid::new_v4())),
        );
        std::fs::write(&file.0, contents).unwrap();
        let endpoint = Endpoint::parse("https://collector.example.test")
            .unwrap()
            .with_ca_file(file.0.clone());
        match Telemetry::start(Some(endpoint)).await {
            Err(error) => {
                assert!(error.to_string().contains("JUNTO_OTEL_CA_FILE"), "{error}");
                assert!(!error.to_string().contains("secret-ca"));
            }
            Ok(telemetry) => {
                if let Some(telemetry) = telemetry {
                    telemetry.shutdown().await;
                }
                panic!("invalid CA bundle must be refused");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telemetry_bounds_a_trickling_response() {
    if bounded_child("transport::telemetry_bounds_a_trickling_response").await {
        return;
    }
    let router = Router::new().fallback(|| async {
        let stream = futures_util::stream::unfold(
            (0, tokio::time::interval(Duration::from_millis(200))),
            |(index, mut interval)| async move {
                if index == 22 {
                    return None;
                }
                interval.tick().await;
                let byte = match index {
                    20 => b'{',
                    21 => b'}',
                    _ => b' ',
                };
                Some((
                    Ok::<_, std::io::Error>(Bytes::from(vec![byte])),
                    (index + 1, interval),
                ))
            },
        );
        axum::body::Body::from_stream(stream)
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let telemetry = Telemetry::start(Some(endpoint)).await.unwrap().unwrap();
    telemetry.instruments.requests.add(1, &[]);
    let metrics = telemetry.metrics.clone();
    let result = tokio::task::spawn_blocking(move || metrics.force_flush())
        .await
        .unwrap();
    task.abort();
    telemetry.shutdown().await;
    assert!(
        result.is_err(),
        "a response that trickles past the export deadline must fail"
    );
}

async fn read_request(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Option<(String, Vec<u8>)> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        header.push(stream.read_u8().await.ok()?);
        assert!(header.len() < 8192);
    }
    let header = String::from_utf8(header).unwrap();
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length < 1024 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    Some((header.lines().next().unwrap().to_owned(), body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telemetry_reconnects_after_the_collector_closes_an_idle_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn({
        let requests = requests.clone();
        async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let requests = requests.clone();
                tokio::spawn(async move {
                    while let Ok(Some(_)) =
                        tokio::time::timeout(Duration::from_millis(200), read_request(&mut socket))
                            .await
                    {
                        requests.fetch_add(1, Ordering::Relaxed);
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}")
                            .await
                            .unwrap();
                    }
                });
            }
        }
    });
    let telemetry = Telemetry::start(Some(endpoint)).await.unwrap().unwrap();
    for export in 0..2 {
        if export > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        telemetry.instruments.requests.add(1, &[]);
        let metrics = telemetry.metrics.clone();
        let result = tokio::task::spawn_blocking(move || metrics.force_flush())
            .await
            .unwrap();
        assert!(result.is_ok(), "metric export {export} failed");
    }
    telemetry.shutdown().await;
    task.abort();
    assert!(requests.load(Ordering::Relaxed) >= 2);
}

#[tokio::test]
async fn telemetry_caps_the_collector_response() {
    for (size, accepted) in [
        (export::RESPONSE_LIMIT, true),
        (export::RESPONSE_LIMIT + 1, false),
    ] {
        let reply = json!({ "padding": " ".repeat(size - 14) });
        assert_eq!(serde_json::to_vec(&reply).unwrap().len(), size);
        let collector = Collector::with_response(false, StatusCode::OK, reply).await;
        let telemetry = runtime(&collector).await;
        telemetry.instruments.requests.add(1, &[]);
        let metrics = telemetry.metrics.clone();
        let result = tokio::task::spawn_blocking(move || metrics.force_flush())
            .await
            .unwrap();
        telemetry.shutdown().await;
        assert_eq!(result.is_ok(), accepted, "response of {size} bytes");
    }
}
