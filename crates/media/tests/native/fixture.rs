use super::protocol::{Geometry, acknowledged};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::TempDir;

pub struct Fixture {
    pub geometry: Geometry,
    socket: BufReader<UnixStream>,
    child: Option<Child>,
    _directory: TempDir,
}
impl Fixture {
    pub async fn launch(name: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join(format!("Fixture{name}.app"));
        let contents = app.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        let executable =
            std::env::var("JUNTO_TEST_NATIVE_FIXTURE").expect("compiled native fixture path");
        std::fs::copy(executable, contents.join("MacOS/fixture")).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>fixture</string>
<key>CFBundleIdentifier</key><string>org.junto.native-fixture.{name}</string>
<key>CFBundleName</key><string>Junto fixture {name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/><key>NSHighResolutionCapable</key><true/>
</dict></plist>"#
        );
        std::fs::write(contents.join("Info.plist"), plist).unwrap();
        let path = directory.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let child = Command::new("open")
            .args(["-W", "-n"])
            .arg(&app)
            .arg("--args")
            .arg(&path)
            .arg(std::env::current_exe().unwrap())
            .arg(format!("Junto fixture {name}"))
            .arg(crate::ADMISSION.get().cloned().unwrap_or_default())
            .spawn()
            .unwrap();
        let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .expect("fixture connects within ten seconds")
            .unwrap();
        let stream = stream.into_std().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut socket = BufReader::new(stream);
        let mut line = String::new();
        socket.read_line(&mut line).unwrap();
        let geometry = Geometry::parse(&line).unwrap_or_else(|error| panic!("{error}: {line}"));
        println!("fixture {name}: {geometry:?}");
        Self {
            geometry,
            socket,
            child: Some(child),
            _directory: directory,
        }
    }
    fn command(&mut self, command: &str) -> String {
        writeln!(self.socket.get_mut(), "{command}").unwrap();
        let mut line = String::new();
        self.socket.read_line(&mut line).unwrap();
        line.trim().to_owned()
    }
    pub fn pattern(&mut self, marker: u32) {
        assert!(acknowledged(
            &self.command(&format!("pattern {marker}")),
            marker
        ));
    }
    pub fn tone(&mut self, hz: u32) {
        assert_eq!(self.command(&format!("tone {hz}")), format!("tone {hz}"));
    }
    pub fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = writeln!(self.socket.get_mut(), "close");
            let _ = self.socket.get_mut().shutdown(std::net::Shutdown::Write);
            let status = child.wait().unwrap();
            assert!(status.success(), "fixture exit {status}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.close();
    }
}
