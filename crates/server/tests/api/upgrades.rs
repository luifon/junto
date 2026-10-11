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

/// The newest migration of the database of `server` is forgotten: the database then has
/// the schema of the version before.
async fn forget_the_newest_migration(server: &crate::support::TestServer) -> i64 {
    sqlx::query(
        "delete from _sqlx_migrations
         where version = (select max(version) from _sqlx_migrations)",
    )
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query_scalar("select max(version) from _sqlx_migrations")
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// ADR 0018: before a migration, the server copies its database on the same PostgreSQL
/// server, and it keeps one copy.
#[tokio::test]
async fn the_server_copies_the_database_before_a_migration() {
    use junto_server::bootstrap::{MigrationCopy, copy_before_migration, copy_name};
    use sqlx::postgres::PgConnectOptions;
    use sqlx::{ConnectOptions, Connection};
    use std::str::FromStr;

    let mut server = crate::support::TestServer::start().await;
    server.owner().await;
    let options = PgConnectOptions::from_str(&server.database_url()).unwrap();
    let name: String = sqlx::query_scalar("select current_database()::text")
        .fetch_one(&server.db)
        .await
        .unwrap();
    let admin = PgConnectOptions::from_str(&crate::support::unused_database_url()).unwrap();
    let copies = || async {
        let mut connection = admin.connect().await.unwrap();
        let mut found: Vec<String> = sqlx::query_scalar(
            "select datname::text from pg_database where starts_with(datname::text, $1)",
        )
        .bind(format!("{name}_before_"))
        .fetch_all(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        found.sort();
        found
    };

    // A database of the current version is not copied.
    let current = copy_before_migration(&options, MigrationCopy::Database).await;
    assert!(matches!(current, Ok(None)), "{current:?}");

    let version = forget_the_newest_migration(&server).await;
    let off = copy_before_migration(&options, MigrationCopy::None).await;
    assert!(matches!(off, Ok(None)), "{off:?}");
    // The server of the test is connected to the database: PostgreSQL makes no copy, and
    // the error says what the copy needs.
    let refused = copy_before_migration(&options, MigrationCopy::Database).await;
    let Err(StartError::MigrationCopy(reason)) = &refused else {
        panic!("{refused:?}");
    };
    assert!(reason.contains("accessed by other users"), "{reason}");
    let message = refused.unwrap_err().to_string();
    assert!(
        message.contains("no other program is connected"),
        "{message}"
    );
    assert!(message.contains("JUNTO_MIGRATION_COPY=none"), "{message}");
    assert!(copies().await.is_empty());

    server.stop().await;
    server.state.database.close().await;
    let expected = copy_name(&name, version);
    for _ in 0..2 {
        let made = copy_before_migration(&options, MigrationCopy::Database).await;
        assert_eq!(made.unwrap(), Some(expected.clone()));
        assert_eq!(copies().await, std::slice::from_ref(&expected));
    }
    let mut copy = options.clone().database(&expected).connect().await.unwrap();
    let (owners, migrations): (i64, i64) = sqlx::query_as(
        "select (select count(*) from server_state where owner_account_id is not null),
                (select max(version) from _sqlx_migrations)",
    )
    .fetch_one(&mut copy)
    .await
    .unwrap();
    assert_eq!((owners, migrations), (1, version));
    copy.close().await.unwrap();

    // The next copy deletes neither a database with another name that starts in the same
    // way, nor the copy that the server of that database made.
    let foreign = format!("{name}_before_x");
    let long = format!("{name}_before_a_long_name");
    let mut connection = admin.connect().await.unwrap();
    for database in [&foreign, &long] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "create database {database} template {name}"
        )))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();
    let foreign_options = options.clone().database(&foreign);
    let foreign_copy = copy_before_migration(&foreign_options, MigrationCopy::Database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(foreign_copy, copy_name(&foreign, version));
    copy_before_migration(&options, MigrationCopy::Database)
        .await
        .unwrap();
    assert_eq!(
        copies().await,
        [
            expected.clone(),
            long.clone(),
            foreign.clone(),
            foreign_copy.clone()
        ]
    );
    // PostgreSQL would cut the name of the copy of a database with a long name: no copy
    // is made, and the server does not start.
    let long_options = options.clone().database(&long);
    let refused = copy_before_migration(&long_options, MigrationCopy::Database).await;
    let Err(StartError::MigrationCopy(reason)) = &refused else {
        panic!("{refused:?}");
    };
    assert!(reason.contains("longer than"), "{reason}");
    assert_eq!(copies().await.len(), 4);

    let mut connection = admin.connect().await.unwrap();
    for database in [expected, foreign, foreign_copy, long] {
        sqlx::query(sqlx::AssertSqlSafe(format!("drop database {database}")))
            .execute(&mut connection)
            .await
            .unwrap();
    }
    connection.close().await.unwrap();
}

#[tokio::test]
async fn no_copy_is_made_while_a_recording_runs() {
    use junto_server::bootstrap::{MigrationCopy, copy_before_migration};
    use sqlx::postgres::PgConnectOptions;
    use std::str::FromStr;

    let studio = Studio::open().await;
    let server = &studio.scene.server;
    studio.record().await;
    forget_the_newest_migration(server).await;
    let options = PgConnectOptions::from_str(&server.database_url()).unwrap();
    let refused = copy_before_migration(&options, MigrationCopy::Database).await;
    assert!(
        matches!(refused, Err(StartError::RecordingRuns)),
        "{refused:?}"
    );
}

/// ADR 0005: a server does not start on a schema that is newer than the one it knows.
#[tokio::test]
async fn the_server_does_not_start_on_a_newer_schema() {
    let server = crate::support::TestServer::start().await;
    let database = server.database_with(|_| {}).await;
    sqlx::query(
        "insert into _sqlx_migrations (version, description, success, checksum, execution_time)
         values (99990101, 'of a later version', true, '\\x00', 0)",
    )
    .execute(&server.db)
    .await
    .unwrap();
    let refused = bootstrap::start(&database, "Acme", server.clock.now()).await;
    let Err(error @ StartError::NewerSchema(99_990_101)) = refused else {
        panic!("the server started on a newer schema");
    };
    assert!(error.to_string().contains("newer version of the server"));
}
