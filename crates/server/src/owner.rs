//! The server owner: the stored owner code, and the two host commands that work only with
//! direct access to the database (ADR 0114).

use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, StreamEpoch};
use junto_protocol::secret::OwnerCode;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};

use crate::audit::{self, About};
use crate::database::{self, Database, TransactionError};
use crate::error::ApiResult;
use crate::events;
use crate::events::log::Affected;
use crate::secrets::{
    CodePurpose, OWNER_CODE_SYMBOLS, RandomnessUnavailable, hash_code, random_code,
};
use crate::transact;

pub struct StoredOwnerCode {
    pub owner: Option<AccountId>,
    pub hash: Option<Vec<u8>>,
    pub expires_at: Option<OffsetDateTime>,
}

/// Reads the owner and the owner code, and locks the row until the transaction ends.
pub async fn locked_code(connection: &mut PgConnection) -> ApiResult<StoredOwnerCode> {
    let (owner, hash, expires_at) = sqlx::query_as(
        "select owner_account_id, owner_code_hash, owner_code_expires_at
         from server_state
         for update",
    )
    .fetch_one(connection)
    .await?;
    Ok(StoredOwnerCode {
        owner,
        hash,
        expires_at,
    })
}

pub async fn clear_code(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("update server_state set owner_code_hash = null, owner_code_expires_at = null")
        .execute(connection)
        .await?;
    Ok(())
}

/// Stores a new owner code in place of the one before and returns it. `expires_at` is absent
/// for the code of a server without an owner.
pub async fn store_new_code(
    connection: &mut PgConnection,
    expires_at: Option<OffsetDateTime>,
) -> Result<OwnerCode, HostCommandError> {
    let code = random_code(OWNER_CODE_SYMBOLS)?;
    sqlx::query("update server_state set owner_code_hash = $1, owner_code_expires_at = $2")
        .bind(hash_code(CodePurpose::Owner, &code).as_slice())
        .bind(expires_at)
        .execute(connection)
        .await?;
    Ok(OwnerCode::new(code))
}

#[derive(Debug, thiserror::Error)]
pub enum HostCommandError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Randomness(#[from] RandomnessUnavailable),
    #[error("the database was too busy; run the command again")]
    Busy,
    #[error("the server has not been started on this database yet")]
    NotStarted,
    #[error("no account has this identifier")]
    UnknownAccount,
    #[error("the account is banned or removed; only an active account can own the server")]
    AccountNotActive,
    #[error("the entry for the audit log or the event could not be written")]
    Audit,
    #[error("the command lost against a concurrent change")]
    Conflict,
}

fn is_undefined_table(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "42P01")
}

impl TransactionError for HostCommandError {
    fn is_conflict(&self) -> bool {
        match self {
            Self::Database(error) => database::is_conflict(error),
            Self::Conflict => true,
            _ => false,
        }
    }

    fn exhausted() -> Self {
        Self::Busy
    }
}

pub(crate) async fn require_started(
    connection: &mut PgConnection,
) -> Result<Option<AccountId>, HostCommandError> {
    let owner: Result<Option<Option<AccountId>>, sqlx::Error> =
        sqlx::query_scalar("select owner_account_id from server_state for update")
            .fetch_optional(connection)
            .await;
    match owner {
        Ok(owner) => owner.ok_or(HostCommandError::NotStarted),
        Err(error) if is_undefined_table(&error) => Err(HostCommandError::NotStarted),
        Err(error) => Err(error.into()),
    }
}

/// Makes `account_id` the server owner. The owner before keeps its account and loses the
/// position. A stored owner code stops working.
pub async fn assign(
    database: &Database,
    account_id: AccountId,
    now: OffsetDateTime,
) -> Result<(), HostCommandError> {
    transact!(database, |connection| {
        let previous_owner_id = require_started(connection).await?;
        let status: Option<String> =
            sqlx::query_scalar("select status from accounts where id = $1 for update")
                .bind(account_id)
                .fetch_optional(&mut *connection)
                .await?;
        match status.as_deref() {
            None => return Err(HostCommandError::UnknownAccount),
            Some("active") => {}
            Some(_) => return Err(HostCommandError::AccountNotActive),
        }
        sqlx::query(
            "update server_state
             set owner_account_id = $1, owner_code_hash = null, owner_code_expires_at = null",
        )
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            None,
            About::Server,
            &AuditEvent::OwnerAssigned {
                account_id,
                previous_owner_id,
            },
        )
        .await
        .map_err(audit_failure)?;
        announce_owner(connection, now, account_id, previous_owner_id)
            .await
            .map_err(audit_failure)
    })
}

/// Records the change of the owner: for the people who list accounts, and for the two
/// accounts, whose permissions changed.
async fn announce_owner(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    previous_owner_id: Option<AccountId>,
) -> ApiResult<()> {
    if previous_owner_id == Some(account_id) {
        return Ok(());
    }
    let changed = Event::OwnerChanged {
        account_id,
        previous_owner_id,
    };
    events::publish(connection, now, &changed).await?;
    for affected in previous_owner_id.into_iter().chain([account_id]) {
        events::permissions_changed(connection, now, Affected::Account(affected), None).await?;
        crate::store::contact_changed(connection, now, affected).await?;
    }
    Ok(())
}

/// Gives the event stream a new epoch, so that every cursor that a client holds is unknown
/// and every client loads a snapshot (ADR 0010). It is run after a restore of a backup.
pub async fn renew_event_epoch(
    database: &Database,
    now: OffsetDateTime,
) -> Result<StreamEpoch, HostCommandError> {
    let epoch = StreamEpoch::generate();
    transact!(database, |connection| {
        require_started(connection).await?;
        sqlx::query("update event_stream set epoch = $1")
            .bind(epoch)
            .execute(&mut *connection)
            .await?;
        audit::record(
            connection,
            now,
            None,
            About::Server,
            &AuditEvent::EventEpochRenewed,
        )
        .await
        .map_err(audit_failure)?;
        Ok(epoch)
    })
}

pub struct IssuedOwnerCode {
    pub code: OwnerCode,
    /// Present on a server that has an owner.
    pub expires_at: Option<OffsetDateTime>,
}

/// Issues a new owner code. On a server with an owner the code lives `lifetime` and gives
/// the device that presents it access to the account of the owner.
pub async fn issue_code(
    database: &Database,
    lifetime: Duration,
    now: OffsetDateTime,
) -> Result<IssuedOwnerCode, HostCommandError> {
    transact!(database, |connection| {
        let expires_at = require_started(connection).await?.map(|_| now + lifetime);
        let stored: (Option<Vec<u8>>, Option<OffsetDateTime>) =
            sqlx::query_as("select owner_code_hash, owner_code_expires_at from server_state")
                .fetch_one(&mut *connection)
                .await?;
        let replaced_a_code = match stored {
            (Some(_), Some(stored_expiry)) => stored_expiry > now,
            (Some(_), None) => true,
            (None, _) => false,
        };
        let code = store_new_code(connection, expires_at).await?;
        audit::record(
            connection,
            now,
            None,
            About::Server,
            &AuditEvent::OwnerCodeIssued {
                replaced_a_code,
                expires_at,
            },
        )
        .await
        .map_err(audit_failure)?;
        Ok(IssuedOwnerCode { code, expires_at })
    })
}

fn audit_failure(error: crate::error::ApiError) -> HostCommandError {
    if error.is_conflict() {
        HostCommandError::Conflict
    } else {
        HostCommandError::Audit
    }
}
