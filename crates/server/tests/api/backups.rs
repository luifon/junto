//! The host commands for a backup and after a restore, and the health command (ADR 0018,
//! ADR 0012).

use junto_protocol::error::ErrorCode;
use junto_protocol::export::Export;
use junto_protocol::ids::StreamEpoch;
use junto_protocol::recording::RecordingState;
use reqwest::StatusCode;
use serde_json::Value;
use std::path::{Path, PathBuf};
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

/// The settings with which a server binary sees the directory of the recordings of `server`.
fn recorder_settings_of(server: &TestServer) -> Vec<(&'static str, String)> {
    vec![
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
    ]
}

/// Runs `unnamed-files` with `arguments` after it, from a server binary that has the
/// settings `directories` of the server.
async fn unnamed(
    server: &TestServer,
    arguments: &[&str],
    directories: &[(&str, String)],
) -> HostCommand {
    let mut environment = vec![("JUNTO_DATABASE_URL", server.database_url())];
    environment.extend(directories.iter().cloned());
    let mut command = vec!["unnamed-files"];
    command.extend(arguments);
    binary(&command, &environment).await
}

fn sorted_lines(output: &HostCommand) -> Vec<String> {
    let mut lines: Vec<String> = output.stdout.lines().map(str::to_owned).collect();
    lines.sort();
    lines
}

/// The time after which `unnamed-files` takes a directory that nobody changed, and more.
const OLD: std::time::Duration = std::time::Duration::from_secs(25 * 60 * 60);

/// Creates the directory `name` in `root` with one file, last changed `age` ago.
fn directory_of_age(root: &Path, name: &str, age: std::time::Duration) -> PathBuf {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("0"), b"content").unwrap();
    changed_before(&directory, age);
    directory
}

/// States that `directory` was last changed `age` ago.
fn changed_before(directory: &Path, age: std::time::Duration) {
    let changed = std::time::SystemTime::now() - age;
    let opened = std::fs::File::open(directory).unwrap();
    opened.set_modified(changed).unwrap();
}

async fn newest_migration(server: &TestServer) -> i64 {
    sqlx::query_scalar("select max(version) from _sqlx_migrations")
        .fetch_one(&server.db)
        .await
        .unwrap()
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
    let settings = recorder_settings_of(server);

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
    // The command reads the time of the machine, by which the backup still runs.
    let late = server.host_command(&["backup-end", &id]).await;
    assert!(late.success, "{}", late.stderr);
}

/// A recording of the state `failed` has no file (ADR 0606): the list names no directory
/// for it. A recording whose state says that files exist is listed also when its directory
/// is absent, so that the program that copies reports the loss.
#[tokio::test]
async fn a_backup_lists_no_directory_for_a_recording_that_has_no_file() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    server.recorder().lose_files(true);
    let failed = studio.finished().await.recording;
    assert_eq!(failed.state, RecordingState::Failed);
    server.recorder().lose_files(false);
    let ready = studio.finished().await.recording;
    assert_eq!(ready.state, RecordingState::Ready);
    let settings = recorder_settings_of(server);

    let id = begun(&server.host_command(&["backup-begin"]).await);
    let expected = [format!("file recordings/{}", ready.id)];
    assert_eq!(listed(server, &id, &settings).await, expected);

    std::fs::remove_dir_all(server.recordings_dir().join(ready.id.to_string())).unwrap();
    assert_eq!(listed(server, &id, &settings).await, expected);
}

/// The end of a backup is a failure when the backup has expired: the server may have
/// deleted files before they were copied. The identifier of no backup is a failure too.
#[tokio::test]
async fn the_end_of_a_backup_that_has_expired_is_a_failure() {
    let server = TestServer::start().await;
    // The command reads the time of the machine: this backup began 25 hours before it.
    let begins = time::OffsetDateTime::now_utc() - Duration::hours(25);
    let expired = junto_server::backup::begin(&server.state.database, begins)
        .await
        .unwrap();

    for _ in 0..2 {
        let ended = server
            .host_command(&["backup-end", &expired.id.to_string()])
            .await;
        assert_eq!(ended.code, Some(1), "{}", ended.stdout);
        assert!(ended.stderr.contains("expired at"), "{}", ended.stderr);
        assert_eq!(ended.stdout, "");
    }

    let unknown = uuid::Uuid::now_v7().to_string();
    let ended = server.host_command(&["backup-end", &unknown]).await;
    assert_eq!(ended.code, Some(1), "{}", ended.stdout);
    assert!(ended.stderr.contains("no backup"), "{}", ended.stderr);

    // A backup that runs ends once.
    let id = begun(&server.host_command(&["backup-begin"]).await);
    let ended = server.host_command(&["backup-end", &id]).await;
    assert_eq!(ended.code, Some(0), "{}", ended.stderr);
    let again = server.host_command(&["backup-end", &id]).await;
    assert_eq!(again.code, Some(1), "{}", again.stdout);
}

