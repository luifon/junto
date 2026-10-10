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
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("the client of the media server could not be built: {0}")]
    MediaServer(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Randomness(#[from] RandomnessUnavailable),
    #[error("the database was too busy to finish the start")]
    Busy,
    #[error(transparent)]
    OwnerCode(#[from] HostCommandError),
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
    database.migrate(&MIGRATIONS).await?;
    transact!(database, |connection| {
        start_in(connection, server_name, now).await
    })
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
