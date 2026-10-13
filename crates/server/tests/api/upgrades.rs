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

    let future = later_version(&["create table recording_guard_probe (id bigint)"]).await;
    let applied = migrations().await;
    let refused = bootstrap::migrate(&database, &future).await;
    assert!(matches!(refused, Err(StartError::RecordingRuns)));
    assert_eq!(migrations().await, applied);
    let message = StartError::RecordingRuns.to_string();
    assert!(message.contains("No migration runs while a recording runs"));

    let stopped = studio.stop(started.id).await;
    assert_eq!(stopped.recording.state, RecordingState::Processing);
    bootstrap::migrate(&database, &future).await.unwrap();
    assert_eq!(migrations().await, applied + 1);
    let applied: bool =
        sqlx::query_scalar("select to_regclass('recording_guard_probe') is not null")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(applied);
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
    use junto_server::bootstrap::{
        MigrationCopied, MigrationCopy, copy_before_migration, copy_name,
    };
    use sqlx::{ConnectOptions, Connection};

    let mut server = crate::support::TestServer::start().await;
    server.owner().await;
    let options = options_of(&server);
    let name = name_of(&server).await;
    let copies = || databases_from(format!("{name}_before_"));

    // A database of the current version is not copied.
    let current = copy_before_migration(&options, MigrationCopy::Database).await;
    assert_eq!(current.unwrap(), MigrationCopied::Nothing);

    let version = forget_the_newest_migration(&server).await;
    let off = copy_before_migration(&options, MigrationCopy::None).await;
    assert_eq!(off.unwrap(), MigrationCopied::Nothing);
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
    let made = copy_before_migration(&options, MigrationCopy::Database).await;
    assert_eq!(made.unwrap(), MigrationCopied::Made(expected.clone()));
    assert_eq!(copies().await, std::slice::from_ref(&expected));
    // The migration has not run: a second start keeps the copy and makes no other one.
    let kept = copy_before_migration(&options, MigrationCopy::Database).await;
    let unfinished = MigrationCopied::Unfinished(Some(expected.clone()));
    assert_eq!(kept.unwrap(), unfinished);
    assert_eq!(copies().await, std::slice::from_ref(&expected));
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
    for database in [&foreign, &long] {
        maintain(&[format!("create database {database} template {name}")]).await;
    }
    let foreign_options = options.clone().database(&foreign);
    let foreign_copy = copy_name(&foreign, version);
    let made = copy_before_migration(&foreign_options, MigrationCopy::Database).await;
    assert_eq!(made.unwrap(), MigrationCopied::Made(foreign_copy.clone()));
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

    delete_databases(&[expected, foreign, foreign_copy, long]).await;
}

fn options_of(server: &crate::support::TestServer) -> sqlx::postgres::PgConnectOptions {
    std::str::FromStr::from_str(&server.database_url()).unwrap()
}

async fn name_of(server: &crate::support::TestServer) -> String {
    sqlx::query_scalar("select current_database()::text")
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// Runs statements on the database server, outside the database of a test.
async fn maintain(statements: &[String]) {
    use sqlx::{ConnectOptions, Connection};

    let admin: sqlx::postgres::PgConnectOptions =
        std::str::FromStr::from_str(&crate::support::unused_database_url()).unwrap();
    let mut connection = admin.connect().await.unwrap();
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe(statement.clone()))
            .execute(&mut connection)
            .await
            .unwrap();
    }
    connection.close().await.unwrap();
}

/// The databases whose names start with `prefix`, each with its comment, in the order of
/// their names.
async fn commented_from(prefix: String) -> Vec<(String, Option<String>)> {
    use sqlx::{ConnectOptions, Connection};

    let admin: sqlx::postgres::PgConnectOptions =
        std::str::FromStr::from_str(&crate::support::unused_database_url()).unwrap();
    let mut connection = admin.connect().await.unwrap();
    let found = sqlx::query_as(
        "select datname::text, shobj_description(d.oid, 'pg_database') from pg_database d
         where starts_with(datname::text, $1) order by datname::text collate \"C\"",
    )
    .bind(prefix)
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    found
}