/// `schema-version` prints the newest migration of the server binary and reads no setting,
/// so that a restore compares it with the schema of a backup before it replaces anything.
#[tokio::test]
async fn the_command_schema_version_prints_the_newest_migration_of_the_binary() {
    let server = TestServer::start().await;
    let printed = binary(&["schema-version"], &[]).await;
    assert_eq!(printed.code, Some(0), "{}", printed.stderr);
    assert_eq!(
        printed.stdout,
        format!("schema {}\n", newest_migration(&server).await)
    );
    assert_eq!(printed.stderr, "");
    let more = binary(&["schema-version", "now"], &[]).await;
    assert_eq!(more.code, Some(2));
}

/// On a database of another schema than its own, `restored` names both schemas and changes
/// nothing. Here the database has the schema before the table of the backups existed.
#[tokio::test]
async fn the_command_after_a_restore_changes_nothing_on_a_database_of_another_schema() {
    let server = TestServer::start().await;
    server.owner().await;
    let needed = newest_migration(&server).await;
    for statement in [
        "delete from _sqlx_migrations where version >= 10",
        "drop table backups",
    ] {
        sqlx::query(statement).execute(&server.db).await.unwrap();
    }
    let state = || async {
        sqlx::query_as::<_, (StreamEpoch, i64)>(
            "select (select epoch from event_stream), (select count(*) from audit_entries)",
        )
        .fetch_one(&server.db)
        .await
        .unwrap()
    };
    let before = state().await;

    let refused = server.host_command(&["restored"]).await;
    assert_eq!(refused.code, Some(1), "{}", refused.stdout);
    assert_eq!(refused.stdout, "");
    let expected = format!(
        "the database has the schema 9, and this version of the server needs the schema {needed}"
    );
    assert!(refused.stderr.contains(&expected), "{}", refused.stderr);
    assert!(
        refused.stderr.contains("Nothing was changed"),
        "{}",
        refused.stderr
    );
    assert_eq!(state().await, before);
}

