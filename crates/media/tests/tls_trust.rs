#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use livekit::webrtc::peer_connection_factory::{
    IceServer, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};

struct Server(Child);

struct TlsFailures(mpsc::Sender<()>);

impl log::Log for TlsFailures {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "libwebrtc"
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata())
            && record
                .args()
                .to_string()
                .contains("TLS post connection check failed")
        {
            let _ = self.0.send(());
        }
    }

    fn flush(&self) {}
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "requires Python and OpenSSL in the isolated Linux container"]
async fn turn_tls_uses_system_roots_and_checks_the_peer() {
    let Ok(fixtures) = std::env::var("JUNTO_TEST_TLS_FIXTURES") else {
        let status = Command::new("python3")
            .args([
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/test-turn-tls.py"),
                "run",
            ])
            .arg(std::env::current_exe().unwrap_or_else(|error| panic!("{error}")))
            .status()
            .unwrap_or_else(|error| panic!("TLS fixtures: {error}"));
        assert!(status.success(), "isolated TLS trust check failed");
        return;
    };
    let (rejected, failures) = mpsc::channel();
    log::set_boxed_logger(Box::new(TlsFailures(rejected)))
        .unwrap_or_else(|error| panic!("TLS diagnostics: {error}"));
    log::set_max_level(log::LevelFilter::Debug);
    let factory = PeerConnectionFactory::default();
    for (certificate, expected) in [
        ("trusted", "allocate"),
        ("unknown", "untrusted"),
        ("wrong-name", "closed"),
        ("client-only", "untrusted"),
    ] {
        let mut server = Server(
            Command::new("python3")
                .args([
                    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/test-turn-tls.py"),
                    "serve",
                    &fixtures,
                    certificate,
                ])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("TLS server: {error}")),
        );
        let mut output = BufReader::new(server.0.stdout.take().unwrap_or_else(|| panic!("stdout")));
        let mut port = String::new();
        output
            .read_line(&mut port)
            .unwrap_or_else(|error| panic!("{error}"));
        let port: u16 = port
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("server port: {error}"));
        let mut config = RtcConfiguration::default();
        config.ice_transport_type = IceTransportsType::Relay;
        config.ice_servers.push(IceServer {
            urls: vec![format!("turns:localhost:{port}?transport=tcp")],
            username: "test".into(),
            password: "test".into(),
        });
        let connection = factory
            .create_peer_connection(config)
            .unwrap_or_else(|error| panic!("{error}"));
        let _channel = connection
            .create_data_channel("test", Default::default())
            .unwrap_or_else(|error| panic!("{error}"));
        let offer = connection
            .create_offer(Default::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        connection
            .set_local_description(offer)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        if certificate == "wrong-name" {
            failures
                .recv_timeout(Duration::from_secs(15))
                .unwrap_or_else(|error| panic!("hostname was not rejected: {error}"));
            connection.close();
        }
        let mut result = String::new();
        output
            .read_line(&mut result)
            .unwrap_or_else(|error| panic!("{error}"));
        connection.close();
        assert_eq!(result.trim(), expected, "certificate {certificate}");
        assert!(
            server
                .0
                .wait()
                .unwrap_or_else(|error| panic!("{error}"))
                .success()
        );
    }
}
