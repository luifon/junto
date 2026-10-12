//! The host commands for a backup and after a restore, and the health command (ADR 0018,
//! ADR 0012).

use junto_protocol::error::ErrorCode;
use junto_protocol::export::Export;
use junto_protocol::ids::StreamEpoch;
use reqwest::StatusCode;
use serde_json::Value;
use time::Duration;

use crate::messages_support::Files;
use crate::recordings_support::Studio;
use crate::support::{HostCommand, Scratch, TestServer};

/// Runs the server binary with `arguments` and only the variables of `environment`.
async fn binary(arguments: &[&str], environment: &[(&str, String)]) -> HostCommand {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_junto-server"));
    command
        .args(arguments)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (*name, value)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    tokio::task::spawn_blocking(move || {
        let output = command.output().unwrap();
        HostCommand {
            code: output.status.code(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    })
    .await
    .unwrap()
}

/// The identifier of the backup that `backup-begin` printed.
fn begun(output: &HostCommand) -> String {
    assert!(output.success, "{}", output.stderr);
    let mut lines = output.stdout.lines();
    let first = lines.next().unwrap();
    assert_eq!(lines.next(), None, "{}", output.stdout);
    let mut words = first.split(' ');
    assert_eq!(words.next(), Some("backup"), "{first}");
    let id = words.next().unwrap().to_owned();
    assert!(words.next().is_some(), "{first}");
    id
}

/// The lines of `backup-files` for the backup `id`, from a server binary that has the
/// settings `directories` of the server.
async fn listed(server: &TestServer, id: &str, directories: &[(&str, String)]) -> Vec<String> {
    let mut environment = vec![("JUNTO_DATABASE_URL", server.database_url())];
    environment.extend(directories.iter().cloned());
    let output = binary(&["backup-files", id], &environment).await;
    assert!(output.success, "{}", output.stderr);
    output.stdout.lines().map(str::to_owned).collect()
}

#[tokio::test]
async fn the_health_command_tells_whether_the_server_answers() {
    let server = TestServer::start().await;
    let listen = |address: String| vec![("JUNTO_LISTEN_ADDRESS", address)];

    let healthy = binary(&["health"], &listen(server.address.to_string())).await;
    assert_eq!(healthy.code, Some(0), "{}", healthy.stderr);
    assert_eq!(healthy.stdout.trim(), "healthy");
    // A server that listens on every address is asked on the loopback address.
    let every = format!("0.0.0.0:{}", server.address.port());
    assert_eq!(binary(&["health"], &listen(every)).await.code, Some(0));

    // Nothing listens on the port of a listener that was closed.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unused = closed.local_addr().unwrap();
    drop(closed);
    let down = binary(&["health"], &listen(unused.to_string())).await;
    assert_eq!(down.code, Some(1), "{}", down.stdout);
    assert!(
        down.stderr.contains("could not be asked"),
        "{}",
        down.stderr
    );
    let unreadable = binary(&["health"], &listen("not-an-address".to_owned())).await;
    assert_eq!(unreadable.code, Some(1));

    // A program that answers another status is not healthy.
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_address = other.local_addr().unwrap();
    let answering = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = other.accept().await.unwrap();
        let mut request = [0u8; 256];
        let _ = stream.read(&mut request).await;
        let answer = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(answer.as_bytes()).await.unwrap();
    });
    let unavailable = binary(&["health"], &listen(other_address.to_string())).await;
    assert_eq!(unavailable.code, Some(1));
    assert!(unavailable.stderr.contains("503"), "{}", unavailable.stderr);
    answering.await.unwrap();
}

