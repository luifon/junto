use junto_authz::WorkspacePermissions;
use junto_protocol::ids::{ChannelId, ServerId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::secret::OwnerCode;
use junto_protocol::structure::ChannelKind;
use sqlx::PgConnection;
use sqlx::migrate::Migrator;
use time::OffsetDateTime;

use crate::database::{self, Database, TransactionError};
use crate::db::{self, StoredText};
use crate::owner::{self, HostCommandError};
use crate::secrets::RandomnessUnavailable;
use crate::transact;

pub static MIGRATIONS: Migrator = sqlx::migrate!("./migrations");

const START_LOCK: i64 = 0x4a55_4e54_4f00_0001;
pub const EVERYONE_ROLE_NAME: &str = "everyone";
pub const DEFAULT_TEXT_CHANNEL_NAME: &str = "general";
pub const DEFAULT_VOICE_CHANNEL_NAME: &str = "voice";

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("database migration failed: {0}")]
    Migration(sqlx::migrate::MigrateError),
    #[error(
        "the database has the schema of a newer version of the server (its migration {0} is \
         unknown to this version). An older server does not start on it"
    )]
    NewerSchema(i64),
    #[error(
        "the copy of the database before the migration could not be made: {0}. The copy needs \
         that no other program is connected to the database, and that the database user may \
         create a database. Nothing was migrated. Set JUNTO_MIGRATION_COPY=none to migrate \
         without a copy"
    )]
    MigrationCopy(String),
    #[error("the client of the media server could not be built: {0}")]
    MediaServer(String),
    #[error(
        "the programs that read the files of recordings do not run: {0}. Install ffmpeg and \
         ffprobe, or set JUNTO_FFMPEG and JUNTO_FFPROBE"
    )]
    MediaTools(String),
    #[error(
        "a recording is starting or runs, and this version of the server changes the database. \
         No migration runs while a recording runs. Start the earlier version of the server, \
         end the recording, and start this version again"
    )]
    RecordingRuns,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Randomness(#[from] RandomnessUnavailable),
    #[error("the database was too busy to finish the start")]
    Busy,
    #[error(transparent)]
    OwnerCode(#[from] HostCommandError),
}

impl From<sqlx::migrate::MigrateError> for StartError {
    fn from(error: sqlx::migrate::MigrateError) -> Self {
        match error {
            sqlx::migrate::MigrateError::VersionMissing(version) => Self::NewerSchema(version),
            other => Self::Migration(other),
        }
    }
}

impl TransactionError for StartError {
    fn is_conflict(&self) -> bool {
        match self {
            Self::Database(error) => database::is_conflict(error),
            Self::OwnerCode(error) => error.is_conflict(),
            _ => false,
        }
    }

    fn exhausted() -> Self {
        Self::Busy
    }
}

pub struct Started {
    pub server_id: ServerId,
    /// Present while the server has no owner. Each start replaces the code of the start before.
    pub owner_code: Option<OwnerCode>,
}

/// Applies the migrations, creates the default workspace on the first start, and issues an
/// owner code while the server has no owner.
pub async fn start(
    database: &Database,
    server_name: &str,
    now: OffsetDateTime,
) -> Result<Started, StartError> {
    migrate(database, &MIGRATIONS).await?;
    transact!(database, |connection| {
        start_in(connection, server_name, now).await
    })
}

/// Applies the migrations of `migrations` that the database does not have. No migration
/// runs while a recording runs (ADR 0005).
pub async fn migrate(database: &Database, migrations: &Migrator) -> Result<(), StartError> {
    if migration_waits(database, migrations).await? && recordings_that_run(database).await? > 0 {
        return Err(StartError::RecordingRuns);
    }
    database.migrate(migrations).await?;
    upgrade_finished(database).await?;
    Ok(())
}

/// The name and the comment of the database of a connection.
const OWN_COMMENT: &str = "select datname::text, shobj_description(oid, 'pg_database')
     from pg_database where datname = current_database()";

/// Removes the line of an unfinished upgrade from the comment of this database. It runs
/// after the migrations ran to their end, also when none waited: the next upgrade then
/// replaces the copy.
async fn upgrade_finished(database: &Database) -> Result<(), sqlx::Error> {
    let (_, comment): (String, Option<String>) = sqlx::query_as(OWN_COMMENT)
        .fetch_one(database.pool())
        .await?;
    if without_unfinished(comment.as_deref()).is_none() {
        return Ok(());
    }
    // Two server processes that start at the same time write the comment one after the
    // other. The second one reads the comment after the first one has written it.
    let mut transaction = database
        .pool()
        .begin_with("begin isolation level read committed")
        .await?;
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(START_LOCK)
        .execute(&mut *transaction)
        .await?;
    let (name, comment): (String, Option<String>) = sqlx::query_as(OWN_COMMENT)
        .fetch_one(&mut *transaction)
        .await?;
    if let Some(before) = without_unfinished(comment.as_deref()) {
        sqlx::query(sqlx::AssertSqlSafe(comment_on(&name, before)))
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

/// The newest migration that this version of the server applies. The database of a server
/// that runs has it, and the host command `schema-version` prints it.
pub fn schema_version() -> i64 {
    MIGRATIONS
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

/// The newest migration that the database has. `None` for a database without any schema.
pub async fn schema_of(connection: &mut PgConnection) -> Result<Option<i64>, sqlx::Error> {
    let recorded: bool =
        sqlx::query_scalar("select to_regclass('public._sqlx_migrations') is not null")
            .fetch_one(&mut *connection)
            .await?;
    if !recorded {
        return Ok(None);
    }
    sqlx::query_scalar("select max(version) from _sqlx_migrations where success")
        .fetch_one(connection)
        .await
}

/// True when the database has a schema of an earlier version of the server, so that this
/// start would apply a migration. False for a database without any schema.
async fn migration_waits(database: &Database, migrations: &Migrator) -> Result<bool, sqlx::Error> {
    let mut connection = database.pool().acquire().await?;
    Ok(waiting_from(&mut connection, migrations).await?.is_some())
}

/// The newest migration that the database has, when `migrations` holds a further one.
/// `None` for a database without any schema, and for one that is current.
async fn waiting_from(
    connection: &mut PgConnection,
    migrations: &Migrator,
) -> Result<Option<i64>, sqlx::Error> {
    let recorded: bool =
        sqlx::query_scalar("select to_regclass('public._sqlx_migrations') is not null")
            .fetch_one(&mut *connection)
            .await?;
    if !recorded {
        return Ok(None);
    }
    let applied: Vec<i64> =
        sqlx::query_scalar("select version from _sqlx_migrations where success")
            .fetch_all(&mut *connection)
            .await?;
    let waits = migrations
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .any(|migration| !applied.contains(&migration.version));
    Ok(applied.into_iter().max().filter(|_| waits))
}

/// Whether the server copies its database before a migration (ADR 0005, ADR 0018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MigrationCopy {
    /// A copy as a second database on the same PostgreSQL server.
    #[default]
    Database,
    None,
}

/// The comment of a database that the server made as a copy of the database `name`. The
/// server deletes only a database with the comment of its own database: a copy of another
/// database, whose name can start in the same way, stays. The programs of a deployment
/// find a copy by this text.
fn copy_comment(name: &str) -> String {
    format!("junto: copy of the database {name} before a migration")
}

/// The start of the first line of the comment of a database while an upgrade of it has not
/// applied every migration. The line is written when the copy from before the upgrade
/// exists, before the first migration, and it is removed by the start after which no
/// migration waits. While a database has it, a migration that failed can have left the
/// database migrated in part, and the server makes no copy of it.
///
/// The state is on the database and not on the copy, so that the copy keeps the one
/// comment by which the programs of a deployment find it.
const UPGRADE_UNFINISHED: &str = "junto: an upgrade of this database has not finished";

/// The comment `before` of a database with the line of an unfinished upgrade in front.
fn with_unfinished(before: Option<&str>, copy: &str) -> String {
    let line = format!("{UPGRADE_UNFINISHED}. {copy} is the copy from before the upgrade");
    match before {
        Some(before) if !before.is_empty() => format!("{line}\n{before}"),
        _ => line,
    }
}

/// What a comment held before the line of an unfinished upgrade. `None` for a comment
/// without that line.
fn without_unfinished(comment: Option<&str>) -> Option<Option<&str>> {
    let after = comment?.strip_prefix(UPGRADE_UNFINISHED)?;
    Some(after.split_once('\n').map(|(_, before)| before))
}

/// The start of the first line of the comment of a database while the server makes its copy
/// before an upgrade. The line names the database that the copy is created as, and it is
/// written before that database is created: PostgreSQL creates no database with a comment,
/// so a stop of the server can leave that database without one. A later start deletes such
/// a database only when this line names it. A database with the same name that the server
/// did not record is the database of somebody else.
const COPY_BEING_MADE: &str = "junto: the server makes the copy ";

/// The comment `before` of a database with the line of the copy that is being made in
/// front.
fn with_being_made(before: Option<&str>, making: &str) -> String {
    let line = format!("{COPY_BEING_MADE}{making}");
    match before {
        Some(before) if !before.is_empty() => format!("{line}\n{before}"),
        _ => line,
    }
}

/// The database that the comment states as the copy that is being made, and what the
/// comment held before that line. `None` for a comment without the line.
fn being_made(comment: Option<&str>) -> Option<(&str, Option<&str>)> {
    let after = comment?.strip_prefix(COPY_BEING_MADE)?;
    Some(match after.split_once('\n') {
        Some((making, before)) => (making, Some(before)),
        None => (after, None),
    })
}

fn comment_on(database: &str, comment: Option<&str>) -> String {
    let comment = match comment {
        Some(comment) => format!("'{}'", comment.replace('\'', "''")),
        None => "null".to_owned(),
    };
    format!("comment on database {} is {comment}", quoted(database))
}

/// PostgreSQL cuts a longer name of a database.
const MOST_NAME_BYTES: usize = 63;

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The name of the copy of the database `name` before the migration after `version`.
pub fn copy_name(name: &str, version: i64) -> String {
    format!("{name}_before_{version}")
}

/// The name under which that copy is created. It has the length of [`copy_name`]. The copy
/// gets the name of a copy when it has its comment, because PostgreSQL creates no database
/// with a comment: no stop of the server leaves a database with the name of a copy and
/// without its comment.
fn making_name(name: &str, version: i64) -> String {
    format!("{name}_making_{version}")
}

/// Copies the database before a migration changes it: when this version of the server
/// would apply a migration to a database that has a schema, the database is copied to
/// `<name>_before_<newest applied migration>` on the same PostgreSQL server, and the copy
/// of an upgrade that has finished is deleted. It runs before the server connects, because
/// PostgreSQL copies only a database that nobody is connected to. The way back after a
/// failed upgrade is to rename the copy to the name of the database.
///
/// While an upgrade of the database has not finished, its copy stays and no copy is made: a
/// migration that failed leaves the migrations before it applied, and the earlier version
/// of the server does not start on a copy of that state.
pub async fn copy_before_migration(
    options: &sqlx::postgres::PgConnectOptions,
    mode: MigrationCopy,
) -> Result<MigrationCopied, StartError> {
    copy_before(options, mode, &MIGRATIONS).await
}

/// What a start did about the copy of the database before a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationCopied {
    /// No migration waits, or the copy is turned off.
    Nothing,
    /// The database was copied to the database of this name.
    Made(String),
    /// An upgrade before this start did not apply every migration, so no copy was made.
    /// The name is the copy that holds the state before that upgrade, when the database
    /// server has one.
    Unfinished(Option<String>),
}

/// [`copy_before_migration`] for the migrations of `migrations`.
pub async fn copy_before(
    options: &sqlx::postgres::PgConnectOptions,
    mode: MigrationCopy,
    migrations: &Migrator,
) -> Result<MigrationCopied, StartError> {
    use sqlx::{ConnectOptions, Connection};

    if mode == MigrationCopy::None {
        return Ok(MigrationCopied::Nothing);
    }
    let mut connection = options.connect().await?;
    let waiting = waiting_from(&mut connection, migrations).await?;
    let (name, own): (String, Option<String>) = sqlx::query_as(OWN_COMMENT)
        .fetch_one(&mut connection)
        .await?;
    let known: bool = sqlx::query_scalar("select to_regclass('public.recordings') is not null")
        .fetch_one(&mut connection)
        .await?;
    let recording: i64 = if known {
        sqlx::query_scalar(
            "select count(*) from recordings where state in ('starting', 'recording')",
        )
        .fetch_one(&mut connection)
        .await?
    } else {
        0
    };
    connection.close().await?;
    let Some(version) = waiting else {
        return Ok(MigrationCopied::Nothing);
    };
    if recording > 0 {
        return Err(StartError::RecordingRuns);
    }

    let copy = copy_name(&name, version);
    if copy.len() > MOST_NAME_BYTES {
        return Err(StartError::MigrationCopy(format!(
            "the name {copy} of the copy is longer than the {MOST_NAME_BYTES} bytes of a \
             database name"
        )));
    }
    let failed = |error: sqlx::Error| StartError::MigrationCopy(error.to_string());
    let mut maintenance = options
        .clone()
        .database("postgres")
        .connect()
        .await
        .map_err(failed)?;
    let making = format!("{name}_making_");
    let found: Vec<(String, Option<String>)> = sqlx::query_as(
        "select datname::text, shobj_description(d.oid, 'pg_database') from pg_database d
         where starts_with(datname::text, $1) or starts_with(datname::text, $2)
         order by datname::text",
    )
    .bind(format!("{name}_before_"))
    .bind(&making)
    .fetch_all(&mut maintenance)
    .await
    .map_err(failed)?;
    let (of_copy, first) = (copy_comment(&name), making_name(&name, version));
    let recorded = being_made(own.as_deref());
    let own = match recorded {
        Some((_, before)) => before,
        None => own.as_deref(),
    };
    let unfinished = without_unfinished(own).is_some();
    let (mut left, mut copies, mut taken) = (Vec::new(), Vec::new(), false);
    let mut unknown = false;
    for (database, comment) in found {
        let of_this = comment.as_deref() == Some(of_copy.as_str());
        if database.starts_with(&making) {
            // What a stopped start left: a database with the comment, and, from a stop
            // before the comment, the database that the comment of this database names.
            let named = recorded.is_some_and(|(made, _)| made == database);
            if of_this || (comment.is_none() && named) {
                left.push(database);
            } else if database == first {
                unknown = true;
            }
        } else if of_this {
            copies.push(database);
        } else if database == copy {
            taken = true;
        }
    }
    let deleted = |database: &String| format!("drop database {}", quoted(database));
    let mut statements: Vec<String> = left.iter().map(deleted).collect();
    if !unfinished {
        if taken {
            return Err(StartError::MigrationCopy(format!(
                "a database with the name {copy} exists and does not have the comment of a \
                 copy of this database; give it another name or delete it"
            )));
        }
        if unknown {
            return Err(StartError::MigrationCopy(format!(
                "a database with the name {first} exists, and this server did not record \
                 that it made it; give it another name or delete it"
            )));
        }
        statements.push(comment_on(
            &name,
            Some(with_being_made(own, &first).as_str()),
        ));
        statements.extend(copies.iter().map(deleted));
        statements.extend([
            format!(
                "create database {} template {}",
                quoted(&first),
                quoted(&name)
            ),
            comment_on(&first, Some(of_copy.as_str())),
            format!(
                "alter database {} rename to {}",
                quoted(&first),
                quoted(&copy)
            ),
            // From here on the copy is not replaced until a start has applied every
            // migration.
            comment_on(&name, Some(with_unfinished(own, &copy).as_str())),
        ]);
    }
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut maintenance)
            .await
            .map_err(failed)?;
    }
    maintenance.close().await.map_err(failed)?;
    Ok(if unfinished {
        MigrationCopied::Unfinished(copies.into_iter().next())
    } else {
        MigrationCopied::Made(copy)
    })
}

