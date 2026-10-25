use std::collections::{HashMap, HashSet};

use junto_authz::{Acquaintance, Contact, Request};
use junto_protocol::account::{
    AccountProfile, AccountStatus, AccountSummary, Avatar, Departure, Device, DeviceOrigin,
    ImageType, OwnAccount, RecoveryNotice,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::auth::SessionCreated;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{
    AccountId, DeviceId, JoinRequestId, RecoveryId, ServerRoleId, WorkspaceId,
};
use junto_protocol::keys::PublicKey;
use junto_protocol::secret::SessionToken;
use junto_protocol::structure::WorkspaceMember;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::calls::Calls;
use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult, violates};
use crate::events;
use crate::events::log::Affected;
use crate::secrets::{hash_session_token, new_session_token};
use crate::state::AppState;

pub async fn default_workspace(connection: &mut PgConnection) -> ApiResult<WorkspaceId> {
    Ok(
        sqlx::query_scalar("select default_workspace_id from server_state")
            .fetch_one(connection)
            .await?,
    )
}

/// The workspaces that an invite or an approval grants: the named ones without repeats, or the
/// default workspace when none is named.
pub async fn granted_workspaces(
    connection: &mut PgConnection,
    named: &[WorkspaceId],
) -> ApiResult<Vec<WorkspaceId>> {
    if named.is_empty() {
        return Ok(vec![default_workspace(connection).await?]);
    }
    let mut granted = named.to_vec();
    granted.sort();
    granted.dedup();
    Ok(granted)
}