async fn databases_from(prefix: String) -> Vec<String> {
    let found = commented_from(prefix).await;
    found.into_iter().map(|(name, _)| name).collect()
}

async fn delete_databases(names: &[String]) {
    let statements: Vec<String> = names
        .iter()
        .map(|name| format!("drop database {name}"))
        .collect();
    maintain(&statements).await;
}

/// The migrations of a version of the server that follows this one: the migrations of this
/// version, and `further` ones after them, each with its SQL text.
async fn later_version(further: &[&str]) -> sqlx::migrate::Migrator {
    let directory = std::env::temp_dir().join(format!("junto-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let own = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(own).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
    }
    for (index, sql) in further.iter().enumerate() {
        let version = LATER + i64::try_from(index).unwrap();
        std::fs::write(directory.join(format!("{version}_later.sql")), sql).unwrap();
    }
    let migrator = sqlx::migrate::Migrator::new(directory.clone()).await;
    std::fs::remove_dir_all(&directory).unwrap();
    migrator.unwrap()
}

/// The first migration of [`later_version`].
const LATER: i64 = 99_990_001;

/// Applies `migrations` as a start of the server does, on a connection that is closed
/// afterwards.
async fn migrate_as_a_start(
    server: &crate::support::TestServer,
    migrations: &sqlx::migrate::Migrator,
) -> Result<(), StartError> {
    let database = server.database_with(|_| {}).await;
    let migrated = bootstrap::migrate(&database, migrations).await;
    database.close().await;
    migrated
}

async fn newest_migration_of(options: &sqlx::postgres::PgConnectOptions) -> i64 {
    use sqlx::{ConnectOptions, Connection};

    let mut connection = options.connect().await.unwrap();
    let newest = sqlx::query_scalar("select max(version) from _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    newest
}

/// The comment of a copy of the database `name`. The scripts of a deployment find a copy
/// by this text.
fn comment_of_a_copy(name: &str) -> Option<String> {
    Some(format!(
        "junto: copy of the database {name} before a migration"
    ))
}

/// The comment of the database `name`.
async fn comment_of(name: &str) -> Option<String> {
    let mut found = commented_from(name.to_owned()).await;
    found.retain(|(database, _)| database == name);
    assert_eq!(found.len(), 1, "{name}");
    found.remove(0).1
}

/// The line in the comment of a database while its upgrade has not applied every
/// migration, with the copy `copy` from before the upgrade.
fn unfinished_line(copy: &str) -> String {
    format!(
        "junto: an upgrade of this database has not finished. {copy} is the copy from before \
         the upgrade"
    )
}

/// ADR 0018: the copy of the state before an upgrade stays until a start has applied every
/// migration. A start after a migration that failed makes no copy of the database that is
/// migrated in part.
#[tokio::test]
async fn the_copy_before_an_upgrade_stays_until_a_start_has_applied_every_migration() {
    use junto_server::bootstrap::{MigrationCopied, MigrationCopy, copy_before, copy_name};

    const FIRST: &str = "create table later_first ()";
    let mut server = crate::support::TestServer::start().await;
    server.owner().await;
    let (options, name) = (options_of(&server), name_of(&server).await);
    let newest = newest_migration_of(&options).await;
    server.stop().await;
    server.state.database.close().await;
    let copies = || commented_from(format!("{name}_before_"));
    // The comment that the database had stays below the line of the server.
    maintain(&[format!("comment on database {name} is 'of the operator'")]).await;

    // The next version has two migrations. The first is applied and the second fails.
    let failing = later_version(&[FIRST, "select 1 / 0"]).await;
    let before = copy_name(&name, newest);
    let made = copy_before(&options, MigrationCopy::Database, &failing).await;
    assert_eq!(made.unwrap(), MigrationCopied::Made(before.clone()));
    let unfinished = Some(format!("{}\nof the operator", unfinished_line(&before)));
    assert_eq!(comment_of(&name).await, unfinished);
    let failed = migrate_as_a_start(&server, &failing).await;
    assert!(
        matches!(failed, Err(StartError::Migration(_))),
        "{failed:?}"
    );
    assert_eq!(newest_migration_of(&options).await, LATER);

    // The next start finds that the upgrade did not finish: it keeps the copy, which holds
    // the schema of the version before, and copies nothing.
    let kept = copy_before(&options, MigrationCopy::Database, &failing).await;
    let same = MigrationCopied::Unfinished(Some(before.clone()));
    assert_eq!(kept.unwrap(), same);
    assert_eq!(copies().await, [(before.clone(), comment_of_a_copy(&name))]);
    let of_copy = options.clone().database(&before);
    assert_eq!(newest_migration_of(&of_copy).await, newest);
    assert_eq!(comment_of(&name).await, unfinished);

    // A version with a corrected second migration ends the same upgrade.
    let corrected = later_version(&[FIRST, "create table later_second ()"]).await;
    let kept = copy_before(&options, MigrationCopy::Database, &corrected).await;
    assert_eq!(kept.unwrap(), same);
    migrate_as_a_start(&server, &corrected).await.unwrap();
    assert_eq!(comment_of(&name).await.as_deref(), Some("of the operator"));
    assert_eq!(copies().await, [(before.clone(), comment_of_a_copy(&name))]);
    let current = copy_before(&options, MigrationCopy::Database, &corrected).await;
    assert_eq!(current.unwrap(), MigrationCopied::Nothing);

    // The upgrade after a finished one replaces the copy.
    let third = later_version(&[
        FIRST,
        "create table later_second ()",
        "create table later_third ()",
    ])
    .await;
    let after = copy_name(&name, LATER + 1);
    let made = copy_before(&options, MigrationCopy::Database, &third).await;
    assert_eq!(made.unwrap(), MigrationCopied::Made(after.clone()));
    assert_eq!(copies().await, [(after.clone(), comment_of_a_copy(&name))]);

    delete_databases(&[after]).await;
}

/// While an upgrade has not finished, the server makes no copy, also when the copy from
/// before the upgrade is no longer there: the database can be migrated in part.
#[tokio::test]
async fn no_copy_is_made_of_a_database_whose_upgrade_did_not_finish() {
    use junto_server::bootstrap::{MigrationCopied, MigrationCopy, copy_before, copy_name};

    let mut server = crate::support::TestServer::start().await;
    let (options, name) = (options_of(&server), name_of(&server).await);
    let newest = newest_migration_of(&options).await;
    server.stop().await;
    server.state.database.close().await;

    let failing = later_version(&["create table later_first ()", "select 1 / 0"]).await;
    let before = copy_name(&name, newest);
    let made = copy_before(&options, MigrationCopy::Database, &failing).await;
    assert_eq!(made.unwrap(), MigrationCopied::Made(before.clone()));
    assert_eq!(comment_of(&name).await, Some(unfinished_line(&before)));
    assert!(migrate_as_a_start(&server, &failing).await.is_err());
    delete_databases(&[before]).await;

    let none = copy_before(&options, MigrationCopy::Database, &failing).await;
    assert_eq!(none.unwrap(), MigrationCopied::Unfinished(None));
    assert!(databases_from(format!("{name}_before_")).await.is_empty());
    assert!(databases_from(format!("{name}_making_")).await.is_empty());
}

/// ADR 0018: the copy is created under another name and gets the name of a copy when it
/// has its comment. A start removes what a stopped start left under that other name.
#[tokio::test]
async fn a_start_removes_the_copy_that_a_stopped_start_did_not_finish() {
    use junto_server::bootstrap::{MigrationCopied, MigrationCopy, copy_before, copy_name};

    let mut server = crate::support::TestServer::start().await;
    let (options, name) = (options_of(&server), name_of(&server).await);
    let newest = newest_migration_of(&options).await;
    server.stop().await;
    server.state.database.close().await;
    let comment = comment_of_a_copy(&name).unwrap();

    // A start that was stopped after it created the database, which the comment of the
    // database of the server names, and one that was stopped after it wrote the comment.
    let created = format!("{name}_making_{newest}");
    let commented = format!("{name}_making_{}", newest - 1);
    // Databases that nothing tells as left by a start of this server on this database: a
    // comment of another program, and no comment under a name that this start does not
    // create.
    let other_comment = format!("{name}_making_{}", newest - 2);
    let other_number = format!("{name}_making_{}", newest - 3);
    let other_name = format!("{name}_making_x");
    let all = [
        &created,
        &commented,
        &other_comment,
        &other_number,
        &other_name,
    ];
    let mut statements: Vec<String> = all
        .iter()
        .map(|database| format!("create database {database} template {name}"))
        .collect();
    statements.extend([
        format!("comment on database {commented} is '{comment}'"),
        format!("comment on database {other_comment} is 'of another program'"),
        format!("comment on database {name} is 'junto: the server makes the copy {created}'"),
    ]);
    maintain(&statements).await;

    let later = later_version(&["create table later_first ()"]).await;
    let copy = copy_name(&name, newest);
    let made = copy_before(&options, MigrationCopy::Database, &later).await;
    assert_eq!(made.unwrap(), MigrationCopied::Made(copy.clone()));
    assert_eq!(
        commented_from(format!("{name}_making_")).await,
        [
            (other_number.clone(), None),
            (other_comment.clone(), Some("of another program".to_owned())),
            (other_name.clone(), None),
        ]
    );
    assert_eq!(
        commented_from(format!("{name}_before_")).await,
        [(copy.clone(), Some(comment))]
    );

    delete_databases(&[copy, other_comment, other_number, other_name]).await;
}

/// A database that has the name of the copy and not the comment of a copy is not a copy
/// that this server made: it stays, and the server does not start.
#[tokio::test]
async fn a_database_with_the_name_of_the_copy_and_without_its_comment_stops_the_start() {
    use junto_server::bootstrap::{MigrationCopy, copy_before, copy_name};

    let mut server = crate::support::TestServer::start().await;
    let (options, name) = (options_of(&server), name_of(&server).await);
    let newest = newest_migration_of(&options).await;
    server.stop().await;
    server.state.database.close().await;
    let copy = copy_name(&name, newest);
    maintain(&[format!("create database {copy} template {name}")]).await;

    let later = later_version(&["create table later_first ()"]).await;
    let refused = copy_before(&options, MigrationCopy::Database, &later).await;
    let Err(StartError::MigrationCopy(reason)) = &refused else {
        panic!("{refused:?}");
    };
    assert!(reason.contains(&copy), "{reason}");
    assert!(
        reason.contains("does not have the comment of a copy"),
        "{reason}"
    );
    assert_eq!(
        commented_from(format!("{name}_before_")).await,
        [(copy.clone(), None)]
    );
    assert!(databases_from(format!("{name}_making_")).await.is_empty());
    assert_eq!(comment_of(&name).await, None);

    delete_databases(&[copy]).await;
}

/// A database under the name that the copy is created as, which the server did not record
/// as the copy that it makes, is the database of somebody else: it stays, and the server
/// does not start.
#[tokio::test]
async fn a_database_that_the_server_did_not_record_is_not_deleted_as_a_left_copy() {
    use junto_server::bootstrap::{MigrationCopy, copy_before};

    let mut server = crate::support::TestServer::start().await;
    let (options, name) = (options_of(&server), name_of(&server).await);
    let newest = newest_migration_of(&options).await;
    server.stop().await;
    server.state.database.close().await;
    let making = format!("{name}_making_{newest}");
    maintain(&[format!("create database {making} template {name}")]).await;

    let later = later_version(&["create table later_first ()"]).await;
    let refused = copy_before(&options, MigrationCopy::Database, &later).await;
    let Err(StartError::MigrationCopy(reason)) = &refused else {
        panic!("{refused:?}");
    };
    assert!(reason.contains(&making), "{reason}");
    assert!(reason.contains("did not record"), "{reason}");
    assert_eq!(
        databases_from(format!("{name}_making_")).await,
        std::slice::from_ref(&making)
    );
    assert!(databases_from(format!("{name}_before_")).await.is_empty());
    assert_eq!(comment_of(&name).await, None);

    delete_databases(&[making]).await;
}

#[tokio::test]
async fn no_copy_is_made_while_a_recording_runs() {
    use junto_server::bootstrap::{MigrationCopy, copy_before_migration};

    let studio = Studio::open().await;
    let server = &studio.scene.server;
    studio.record().await;
    forget_the_newest_migration(server).await;
    let options = options_of(server);
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