/// The number of recordings that are starting or run. The server does not migrate while
/// this is above zero (ADR 0005), and the host command `check-upgrade` prints it.
pub async fn recordings_that_run(database: &Database) -> Result<i64, sqlx::Error> {
    let known: bool = sqlx::query_scalar("select to_regclass('public.recordings') is not null")
        .fetch_one(database.pool())
        .await?;
    if !known {
        return Ok(0);
    }
    sqlx::query_scalar("select count(*) from recordings where state in ('starting', 'recording')")
        .fetch_one(database.pool())
        .await
}

async fn start_in(
    connection: &mut PgConnection,
    server_name: &str,
    now: OffsetDateTime,
) -> Result<Started, StartError> {
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(START_LOCK)
        .execute(&mut *connection)
        .await?;

    let existing: Option<(ServerId, bool)> =
        sqlx::query_as("select server_id, owner_account_id is not null from server_state")
            .fetch_optional(&mut *connection)
            .await?;

    let (server_id, has_owner) = match existing {
        Some(state) => state,
        None => {
            let server_id = ServerId::generate();
            let workspace_id = create_workspace(&mut *connection, server_name, now)
                .await?
                .0;
            for (kind, name, position) in [
                (ChannelKind::Text, DEFAULT_TEXT_CHANNEL_NAME, 0),
                (ChannelKind::Voice, DEFAULT_VOICE_CHANNEL_NAME, 1),
            ] {
                sqlx::query(
                    "insert into channels (id, workspace_id, kind, name, position, created_at)
                     values ($1, $2, $3, $4, $5, $6)",
                )
                .bind(ChannelId::generate())
                .bind(workspace_id)
                .bind(kind.stored())
                .bind(name)
                .bind(position)
                .bind(now)
                .execute(&mut *connection)
                .await?;
            }
            sqlx::query(
                "insert into server_state (server_id, name, default_workspace_id, created_at)
                 values ($1, $2, $3, $4)",
            )
            .bind(server_id)
            .bind(server_name)
            .bind(workspace_id)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            (server_id, false)
        }
    };

    let owner_code = if has_owner {
        None
    } else {
        Some(owner::store_new_code(connection, None).await?)
    };

    Ok(Started {
        server_id,
        owner_code,
    })
}

/// A workspace with its "everyone" role and no member. Returns both.
pub async fn create_workspace(
    connection: &mut PgConnection,
    name: &str,
    now: OffsetDateTime,
) -> Result<(WorkspaceId, WorkspaceRole), sqlx::Error> {
    let workspace_id = WorkspaceId::generate();
    sqlx::query("insert into workspaces (id, name, created_at) values ($1, $2, $3)")
        .bind(workspace_id)
        .bind(name)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    let everyone = WorkspaceRole {
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: EVERYONE_ROLE_NAME.to_owned(),
        permissions: db::workspace_list(WorkspacePermissions::EVERYONE_DEFAULT),
        is_everyone: true,
    };
    sqlx::query(
        "insert into workspace_roles (id, workspace_id, name, permissions, is_everyone, created_at)
         values ($1, $2, $3, $4, true, $5)",
    )
    .bind(everyone.id)
    .bind(workspace_id)
    .bind(&everyone.name)
    .bind(db::workspace_bits(WorkspacePermissions::EVERYONE_DEFAULT))
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok((workspace_id, everyone))
}