pub async fn require_workspaces_exist(
    connection: &mut PgConnection,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<()> {
    let existing: i64 = sqlx::query_scalar(
        "select count(*) from workspaces where id = any($1) and deleted_at is null",
    )
    .bind(workspace_ids)
    .fetch_one(connection)
    .await?;
    if usize::try_from(existing) == Ok(workspace_ids.len()) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

pub async fn create_account(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    name: &str,
) -> ApiResult<AccountId> {
    let id = AccountId::generate();
    sqlx::query(
        "insert into accounts (id, name, status, created_at) values ($1, $2, 'active', $3)",
    )
    .bind(id)
    .bind(name)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let account = AccountSummary {
        id,
        name: name.to_owned(),
        status: AccountStatus::Active,
        is_owner: false,
        server_role_ids: Vec::new(),
        created_at: now,
    };
    events::publish(connection, now, &Event::AccountAdded { account }).await?;
    Ok(id)
}

/// The caller's account as `GET /api/v1/account` and the snapshot return it.
pub async fn own_account(connection: &mut PgConnection, access: &Access) -> ApiResult<OwnAccount> {
    let name: String = sqlx::query_scalar("select name from accounts where id = $1")
        .bind(access.account_id)
        .fetch_one(&mut *connection)
        .await?;
    let workspace_ids: Vec<WorkspaceId> = sqlx::query_scalar(
        "select workspace_id from workspace_members
         where account_id = $1
         order by workspace_id",
    )
    .bind(access.account_id)
    .fetch_all(&mut *connection)
    .await?;
    let avatar = avatar_of(connection, access.account_id).await?;
    Ok(OwnAccount {
        id: access.account_id,
        name,
        avatar,
        is_owner: access.is_owner(),
        server_permissions: db::server_list(access.actor().server_permissions()),
        server_role_ids: access.server_role_ids.clone(),
        workspace_ids,
    })
}

pub async fn server_role_ids_of(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Vec<ServerRoleId>> {
    Ok(sqlx::query_scalar(
        "select role_id from account_server_roles where account_id = $1 order by role_id",
    )
    .bind(account_id)
    .fetch_all(connection)
    .await?)
}

/// Ends the waiting join request of `public_key` as withdrawn: the device withdrew it, or
/// the key has an account by another way. Approvers get `join_request_withdrawn`.
pub async fn end_waiting_join_request(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    public_key: &PublicKey,
) -> ApiResult<Option<JoinRequestId>> {
    let ended: Option<JoinRequestId> = sqlx::query_scalar(
        "update join_requests set state = 'withdrawn', decided_at = $2
         where public_key = $1 and state = 'pending' and expires_at > $2
         returning id",
    )
    .bind(public_key.as_bytes().as_slice())
    .bind(now)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(join_request_id) = ended {
        crate::audit::record(
            connection,
            now,
            None,
            crate::audit::About::Server,
            &AuditEvent::JoinRequestWithdrawn { join_request_id },
        )
        .await?;
        events::publish(
            connection,
            now,
            &Event::JoinRequestWithdrawn { join_request_id },
        )
        .await?;
    }
    Ok(ended)
}

pub async fn create_device(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    public_key: &PublicKey,
    name: &str,
    origin: DeviceOrigin,
) -> ApiResult<Device> {
    let id = DeviceId::generate();
    let inserted = sqlx::query(
        "insert into devices (id, account_id, public_key, name, origin, created_at)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(account_id)
    .bind(public_key.as_bytes().as_slice())
    .bind(name)
    .bind(origin.stored())
    .bind(now)
    .execute(&mut *connection)
    .await;
    match inserted {
        Ok(_) => {
            if origin != DeviceOrigin::JoinRequest {
                end_waiting_join_request(connection, now, public_key).await?;
            }
            sqlx::query(
                "delete from moderation_notice_devices where public_key=$1 and account_id<>$2",
            )
            .bind(public_key.as_bytes().as_slice())
            .bind(account_id)
            .execute(&mut *connection)
            .await?;
            let added = Event::DeviceAdded {
                device_id: id,
                name: name.to_owned(),
                public_key: *public_key,
                origin,
                created_at: now,
            };
            events::tell_account(connection, now, account_id, &added).await?;
            Ok(Device {
                id,
                name: name.to_owned(),
                public_key: *public_key,
                origin,
                created_at: now,
                last_login_at: None,
                current: false,
            })
        }
        Err(error) if violates(&error, "devices_public_key_unique") => {
            Err(key_already_registered())
        }
        Err(error) => Err(error.into()),
    }
}

pub fn key_already_registered() -> ApiError {
    ApiError::new(
        ErrorCode::KeyAlreadyRegistered,
        "the device key already belongs to an account",
    )
}

pub async fn require_key_unregistered(
    connection: &mut PgConnection,
    public_key: &PublicKey,
) -> ApiResult<()> {
    let registered: bool =
        sqlx::query_scalar("select exists (select 1 from devices where public_key = $1)")
            .bind(public_key.as_bytes().as_slice())
            .fetch_one(connection)
            .await?;
    if registered {
        Err(key_already_registered())
    } else {
        Ok(())
    }
}

pub async fn add_members(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<()> {
    sqlx::query(
        "insert into workspace_members (workspace_id, account_id, joined_at)
         select workspace_id, $1, $2 from unnest($3::uuid[]) as granted (workspace_id)",
    )
    .bind(account_id)
    .bind(now)
    .bind(workspace_ids)
    .execute(&mut *connection)
    .await?;
    announce_members(connection, now, account_id, workspace_ids).await
}

/// Records that an account became a member of each workspace: for the members of the
/// workspace, and for the account, whose permissions changed.
pub async fn announce_members(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<()> {
    let name: String = sqlx::query_scalar("select name from accounts where id = $1")
        .bind(account_id)
        .fetch_one(&mut *connection)
        .await?;
    let avatar = avatar_of(connection, account_id).await?;
    for workspace_id in workspace_ids {
        let added = Event::WorkspaceMemberAdded {
            workspace_id: *workspace_id,
            member: WorkspaceMember {
                avatar: avatar.clone(),
                account_id,
                name: name.clone(),
                role_ids: Vec::new(),
            },
        };
        events::publish(connection, now, &added).await?;
        let affected = Affected::Account(account_id);
        events::permissions_changed(connection, now, affected, Some(*workspace_id)).await?;
    }
    Ok(())
}

/// Starts a session of a device and records the login. The response carries the recoveries of
/// the account that this device has not acknowledged; a login does not remove them
/// (ADR 0112).
pub async fn start_session(
    app: &AppState,
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    device_id: DeviceId,
) -> ApiResult<SessionCreated> {
    let recoveries = recovery_notices_of(connection, device_id).await?;

    sqlx::query("update devices set last_login_at = $2 where id = $1")
        .bind(device_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    let logged_in = Event::DeviceLoggedIn {
        device_id,
        logged_in_at: now,
    };
    events::tell_account(connection, now, account_id, &logged_in).await?;

    let token = new_session_token()?;
    let expires_at = now + app.config.limits.session_lifetime;
    sqlx::query(
        "insert into sessions (id, device_id, token_hash, created_at, expires_at)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(device_id)
    .bind(hash_session_token(&token).as_slice())
    .bind(now)
    .bind(expires_at)
    .execute(&mut *connection)
    .await?;
    end_surplus_sessions(app, connection, now, device_id).await?;

    Ok(SessionCreated {
        token: SessionToken::new(token),
        expires_at,
        account_id,
        device_id,
        recoveries,
    })
}

/// Ends the oldest sessions of a device beyond the number that a device holds, with their
/// places in calls (ADR 0109). The newest sessions are the ones that the device uses. No
/// reason is stored: the connection of such a session ends as for an expired session.
async fn end_surplus_sessions(
    app: &AppState,
    connection: &mut PgConnection,
    now: OffsetDateTime,
    device_id: DeviceId,
) -> ApiResult<()> {
    let surplus: Vec<Uuid> = sqlx::query_scalar(
        "select id from sessions where device_id = $1
         order by created_at desc, id desc
         offset $2 limit 100",
    )
    .bind(device_id)
    .bind(i64::from(app.config.limits.most_sessions_per_device))
    .fetch_all(&mut *connection)
    .await?;
    for session_id in &surplus {
        let session = Sessions::One(*session_id);
        crate::calls::store::end_places_of(&mut *connection, &app.calls, now, session).await?;
    }
    if !surplus.is_empty() {
        sqlx::query("delete from sessions where id = any($1)")
            .bind(&surplus)
            .execute(connection)
            .await?;
    }
    Ok(())
}

type RecoveryRow = (
    RecoveryId,
    Option<DeviceId>,
    Option<AccountId>,
    OffsetDateTime,
);

fn recovery_rows(rows: Vec<RecoveryRow>) -> Vec<RecoveryNotice> {
    rows.into_iter()
        .map(
            |(id, device_id, performed_by, performed_at)| RecoveryNotice {
                id,
                device_id,
                performed_by,
                performed_at,
            },
        )
        .collect()
}

/// The recoveries that a device has not acknowledged, oldest first.
pub async fn recovery_notices_of(
    connection: &mut PgConnection,
    device_id: DeviceId,
) -> ApiResult<Vec<RecoveryNotice>> {
    Ok(recovery_rows(
        sqlx::query_as(
            "select r.id, r.device_id, r.performed_by, r.performed_at
             from recovery_notices n
             join recoveries r on r.id = n.recovery_id
             where n.device_id = $1
             order by r.performed_at, r.id",
        )
        .bind(device_id)
        .fetch_all(connection)
        .await?,
    ))
}

type AvatarRow = (Vec<u8>, String, i32, i32, i32);

const AVATAR_COLUMNS: &str = "hash, media_type, octet_length(content), width, height";

fn avatar(row: AvatarRow) -> ApiResult<Avatar> {
    let (hash, media_type, bytes, width, height) = row;
    let media_type = ImageType::ALL
        .iter()
        .copied()
        .find(|kind| kind.media_type() == media_type)
        .ok_or_else(|| ApiError::internal(&format_args!("unknown stored type {media_type:?}")))?;
    Ok(Avatar {
        hash: hash.iter().map(|byte| format!("{byte:02x}")).collect(),
        media_type,
        bytes: db::count(bytes)?,
        width: db::count(width)?,
        height: db::count(height)?,
    })
}

pub async fn avatar_of(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Option<Avatar>> {
    let row: Option<AvatarRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {AVATAR_COLUMNS} from account_avatars where account_id = $1"
    )))
    .bind(account_id)
    .fetch_optional(connection)
    .await?;
    row.map(avatar).transpose()
}

/// The profile of an account that exists. The profile of an inactive or a deleted account
/// states its departure and has no avatar (ADR 0121).
pub async fn profile_of(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Option<AccountProfile>> {
    let row: Option<(String, String)> =
        sqlx::query_as("select name, status from accounts where id = $1")
            .bind(account_id)
            .fetch_optional(&mut *connection)
            .await?;
    let Some((name, status)) = row else {
        return Ok(None);
    };
    let departure = departure_of(AccountStatus::read(&status)?);
    let avatar = match departure {
        Some(_) => None,
        None => avatar_of(connection, account_id).await?,
    };
    Ok(Some(AccountProfile {
        id: account_id,
        name,
        avatar,
        departure,
    }))
}

/// The departure that a profile states for an account in this state.
pub const fn departure_of(status: AccountStatus) -> Option<Departure> {
    match status {
        AccountStatus::Inactive => Some(Departure::Inactive),
        AccountStatus::Deleted => Some(Departure::Deleted),
        AccountStatus::Active
        | AccountStatus::Banned
        | AccountStatus::Removed
        | AccountStatus::Unknown => None,
    }
}

/// Records the current profile of an account for the accounts that may see it.
pub async fn announce_profile(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<AccountProfile> {
    let profile = profile_of(&mut *connection, account_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let updated = Event::AccountProfileUpdated {
        profile: profile.clone(),
    };
    events::publish(connection, now, &updated).await?;
    Ok(profile)
}

/// The accounts of `$2` that share a workspace with the account `$1`: for each workspace of
/// `$1` and each account named, one lookup by the key of the membership, so the rows read
/// follow those two lists and not the other members of the workspaces (ADR 0307).
pub const FELLOW_MEMBERS: &str = "select distinct asked.id
     from unnest($2::uuid[]) asked(id)
     cross join lateral (
         select workspace_id from workspace_members where account_id = $1 offset 0
     ) mine
     cross join lateral (
         select account_id from workspace_members
         where workspace_id = mine.workspace_id and account_id = asked.id offset 0
     ) theirs";

/// The accounts in `account_ids` that are members of a workspace that `account_id` is a
/// member of.
pub async fn fellow_members(
    connection: &mut PgConnection,
    account_id: AccountId,
    account_ids: &[AccountId],
) -> ApiResult<HashSet<AccountId>> {
    let fellows: Vec<AccountId> = sqlx::query_scalar(FELLOW_MEMBERS)
        .bind(account_id)
        .bind(account_ids)
        .fetch_all(connection)
        .await?;
    Ok(fellows.into_iter().collect())
}

/// How the caller stands to another account, for the decision whether it may see it.
pub async fn acquaintance(
    connection: &mut PgConnection,
    access: &Access,
    account_id: AccountId,
) -> ApiResult<Acquaintance> {
    if account_id == access.account_id {
        return Ok(Acquaintance::Own);
    }
    // The profile of a departed account goes to the members of the workspaces that it left
    // by itself and to its conversation peers (ADR 0121); a holder of `ban_members` sees it
    // as every profile.
    let witness: bool = sqlx::query_scalar(
        "select exists (select 1 from accounts where id = $1 and status in ('inactive', 'deleted'))
            and (exists (select 1 from account_departures d
                         join workspace_members m on m.workspace_id = d.workspace_id
                         where d.account_id = $1 and m.account_id = $2)
                 or exists (select 1 from conversations
                            where (low_account_id = $1 and high_account_id = $2)
                               or (low_account_id = $2 and high_account_id = $1)))",
    )
    .bind(account_id)
    .bind(access.account_id)
    .fetch_one(&mut *connection)
    .await?;
    if witness {
        return Ok(Acquaintance::Departed);
    }
    let fellows = fellow_members(connection, access.account_id, &[account_id]).await?;
    if fellows.contains(&account_id) {
        return Ok(Acquaintance::SharedWorkspace);
    }
    // An account that the caller may write to is one whose profile it sees (ADR 0119,
    // ADR 0307).
    Ok(match contact(connection, access, account_id).await? {
        Some(Contact::Administrator) => Acquaintance::Administrator,
        _ => Acquaintance::Unrelated,
    })
}

/// Records the notice `permissions_changed` for the accounts that have a direct-message
/// conversation with `account_id`. Whether they reach the account can have changed: its
/// state, its memberships or its administrator permission did (ADR 0307). The change
/// records this, because no other event of it goes to those accounts (ADR 0208).
pub async fn contact_changed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<()> {
    let has_peers: bool = sqlx::query_scalar(
        "select exists (select 1 from conversations where low_account_id = $1)
             or exists (select 1 from conversations where high_account_id = $1)",
    )
    .bind(account_id)
    .fetch_one(&mut *connection)
    .await?;
    if has_peers {
        let peers = events::Receivers::ConversationPeers(account_id);
        events::permissions_changed_for(connection, now, peers, None).await?;
    }
    Ok(())
}

/// Like [`contact_changed`], for every holder of a server role whose administrator
/// permission changes or that is deleted with it.
pub async fn contact_changed_for_holders(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    role_id: ServerRoleId,
) -> ApiResult<()> {
    let has_peers: bool = sqlx::query_scalar(
        "select exists (
                 select 1 from account_server_roles h
                 join conversations c on c.low_account_id = h.account_id
                 where h.role_id = $1)
             or exists (
                 select 1 from account_server_roles h
                 join conversations c on c.high_account_id = h.account_id
                 where h.role_id = $1)",
    )
    .bind(role_id)
    .fetch_one(&mut *connection)
    .await?;
    if has_peers {
        let peers = events::Receivers::ConversationPeersOfServerRole(role_id);
        events::permissions_changed_for(connection, now, peers, None).await?;
    }
    Ok(())
}

/// How the caller stands to another account for a direct message or a direct call. `None`
/// when that account is not active: nobody reaches it.
pub async fn contact(
    connection: &mut PgConnection,
    access: &Access,
    account_id: AccountId,
) -> ApiResult<Option<Contact>> {
    let Some(other) = Access::of_account(&mut *connection, account_id).await? else {
        return Ok(None);
    };
    let fellows = fellow_members(connection, access.account_id, &[account_id]).await?;
    Ok(Some(if fellows.contains(&account_id) {
        Contact::SharedWorkspace
    } else if other.actor().is_administrator() {
        Contact::Administrator
    } else {
        Contact::Unrelated
    }))
}

/// Whether an account departed: it is inactive or deleted, or a moderator removed it after
/// its departure. A direct conversation with it stays readable (ADR 0121).
pub async fn departed(connection: &mut PgConnection, account_id: AccountId) -> ApiResult<bool> {
    Ok(departed_among(connection, &[account_id])
        .await?
        .contains(&account_id))
}

/// The accounts of `account_ids` that departed, as [`departed`] states it.
pub async fn departed_among(
    connection: &mut PgConnection,
    account_ids: &[AccountId],
) -> ApiResult<HashSet<AccountId>> {
    if account_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let departed: Vec<AccountId> = sqlx::query_scalar(
        "select id from accounts where id = any($1) and departed_at is not null
           and status in ('inactive', 'deleted', 'removed')",
    )
    .bind(account_ids)
    .fetch_all(connection)
    .await?;
    Ok(departed.into_iter().collect())
}

/// Whether the caller and another account may exchange direct messages and direct calls
/// now: both are active, and they share a workspace or one of them administers the server
/// (ADR 0307). Every decision about a direct message or a direct call uses this function or
/// the same request of the permission function.
pub async fn may_reach(
    connection: &mut PgConnection,
    access: &Access,
    account_id: AccountId,
) -> ApiResult<bool> {
    Ok(contact(connection, access, account_id)
        .await?
        .is_some_and(|contact| access.allows(Request::Contact(contact))))
}

pub async fn recoveries_of(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Vec<RecoveryNotice>> {
    Ok(recovery_rows(
        sqlx::query_as(
            "select id, device_id, performed_by, performed_at
             from recoveries
             where account_id = $1
             order by performed_at, id",
        )
        .bind(account_id)
        .fetch_all(connection)
        .await?,
    ))
}

pub type DeviceRow = (
    DeviceId,
    String,
    Vec<u8>,
    String,
    OffsetDateTime,
    Option<OffsetDateTime>,
);

pub const DEVICE_COLUMNS: &str = "id, name, public_key, origin, created_at, last_login_at";

pub fn device(row: DeviceRow, current: DeviceId) -> ApiResult<Device> {
    let (id, name, public_key, origin, created_at, last_login_at) = row;
    Ok(Device {
        id,
        name,
        public_key: db::public_key(&public_key)?,
        origin: DeviceOrigin::read(&origin)?,
        created_at,
        last_login_at,
        current: id == current,
    })
}

/// Why a session was ended before its expiry. The reason is stored with the identifier of
/// the session, for the connection that the session opened (ADR 0208).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    LoggedOut,
    DeviceRemoved,
    Banned,
    Removed,
    Recovered(RecoveryId),
    Inactive,
    Deleted,
}

impl SessionEnd {
    pub const fn stored(self) -> &'static str {
        match self {
            Self::LoggedOut => "logged_out",
            Self::DeviceRemoved => "device_removed",
            Self::Banned => "banned",
            Self::Removed => "removed",
            Self::Recovered(_) => "recovered",
            Self::Inactive => "inactive",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Sessions {
    One(Uuid),
    OfDevice(DeviceId),
    OfAccount(AccountId),
}

/// Stores why the sessions end, and ends the places in calls that belong to them, with their
/// moves: a session that has ended holds no identity in a room (ADR 0510). It runs before
/// the statement that deletes the sessions.
pub async fn record_ended(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    sessions: Sessions,
    end: SessionEnd,
) -> ApiResult<()> {
    crate::calls::store::end_places_of(&mut *connection, calls, now, sessions).await?;
    let (filter, id) = match sessions {
        Sessions::One(id) => ("s.id = $1", id),
        Sessions::OfDevice(id) => ("s.device_id = $1", id.as_uuid()),
        Sessions::OfAccount(id) => ("d.account_id = $1", id.as_uuid()),
    };
    let recovery_id = match end {
        SessionEnd::Recovered(id) => Some(id),
        _ => None,
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "insert into ended_sessions (session_id, account_id, reason, recovery_id, ended_at)
         select s.id, d.account_id, $2, $3, $4
         from sessions s
         join devices d on d.id = s.device_id
         where {filter}
         on conflict (session_id) do nothing"
    )))
    .bind(id)
    .bind(end.stored())
    .bind(recovery_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// Ends every session of an account, and with them its access tokens: a credential of the
/// account that was created before its ban, its removal or its recovery does not work after
/// it (ADR 0109, ADR 0210).
pub async fn end_sessions(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    account_id: AccountId,
    end: SessionEnd,
) -> ApiResult<()> {
    let sessions = Sessions::OfAccount(account_id);
    record_ended(&mut *connection, calls, now, sessions, end).await?;
    sqlx::query(
        "delete from sessions where device_id in (select id from devices where account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    crate::tokens::end_all_of(connection, now, account_id).await
}

/// The new device of a recovery and who performed it.
pub struct Recovery<'a> {
    pub account_id: AccountId,
    pub public_key: &'a PublicKey,
    pub device_name: &'a str,
    pub origin: DeviceOrigin,
    pub performed_by: Option<AccountId>,
}

/// Gives `public_key` access to an account as a new device: ends every session of the
/// account, records the recovery, and leaves one notice for each device of the account.
pub async fn recover(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    recovery: Recovery<'_>,
) -> ApiResult<RecoveryNotice> {
    let Recovery {
        account_id,
        public_key,
        device_name,
        origin,
        performed_by,
    } = recovery;
    let device = create_device(
        &mut *connection,
        now,
        account_id,
        public_key,
        device_name,
        origin,
    )
    .await?;
    let notice = RecoveryNotice {
        id: RecoveryId::generate(),
        device_id: Some(device.id),
        performed_by,
        performed_at: now,
    };
    sqlx::query(
        "insert into recoveries (id, account_id, device_id, performed_by, performed_at)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(notice.id)
    .bind(account_id)
    .bind(device.id)
    .bind(notice.performed_by)
    .bind(notice.performed_at)
    .execute(&mut *connection)
    .await?;
    let end = SessionEnd::Recovered(notice.id);
    end_sessions(&mut *connection, calls, now, account_id, end).await?;
    sqlx::query(
        "insert into recovery_notices (recovery_id, device_id)
         select $1, id from devices where account_id = $2",
    )
    .bind(notice.id)
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    let recovered = Event::AccountRecovered {
        recovery: notice.clone(),
    };
    events::tell_account(connection, now, account_id, &recovered).await?;
    Ok(notice)
}

/// The active accounts of `$1`, each with whether it owns the server and the permissions of
/// each server role that it holds. Each lookup is a lateral subquery with `offset 0`, a
/// lookup by key for each account named: a join could become a merge or a hash join that
/// reads every role assignment, so that the time of a list of contacts would depend on
/// accounts that the caller may not see (ADR 0307).
pub const CONTACT_STANDING: &str = "select a.id, coalesce(a.id = s.owner_account_id, false),
            coalesce(held.permissions, '{}')
     from unnest($1::uuid[]) asked(id)
     cross join lateral (
         select id, status from accounts where id = asked.id offset 0
     ) a
     cross join server_state s
     cross join lateral (
         select array_agg(r.permissions) as permissions
         from (
             select role_id from account_server_roles where account_id = a.id offset 0
         ) assigned
         cross join lateral (
             select permissions from server_roles where id = assigned.role_id offset 0
         ) r
     ) held
     where a.status = 'active'";

/// Contact standing for a set of peers, loaded without a query per conversation.
pub async fn contacts(
    connection: &mut PgConnection,
    access: &Access,
    accounts: &[AccountId],
) -> ApiResult<HashMap<AccountId, Contact>> {
    let rows: Vec<(AccountId, bool, Vec<i64>)> = sqlx::query_as(CONTACT_STANDING)
        .bind(accounts)
        .fetch_all(&mut *connection)
        .await?;
    let fellows = fellow_members(connection, access.account_id, accounts).await?;
    rows.into_iter()
        .map(|(id, owner, roles)| {
            let actor = junto_authz::Actor {
                position: if owner {
                    junto_authz::Position::Owner
                } else {
                    junto_authz::Position::Member
                },
                server_roles: roles
                    .into_iter()
                    .map(db::server_permissions)
                    .collect::<ApiResult<_>>()?,
            };
            Ok((
                id,
                if fellows.contains(&id) {
                    Contact::SharedWorkspace
                } else if actor.is_administrator() {
                    Contact::Administrator
                } else {
                    Contact::Unrelated
                },
            ))
        })
        .collect()
}
