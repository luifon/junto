#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

#[allow(dead_code)]
#[path = "../../client-core/tests/e2e/limit.rs"]
mod limit;

use livekit::webrtc::peer_connection_factory::{
    IceServer, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};

struct Server {
    child: Child,
    _adopted: limit::Adopted,
}

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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The fixture script, in the directory of this package in the checkout that runs the test:
/// `cargo test` sets that directory at run time, while a path compiled into the binary can
/// name another checkout whose build the shared target directory reused.
fn fixture_script() -> String {
    let package = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| {
        panic!("CARGO_MANIFEST_DIR is not set: run the test through cargo test")
    });
    format!("{package}/../../tools/test-turn-tls.py")
}

#[tokio::test]
#[ignore = "requires Python and OpenSSL in the isolated Linux container"]
async fn turn_tls_uses_system_roots_and_checks_the_peer() {
    // The test waits for processes and reads their pipes, which a timer of tokio does not
    // end: a watch ends the test process when the test has not ended in time.
    let _limit = limit::watch(limit::TEST_LIMIT);
    let Ok(fixtures) = std::env::var("JUNTO_TEST_TLS_FIXTURES") else {
        limit::step("runs the isolated TLS check");
        let mut check = Command::new("python3")
            .args([&fixture_script(), "run"])
            .arg(std::env::current_exe().unwrap_or_else(|error| panic!("{error}")))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|error| panic!("TLS fixtures: {error}"));
        let _adopted = limit::adopt(check.id());
        let status = check
            .wait()
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
        limit::step(format!(
            "starts the TLS server with the certificate {certificate}"
        ));
        let started = Command::new("python3")
            .args([&fixture_script(), "serve", &fixtures, certificate])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("TLS server: {error}"));
        let adopted = limit::adopt(started.id());
        let mut server = Server {
            child: started,
            _adopted: adopted,
        };
        let mut output = BufReader::new(
            server
                .child
                .stdout
                .take()
                .unwrap_or_else(|| panic!("stdout")),
        );
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
        limit::step(format!(
            "reads the result for the certificate {certificate}"
        ));
        let mut result = String::new();
        output
            .read_line(&mut result)
            .unwrap_or_else(|error| panic!("{error}"));
        connection.close();
        assert_eq!(result.trim(), expected, "certificate {certificate}");
        assert!(
            server
                .child
                .wait()
                .unwrap_or_else(|error| panic!("{error}"))
                .success()
        );
    }
}