/// `unnamed-files` lists the directories of attachments and of exports that no row names,
/// and removes them only with `--remove`. A directory that a row names stays, and so does
/// one that was changed in the last 24 hours, one whose name is not an identifier as the
/// server writes it, and a file.
#[tokio::test]
async fn the_command_removes_only_old_directories_that_no_row_names() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let directories = (attachments.0.clone(), exports.0.clone());
    let server = TestServer::start_with(move |config| {
        config.attachments_directory = Some(directories.0);
        config.exports_directory = Some(directories.1);
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let free = junto_server::storage::FakeDisk::new(crate::support::FREE_DISK);
    server
        .state
        .exports
        .space()
        .use_disk(std::sync::Arc::new(free));
    let owner = server.owner().await;
    let channel = crate::messages_support::channel(&server, &owner, "text").await;
    let place = serde_json::json!({ "channel_id": channel.id });
    let upload = crate::messages_support::upload(&server, &owner, place, b"kept").await;
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
    let settings = [
        (
            "JUNTO_ATTACHMENTS_DIR",
            attachments.0.to_string_lossy().into_owned(),
        ),
        (
            "JUNTO_EXPORTS_DIR",
            exports.0.to_string_lossy().into_owned(),
        ),
    ];

    // What a row names, also when nobody changed it for a long time.
    let of_upload = attachments.0.join(upload.id.to_string());
    let of_export = exports.0.join(export.id.to_string());
    for named in [&of_upload, &of_export] {
        assert!(named.is_dir(), "{named:?}");
        changed_before(named, OLD);
    }
    // What no row names.
    let identifier = || uuid::Uuid::now_v7().to_string();
    let (old, young, old_export) = (identifier(), identifier(), identifier());
    let old_attachment = directory_of_age(&attachments.0, &old, OLD);
    let young_attachment =
        directory_of_age(&attachments.0, &young, std::time::Duration::from_secs(60));
    let old_export_directory = directory_of_age(&exports.0, &old_export, OLD);
    // What the server did not create: other names, and a file.
    let other_form = uuid::Uuid::now_v7().simple().to_string();
    let others = [
        directory_of_age(&attachments.0, "not-an-identifier", OLD),
        directory_of_age(&attachments.0, &other_form, OLD),
        directory_of_age(&attachments.0, &identifier().to_uppercase(), OLD),
        attachments.0.join(identifier()),
    ];
    std::fs::write(&others[3], b"a file").unwrap();
    let stay = [&of_upload, &of_export, &young_attachment];

    let expected = |verb: &str| {
        let mut lines = vec![
            format!("skip attachments/{young} changed_recently"),
            format!("{verb} attachments/{old}"),
            format!("{verb} exports/{old_export}"),
        ];
        lines.sort();
        lines
    };
    let found = unnamed(&server, &[], &settings).await;
    assert_eq!(found.code, Some(0), "{}", found.stderr);
    assert_eq!(sorted_lines(&found), expected("unnamed"));
    assert!(old_attachment.is_dir() && old_export_directory.is_dir());

    // While a backup holds deletions back, the command removes nothing.
    let id = begun(&server.host_command(&["backup-begin"]).await);
    let held = unnamed(&server, &["--remove"], &settings).await;
    assert_eq!(held.code, Some(1), "{}", held.stdout);
    assert_eq!(held.stdout, "");
    assert!(held.stderr.contains("a backup holds"), "{}", held.stderr);
    assert!(old_attachment.is_dir() && old_export_directory.is_dir());
    assert_eq!(unnamed(&server, &[], &settings).await.code, Some(0));
    assert!(server.host_command(&["backup-end", &id]).await.success);

    // Two settings that name one directory: the command cannot tell whose a directory is.
    let one = [
        ("JUNTO_ATTACHMENTS_DIR", settings[0].1.clone()),
        ("JUNTO_EXPORTS_DIR", settings[0].1.clone()),
    ];
    let refused = unnamed(&server, &["--remove"], &one).await;
    assert_eq!(refused.code, Some(1), "{}", refused.stdout);
    assert!(
        refused.stderr.contains("the same directory"),
        "{}",
        refused.stderr
    );
    assert!(of_upload.is_dir() && old_attachment.is_dir());
    // No setting names a directory, and an argument that the command does not know.
    let blind = unnamed(&server, &["--remove"], &[]).await;
    assert_eq!(blind.code, Some(1), "{}", blind.stdout);
    assert!(
        blind.stderr.contains("JUNTO_ATTACHMENTS_DIR"),
        "{}",
        blind.stderr
    );
    assert_eq!(unnamed(&server, &["--all"], &settings).await.code, Some(2));

    let removed = unnamed(&server, &["--remove"], &settings).await;
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);
    assert_eq!(sorted_lines(&removed), expected("removed"));
    assert!(!old_attachment.exists() && !old_export_directory.exists());
    for kept in stay.into_iter().chain(&others) {
        assert!(kept.exists(), "{kept:?}");
    }
    server
        .get(&owner, &format!("/api/v1/exports/{}", export.id))
        .await
        .ok::<Export>(StatusCode::OK);

    // A database of another schema can name directories in another way: nothing is
    // removed.
    let unnamed_again = directory_of_age(&attachments.0, &identifier(), OLD);
    let needed = newest_migration(&server).await;
    sqlx::query("delete from _sqlx_migrations where version = $1")
        .bind(needed)
        .execute(&server.db)
        .await
        .unwrap();
    let other_schema = unnamed(&server, &["--remove"], &settings).await;
    assert_eq!(other_schema.code, Some(1), "{}", other_schema.stdout);
    assert_eq!(other_schema.stdout, "");
    let expected = format!("needs the schema {needed}");
    assert!(
        other_schema.stderr.contains(&expected),
        "{}",
        other_schema.stderr
    );
    assert!(unnamed_again.is_dir());
}

/// In the directory of the recordings, a directory stays that a recording names or that
/// waits in the queue of removals, and so does `rescued/`, which maintenance sweeps
/// (ADR 0606).
#[tokio::test]
async fn the_command_keeps_the_directories_of_recordings_and_of_queued_removals() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let finished = studio.finished().await.recording.id;
    let root = server.recordings_dir();
    let of_recording = root.join(finished.to_string());
    assert!(of_recording.is_dir());
    changed_before(&of_recording, OLD);
    let (queued, old) = (
        uuid::Uuid::now_v7().to_string(),
        uuid::Uuid::now_v7().to_string(),
    );
    let of_removal = directory_of_age(&root, &queued, OLD);
    sqlx::query("insert into file_removals (id, path, requested_at) values ($1, $2, now())")
        .bind(uuid::Uuid::now_v7())
        .bind(&queued)
        .execute(&server.db)
        .await
        .unwrap();
    let rescued = directory_of_age(&root.join("rescued"), "EG_job", OLD);
    changed_before(&root.join("rescued"), OLD);
    let unnamed_directory = directory_of_age(&root, &old, OLD);
    let settings = recorder_settings_of(server);

    let removed = unnamed(server, &["--remove"], &settings).await;
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);
    assert_eq!(removed.stdout, format!("removed recordings/{old}\n"));
    assert!(!unnamed_directory.exists());
    for kept in [&of_recording, &of_removal, &rescued] {
        assert!(kept.is_dir(), "{kept:?}");
    }
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
