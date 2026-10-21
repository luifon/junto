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
    assert_eq!(calls.load(Ordering::Relaxed), 2);
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
    assert_eq!(collector.requests.lock().await.len(), 2);
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
                        let mut header = Vec::new();
                        while !header.ends_with(b"\r\n\r\n") {
                            header.push(stream.read_u8().await.unwrap());
                            assert!(header.len() < 8192);
                        }
                        let header = String::from_utf8(header).unwrap();
                        let length: usize = header.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                        }).unwrap();
                        assert!(length < 1024 * 1024);
                        let mut body = vec![0; length];
                        stream.read_exact(&mut body).await.unwrap();
                        received.lock().await.push((header.lines().next().unwrap().to_owned(), serde_json::from_slice::<Value>(&body).unwrap()));
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
    assert_eq!(exports.len(), 2);
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
