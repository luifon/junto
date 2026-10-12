use junto_authz::WorkspacePermissions;
use junto_protocol::ids::{ChannelId, ServerId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::secret::OwnerCode;
use junto_protocol::structure::ChannelKind;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::database::{self, Database, TransactionError};
use crate::db::{self, StoredText};
use crate::owner::{self, HostCommandError};
use crate::secrets::RandomnessUnavailable;
use crate::transact;

pub static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

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
    if migration_waits(database).await? && recordings_that_run(database).await? > 0 {
        return Err(StartError::RecordingRuns);
    }
    database.migrate(&MIGRATIONS).await?;
    transact!(database, |connection| {
        start_in(connection, server_name, now).await
    })
}

/// True when the database has a schema of an earlier version of the server, so that this
/// start would apply a migration. False for a database without any schema.
async fn migration_waits(database: &Database) -> Result<bool, sqlx::Error> {
    let mut connection = database.pool().acquire().await?;
    Ok(waiting_from(&mut connection).await?.is_some())
}

/// The newest migration that the database has, when this version of the server would apply
/// a further one. `None` for a database without any schema, and for one that is current.
async fn waiting_from(connection: &mut PgConnection) -> Result<Option<i64>, sqlx::Error> {
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
    let waits = MIGRATIONS
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
/// database, whose name can start in the same way, stays.
fn copy_comment(name: &str) -> String {
    format!("junto: copy of the database {name} before a migration")
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

/// Copies the database before a migration changes it: when this version of the server
/// would apply a migration to a database that has a schema, the database is copied to
/// `<name>_before_<newest applied migration>` on the same PostgreSQL server, and the copy
/// of an earlier migration is deleted. Returns the name of the copy. It runs before the
/// server connects, because PostgreSQL copies only a database that nobody is connected to.
/// The way back after a failed upgrade is to rename the copy to the name of the database.
pub async fn copy_before_migration(
    options: &sqlx::postgres::PgConnectOptions,
    mode: MigrationCopy,
) -> Result<Option<String>, StartError> {
    use sqlx::{ConnectOptions, Connection};

    if mode == MigrationCopy::None {
        return Ok(None);
    }
    let mut connection = options.connect().await?;
    let waiting = waiting_from(&mut connection).await?;
    let name: String = sqlx::query_scalar("select current_database()::text")
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
        return Ok(None);
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
    let earlier: Vec<String> = sqlx::query_scalar(
        "select datname::text from pg_database d
         where starts_with(datname::text, $1)
           and shobj_description(d.oid, 'pg_database') = $2",
    )
    .bind(format!("{name}_before_"))
    .bind(copy_comment(&name))
    .fetch_all(&mut maintenance)
    .await
    .map_err(failed)?;
    for earlier in earlier {
        let statement = format!("drop database {}", quoted(&earlier));
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut maintenance)
            .await
            .map_err(failed)?;
    }
    for statement in [
        format!(
            "create database {} template {}",
            quoted(&copy),
            quoted(&name)
        ),
        format!(
            "comment on database {} is '{}'",
            quoted(&copy),
            copy_comment(&name).replace('\'', "''")
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut maintenance)
            .await
            .map_err(failed)?;
    }
    maintenance.close().await.map_err(failed)?;
    Ok(Some(copy))
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
