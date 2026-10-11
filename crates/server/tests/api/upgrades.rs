//! ADR 0005: the server does not migrate its database while a recording runs.

use junto_protocol::recording::RecordingState;
use junto_server::bootstrap::{self, StartError};

use crate::recordings_support::Studio;

#[tokio::test]
async fn the_host_command_tells_whether_a_recording_runs() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let free = server.host_command(&["check-upgrade"]).await;
    assert!(free.success, "{}", free.stderr);
    assert!(free.stdout.contains("No recording runs"), "{}", free.stdout);

    let started = studio.record().await;
    let busy = server.host_command(&["check-upgrade"]).await;
    assert_eq!(busy.code, Some(3), "{}", busy.stderr);
    assert!(busy.stdout.contains("1 recording(s)"), "{}", busy.stdout);

    // A recording whose files are processed does not run.
    let stopped = studio.stop(started.id).await;
    assert_eq!(stopped.recording.state, RecordingState::Processing);
    assert!(server.host_command(&["check-upgrade"]).await.success);
}

#[tokio::test]
async fn the_server_does_not_migrate_while_a_recording_runs() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let database = server.database_with(|_| {}).await;
    let now = server.clock.now();
    let migrations = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from _sqlx_migrations")
            .fetch_one(&server.db)
            .await
            .unwrap()
    };

    // A server of the same version starts during a recording: no migration waits.
    let started = studio.record().await;
    assert!(bootstrap::start(&database, "Acme", now).await.is_ok());

    // The database has the schema of the version before: the newest migration waits.
    let applied = migrations().await;
    sqlx::query(
        "delete from _sqlx_migrations
         where version = (select max(version) from _sqlx_migrations)",
    )
    .execute(&server.db)
    .await
    .unwrap();
    let refused = bootstrap::start(&database, "Acme", now).await;
    assert!(matches!(refused, Err(StartError::RecordingRuns)));
    assert_eq!(migrations().await, applied - 1);
    let message = StartError::RecordingRuns.to_string();
    assert!(message.contains("No migration runs while a recording runs"));

    // When no recording runs any more, the start applies the migration. Here its tables
    // exist already, so the migration itself fails; the rule about recordings does not
    // refuse the start.
    let stopped = studio.stop(started.id).await;
    assert_eq!(stopped.recording.state, RecordingState::Processing);
    let migrated = bootstrap::start(&database, "Acme", now).await;
    assert!(matches!(migrated, Err(StartError::Migration(_))));
}

/// A server with a recorder checks at its start that the programs that read the files of
/// recordings run, so that a missing program is no failure of a recording later.
#[tokio::test]
async fn a_server_with_a_recorder_does_not_start_without_ffmpeg() {
    use junto_server::config::{MediaSecret, MediaSettings};

    let server = crate::support::TestServer::start().await;
    let mut config = crate::support::test_config();
    config.media = Some(MediaSettings {
        api_url: "http://127.0.0.1:9".to_owned(),
        client_url: "ws://127.0.0.1:9".to_owned(),
        api_key: "key".to_owned(),
        api_secret: MediaSecret::new("media-server-secret".to_owned()),
        request_timeout: time::Duration::seconds(1),
    });
    let mut recorder = crate::support::recorder_settings();
    recorder.ffprobe = "/nonexistent/ffprobe".into();
    config.recorder = Some(recorder);
    let database = server.database_with(|_| {}).await;
    let clock = server.clock.clone();
    let refused = junto_server::prepare(database, config.clone(), clock.clone()).await;
    let Err(StartError::MediaTools(reason)) = refused else {
        panic!("the server started without ffprobe");
    };
    assert!(reason.contains("/nonexistent/ffprobe"), "{reason}");

    config.recorder = Some(crate::support::recorder_settings());
    let database = server.database_with(|_| {}).await;
    assert!(junto_server::prepare(database, config, clock).await.is_ok());
}