#[tokio::test]
async fn a_backup_holds_the_deletion_of_attachments_back_until_it_ends() {
    let files = Files::open().await;
    let server = &files.server;
    let kept = files.upload(b"kept").await;
    files.attach(&[kept.id]).await;
    let deleted = files.upload(b"deleted").await;
    let message = files.attach(&[deleted.id]).await;
    let unfinished = files.start(b"not complete").await;
    let directory = |id: junto_protocol::ids::AttachmentId| files.directory.0.join(id.to_string());
    let settings = [(
        "JUNTO_ATTACHMENTS_DIR",
        files.directory.0.to_string_lossy().into_owned(),
    )];
    // A file whose message was deleted before the backup is gone from the disk.
    let gone = files.upload(b"gone").await;
    let early = files.attach(&[gone.id]).await;
    server
        .delete(&files.owner, &format!("/api/v1/messages/{}", early.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(Duration::minutes(2));
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert!(!directory(gone.id).exists());
    assert!(directory(deleted.id).exists());

    let id = begun(&server.host_command(&["backup-begin"]).await);
    // The database is copied after the start of the backup. An upload that ends before the
    // copy, and a message that is deleted after it, are both complete files in the copy.
    let late = files.upload(b"late").await;
    files.attach(&[late.id]).await;
    server
        .delete(&files.owner, &format!("/api/v1/messages/{}", message.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(Duration::minutes(2));
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert!(directory(deleted.id).exists());
    server
        .get(
            &files.owner,
            &format!("/api/v1/attachments/{}/file", deleted.id),
        )
        .await
        .error(ErrorCode::NotFound);

    let mut expected = vec![
        format!("file attachments/{}", kept.id),
        format!("file attachments/{}", deleted.id),
        format!("file attachments/{}", late.id),
        format!("skip attachments/{} upload_not_complete", unfinished.id),
    ];
    expected.sort();
    let mut lines = listed(server, &id, &settings).await;
    lines.sort();
    assert_eq!(lines, expected);

    // A command that does not see the directory cannot tell which removed file is still
    // on the disk: it prints no list.
    let blind = server.host_command(&["backup-files", &id]).await;
    assert_eq!(blind.code, Some(1));
    assert!(
        blind.stderr.contains("JUNTO_ATTACHMENTS_DIR"),
        "{}",
        blind.stderr
    );
    assert_eq!(blind.stdout, "");

    let ended = server.host_command(&["backup-end", &id]).await;
    assert!(ended.success, "{}", ended.stderr);
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert!(!directory(deleted.id).exists());
    assert!(directory(kept.id).exists());

    // A backup that has ended has no list and no second end.
    let late_list = server.host_command(&["backup-files", &id]).await;
    assert_eq!(late_list.code, Some(1));
    assert!(
        late_list.stderr.contains("no backup"),
        "{}",
        late_list.stderr
    );
    let again = server.host_command(&["backup-end", &id]).await;
    assert_eq!(again.code, Some(1));
    assert!(again.stderr.contains("no backup"), "{}", again.stderr);
    for command in ["backup-end", "backup-files"] {
        let unreadable = server.host_command(&[command, "not-an-identifier"]).await;
        assert_eq!(unreadable.code, Some(2));
    }
}

#[tokio::test]
async fn a_backup_lists_recordings_and_holds_their_deletion_back() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let finished = studio.finished().await.recording.id;
    let running = studio.record().await.id;
    let directory = server.recordings_dir().join(finished.to_string());
    let settings = [
        (
            "JUNTO_RECORDINGS_DIR",
            server.recordings_dir().to_string_lossy().into_owned(),
        ),
        ("JUNTO_RECORDER_DIR", "/out".to_owned()),
        (
            "JUNTO_RECORDER_LAYOUT_URL",
            "http://server.example.test/layout/".to_owned(),
        ),
        ("JUNTO_LIVEKIT_API_URL", "http://livekit:7880".to_owned()),
        ("JUNTO_LIVEKIT_URL", "wss://media.example.test".to_owned()),
        ("JUNTO_LIVEKIT_API_KEY", "key".to_owned()),
        ("JUNTO_LIVEKIT_API_SECRET", "media-server-secret".to_owned()),
    ];

    let id = begun(&server.host_command(&["backup-begin"]).await);
    let expected = [
        format!("file recordings/{finished}"),
        format!("skip recordings/{running} recording_runs"),
    ];
    assert_eq!(listed(server, &id, &settings).await, expected);

    // The recording is removed after the copy of the database: its directory stays on the
    // disk and in the list.
    server
        .delete(owner, &format!("/api/v1/recordings/{finished}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.process().await;
    assert!(directory.exists());
    assert_eq!(server.count("file_removals").await, 1);
    let mut after = listed(server, &id, &settings).await;
    after.sort();
    assert_eq!(after, expected);

    // A backup that never reports its end holds deletions back for 24 hours.
    server
        .clock
        .advance(Duration::hours(24) + Duration::minutes(1));
    server.process().await;
    assert!(!directory.exists());
    assert_eq!(server.count("file_removals").await, 0);
    let late = server.host_command(&["backup-end", &id]).await;
    assert!(late.success, "{}", late.stderr);
}

#[tokio::test]
async fn the_command_after_a_restore_renews_the_epoch_and_removes_exports() {
    let exports = Scratch::new();
    let directory = exports.0.clone();
    let server = TestServer::start_with(move |config| {
        config.exports_directory = Some(directory);
    })
    .await;
    let free = junto_server::storage::FakeDisk::new(crate::support::FREE_DISK);
    server
        .state
        .exports
        .space()
        .use_disk(std::sync::Arc::new(free));
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let export: Export = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/exports", channel.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::CREATED);
    junto_server::exports::run_once(&server.state)
        .await
        .unwrap();
    begun(&server.host_command(&["backup-begin"]).await);
    let epoch = || async {
        sqlx::query_scalar::<_, StreamEpoch>("select epoch from event_stream")
            .fetch_one(&server.db)
            .await
            .unwrap()
    };
    let before = epoch().await;

    let done = server.host_command(&["restored"]).await;
    assert!(done.success, "{}", done.stderr);
    assert!(done.stdout.contains("1 export(s)"), "{}", done.stdout);
    assert_ne!(epoch().await, before);
    assert_eq!(server.count("backups").await, 0);
    server
        .get(&owner, &format!("/api/v1/exports/{}", export.id))
        .await
        .error(ErrorCode::NotFound);
    junto_server::exports::run_once(&server.state)
        .await
        .unwrap();
    assert_eq!(server.count("exports").await, 0);
    assert!(!exports.0.join(export.id.to_string()).exists());

    // The command refuses a database on which no server was started.
    let empty =
        crate::support::host_command_on(crate::support::unused_database_url(), &["restored"]).await;
    assert!(!empty.success);
}
