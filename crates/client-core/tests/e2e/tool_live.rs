//! The commands of the tool that show the state of a server and act in calls.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;

use crate::support::{Device, TestServer, claim, logged_in};

const PATIENCE: Duration = Duration::from_secs(60);

fn tool(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_junto-cli"));
    command
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("JUNTO_DATA_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs one command to its end, with nothing on its standard input.
fn run(data_dir: &Path, arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = tool(data_dir)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// A running `watch` whose output lines arrive through a channel.
struct Watch {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Watch {
    fn start(data_dir: &Path) -> Self {
        let mut child = tool(data_dir)
            .args(["watch", "--seconds", "120"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    return;
                };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    fn write(&mut self, line: &str) {
        let input = self.child.stdin.as_mut().unwrap();
        writeln!(input, "{line}").unwrap();
        input.flush().unwrap();
    }

    /// The next line that fulfils `holds`. Lines before it are passed over.
    #[track_caller]
    fn line(&mut self, what: &str, holds: impl Fn(&str) -> bool) -> String {
        let started = Instant::now();
        loop {
            let left = PATIENCE.saturating_sub(started.elapsed());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if holds(&line) {
                        return line;
                    }
                }
                Err(_) => panic!(
                    "the watch did not print {what}; it printed:\n{}\n{}",
                    self.seen.join("\n"),
                    self.errors()
                ),
            }
        }
    }

    /// Waits for the end of the process and returns its exit status.
    #[track_caller]
    fn end(&mut self) -> Option<i32> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            assert!(
                started.elapsed() < PATIENCE,
                "the watch did not end; it printed:\n{}",
                self.seen.join("\n")
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Stops the process when it still runs, and returns what it wrote to standard error.
    fn errors(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut errors = String::new();
        if let Some(mut output) = self.child.stderr.take() {
            let _ = output.read_to_string(&mut errors);
        }
        errors
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// How many sessions the server holds.
async fn sessions(server: &TestServer) -> i64 {
    let options: PgConnectOptions = server.database_url().parse().unwrap();
    let mut database = options.connect().await.unwrap();
    sqlx::query_scalar("select count(*) from sessions")
        .fetch_one(&mut database)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_shows_the_state_watches_events_and_runs_call_actions() {
    let server = TestServer::start("Acme").await;
    let device = Device::new();
    let handle = device.add(&server.address()).await;
    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    let account_id = logged_in(&handle);
    let workspace = handle.api().workspaces().await.unwrap().remove(0);
    let create = |name: &str, kind| CreateChannel {
        name: name.to_owned(),
        kind,
        category_id: None,
        position: None,
    };
    let voice = handle
        .api()
        .create_channel(workspace.id, &create("Daily", ChannelKind::Voice))
        .await
        .unwrap();
    let voice_id = voice.id.to_string();
    // The tool is a second session of the same device: it uses the same data directory.
    let data_dir = device.directory.path().join("junto");
    assert_eq!(sessions(&server).await, 1);

    let (status, state, errors) = run(&data_dir, &["state"]);
    assert_eq!(status, Some(0), "{state}\n{errors}");
    assert_eq!(sessions(&server).await, 1, "`state` ended its session");
    let lines: Vec<&str> = state.lines().collect();
    let has = |line: String| lines.contains(&line.as_str());
    assert!(has(format!("account: {account_id}")), "{state}");
    assert!(has("name: Ada".to_owned()), "{state}");
    assert!(has("owner: true".to_owned()), "{state}");
    assert!(
        has(format!("workspace: {}  {}", workspace.id, workspace.name)),
        "{state}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("permissions: ") && line.contains("view_channel")),
        "{state}"
    );
    let position = lines
        .iter()
        .position(|line| *line == format!("channel: {voice_id}  voice  Daily"))
        .unwrap_or_else(|| panic!("no voice channel in:\n{state}"));
    assert_eq!(
        lines[position + 1..position + 4],
        [
            "  in use: false",
            "  user limit: none",
            "  hides who is inside: false"
        ]
    );
    assert!(has("own call: none".to_owned()), "{state}");
    assert!(lines.last().unwrap().starts_with("cursor: "), "{state}");
    assert!(!state.contains("status:"), "{state}");

    let (status, shown, errors) = run(&data_dir, &["call", "show", &voice_id]);
    assert_eq!(status, Some(0), "{shown}\n{errors}");
    assert_eq!(shown, "in use: false\n");
    let (status, own, errors) = run(&data_dir, &["call", "show"]);
    assert_eq!(
        (status, own.as_str()),
        (Some(0), "own call: none\n"),
        "{errors}"
    );
    // The test server has no media server.
    let (status, output, errors) = run(&data_dir, &["call", "join", &voice_id]);
    assert_eq!(status, Some(1), "{output}\n{errors}");
    assert!(errors.contains("media_unavailable"), "{errors}");
    // A wrong action is refused before any request.
    for arguments in [
        &["call"][..],
        &["call", "join"],
        &["call", "dance"],
        &["call", "hand", "sideways"],
        &[
            "call",
            "settings",
            voice_id.as_str(),
            "--limit",
            "4",
            "--no-limit",
        ],
    ] {
        let (status, _, errors) = run(&data_dir, arguments);
        assert_eq!(status, Some(2), "{arguments:?}\n{errors}");
        assert!(errors.contains("usage: junto-cli"), "{errors}");
    }

    assert_eq!(sessions(&server).await, 1, "every `call` ended its session");

    let mut watch = Watch::start(&data_dir);
    watch.line("the live status", |line| line == "status: live");
    assert_eq!(sessions(&server).await, 2);
    watch.line("the voice channel", |line| {
        line == format!("channel: {voice_id}  voice  Daily")
    });
    watch.line("the end of the state", |line| line.starts_with("cursor: "));

    // A change by another session of the account arrives as an event.
    let text = handle
        .api()
        .create_channel(workspace.id, &create("plans", ChannelKind::Text))
        .await
        .unwrap();
    let created = watch.line("the new channel", |line| line.contains("channel_created"));
    let mut words = created.split(' ');
    assert_eq!(words.next(), Some("event"));
    assert!(words.next().unwrap().parse::<u64>().is_ok(), "{created}");
    assert_eq!(words.next(), Some("channel_created"));
    assert_eq!(
        words.collect::<Vec<_>>(),
        [
            format!("workspace={}", workspace.id),
            format!("channel={}", text.id)
        ]
    );

    // An action on standard input runs with the session of the watch.
    watch.write(&format!("settings {voice_id} --limit 4"));
    watch.line("the changed limit", |line| line == "user limit: 4");
    watch.line("the event of the change", |line| {
        line.starts_with("event ")
            && line.contains(" channel_updated ")
            && line.ends_with(&format!("channel={voice_id}"))
    });
    watch.write(&format!("show {voice_id}"));
    watch.line("the call of the channel", |line| line == "in use: false");
    watch.write(&format!("join {voice_id}"));
    watch.line("the refusal", |line| {
        line.starts_with("error:") && line.contains("media_unavailable")
    });
    watch.write("dance");
    watch.line("the unknown action", |line| {
        line == "error: unknown call action dance"
    });
    watch.write("quit");
    assert_eq!(watch.end(), Some(0), "{}", watch.errors());

    // The watch ended its own session and no other.
    assert_eq!(sessions(&server).await, 1);
    handle.api().own_account().await.unwrap();
    let (status, state, errors) = run(&data_dir, &["state"]);
    assert_eq!(status, Some(0), "{errors}");
    assert!(state.contains("  user limit: 4"), "{state}");
    assert!(
        state.contains(&format!("channel: {}  text  plans", text.id)),
        "{state}"
    );
}
