pub mod visibility;

use std::collections::HashMap;

use junto_authz::{
    Actor, ChannelScope, Decision, HeldRole, Membership, Override, Position, Reach, Request,
    RoleKey, RoleOverride, Target, WorkspaceScope, authorize,
};
use junto_authz::{ServerPermission, ServerPermissions};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{
    AccessTokenId, AccountId, CategoryId, ChannelId, DeviceId, ServerRoleId, WorkspaceId,
    WorkspaceRoleId,
};
use junto_protocol::structure::{Channel, ChannelKind};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult};
use crate::extract::{Bearer, Credential};
use crate::secrets::constant_time_eq;
use crate::tokens;
use junto_protocol::access_token::TokenAction;

/// The caller of a request as the permission function needs it. Every route decides through
/// [`Access::require`] or [`Access::allows`], which call the one function of `junto-authz`.
pub struct Access {
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub session_id: Uuid,
    pub server_role_ids: Vec<ServerRoleId>,
    /// The access token of the request. The account is then the creator of the token, and
    /// the request has no device and no session.
    pub token_id: Option<AccessTokenId>,
    actor: Actor,
    owner_id: Option<AccountId>,
}

pub struct ChannelRecord {
    pub channel: Channel,
    pub permanent_message_count: i64,
    pub deleted_at: Option<OffsetDateTime>,
}

type ChannelRow = (
    ChannelId,
    WorkspaceId,
    Option<CategoryId>,
    String,
    String,
    i32,
    bool,
    i64,
    Option<OffsetDateTime>,
    Option<i32>,
    bool,
);

const CHANNEL_COLUMNS: &str = "id, workspace_id, category_id, kind, name, position, \
     permanent_messages, permanent_message_count, deleted_at, user_limit, hides_occupants";

impl Access {
    /// Verifies the session or the access token of the request and loads the caller, inside
    /// the transaction that the request then works in. The session or the token must be
    /// unexpired and its account active.
    pub async fn open(
        connection: &mut PgConnection,
        bearer: &Bearer,
        now: OffsetDateTime,
    ) -> ApiResult<Self> {
        if let Credential::AccessToken(action) = bearer.1 {
            return Self::open_with_token(connection, &bearer.0, action, now).await;
        }
        let session: Option<(Uuid, Vec<u8>, DeviceId, AccountId)> = sqlx::query_as(
            "select s.id, s.token_hash, d.id, d.account_id
             from sessions s
             join devices d on d.id = s.device_id
             join accounts a on a.id = d.account_id
             where s.token_hash = $1 and s.expires_at > $2 and a.status = 'active'",
        )
        .bind(bearer.0.as_slice())
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let (session_id, device_id, account_id) = match session {
            Some((session_id, stored_hash, device_id, account_id))
                if constant_time_eq(&stored_hash, &bearer.0) =>
            {
                (session_id, device_id, account_id)
            }
            _ => return Err(ApiError::unauthenticated()),
        };
        Self::load(connection, account_id, device_id, session_id).await
    }

    /// The caller of a request with an access token on a route of `action`: the account that
    /// created the token, as it stands now (ADR 0203). The token must hold the action. The
    /// owner position is not carried: a token of the server owner acts with the
    /// administrator permission (ADR 0210).
    async fn open_with_token(
        connection: &mut PgConnection,
        hash: &[u8; 32],
        action: TokenAction,
        now: OffsetDateTime,
    ) -> ApiResult<Self> {
        let token: Option<(AccessTokenId, Vec<u8>, AccountId, i64)> = sqlx::query_as(
            "select t.id, t.token_hash, t.created_by, t.actions
             from access_tokens t
             join accounts a on a.id = t.created_by
             where t.token_hash = $1 and t.expires_at > $2 and a.status = 'active'",
        )
        .bind(hash.as_slice())
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let (token_id, account_id, actions) = match token {
            Some((token_id, stored_hash, account_id, actions))
                if constant_time_eq(&stored_hash, hash) =>
            {
                (token_id, account_id, actions)
            }
            _ => {
                return Err(ApiError::new(
                    ErrorCode::Unauthenticated,
                    "the access token is unknown, has expired or was revoked",
                ));
            }
        };
        if !tokens::stored_actions(actions)?.contains(&action) {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                format!(
                    "the access token does not allow the action {}",
                    action.name()
                ),
            ));
        }
        sqlx::query("select set_config($1, $2, true)")
            .bind(crate::audit::ACCESS_TOKEN_SETTING)
            .bind(token_id.to_string())
            .execute(&mut *connection)
            .await?;
        let nobody = DeviceId::from_uuid(Uuid::nil());
        let mut access = Self::load(connection, account_id, nobody, Uuid::nil()).await?;
        access.token_id = Some(token_id);
        if access.actor.position == Position::Owner {
            access.actor.position = Position::Member;
            access.actor.server_roles.push(
                [ServerPermission::Administrator]
                    .into_iter()
                    .collect::<ServerPermissions>(),
            );
        }
        Ok(access)
    }

    /// The standing of an active account that is not the caller of the request: a person who
    /// is invited or moved. `None` when the account does not exist or is not active.
    pub async fn of_account(
        connection: &mut PgConnection,
        account_id: AccountId,
    ) -> ApiResult<Option<Self>> {
        let active: bool = sqlx::query_scalar(
            "select exists (select 1 from accounts where id = $1 and status = 'active')",
        )
        .bind(account_id)
        .fetch_one(&mut *connection)
        .await?;
        if !active {
            return Ok(None);
        }
        let nobody = DeviceId::from_uuid(Uuid::nil());
        Ok(Some(
            Self::load(connection, account_id, nobody, Uuid::nil()).await?,
        ))
    }

    /// The standing of the account behind a session that was valid when a call was joined.
    /// `None` when the session no longer counts: it has ended or expired, its device is
    /// gone, or its account is not active.
    pub async fn of_session(
        connection: &mut PgConnection,
        account_id: AccountId,
        device_id: DeviceId,
        session_id: Uuid,
        now: OffsetDateTime,
    ) -> ApiResult<Option<Self>> {
        let counts: bool = sqlx::query_scalar(
            "select exists (
                 select 1 from sessions s
                 join devices d on d.id = s.device_id
                 join accounts a on a.id = d.account_id
                 where s.id = $1 and d.id = $2 and a.id = $3
                   and s.expires_at > $4 and a.status = 'active')",
        )
        .bind(session_id)
        .bind(device_id)
        .bind(account_id)
        .bind(now)
        .fetch_one(&mut *connection)
        .await?;
        if !counts {
            return Ok(None);
        }
        Ok(Some(
            Self::load(connection, account_id, device_id, session_id).await?,
        ))
    }

    async fn load(
        connection: &mut PgConnection,
        account_id: AccountId,
        device_id: DeviceId,
        session_id: Uuid,
    ) -> ApiResult<Self> {
        let owner_id: Option<AccountId> =
            sqlx::query_scalar("select owner_account_id from server_state")
                .fetch_one(&mut *connection)
                .await?;
        let roles: Vec<(ServerRoleId, i64)> = sqlx::query_as(
            "select r.id, r.permissions
             from account_server_roles a
             join server_roles r on r.id = a.role_id
             where a.account_id = $1
             order by r.id",
        )
        .bind(account_id)
        .fetch_all(&mut *connection)
        .await?;

        let position = if owner_id == Some(account_id) {
            Position::Owner
        } else {
            Position::Member
        };
        let server_roles = roles
            .iter()
            .map(|(_, bits)| db::server_permissions(*bits))
            .collect::<ApiResult<Vec<_>>>()?;
        Ok(Self {
            account_id,
            device_id,
            session_id,
            server_role_ids: roles.into_iter().map(|(id, _)| id).collect(),
            token_id: None,
            actor: Actor {
                position,
                server_roles,
            },
            owner_id,
        })
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    pub fn is_owner(&self) -> bool {
        self.actor.position == Position::Owner
    }

    pub fn allows(&self, request: Request<'_>) -> bool {
        authorize(&self.actor, &request).is_allowed()
    }

    pub fn require(&self, request: Request<'_>) -> ApiResult<()> {
        match authorize(&self.actor, &request) {
            Decision::Allow => Ok(()),
            Decision::Deny(denial) => Err(denial.into()),
        }
    }

    pub fn target(&self, account_id: AccountId) -> Target {
        if self.owner_id == Some(account_id) {
            Target::Owner
        } else if account_id == self.account_id {
            Target::Actor
        } else {
            Target::Other
        }
    }

    /// The caller's standing in one workspace. The workspace must exist.
    pub async fn workspace(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
    ) -> ApiResult<WorkspaceScope> {
        let rows: Vec<(WorkspaceRoleId, i64, bool)> = sqlx::query_as(
            "select r.id, r.permissions, r.is_everyone
             from workspace_roles r
             where r.workspace_id = $1
               and (r.is_everyone or exists (
                   select 1 from member_roles m
                   where m.role_id = r.id and m.account_id = $2))",
        )
        .bind(workspace_id)
        .bind(self.account_id)
        .fetch_all(&mut *connection)
        .await?;
        let is_member: bool = sqlx::query_scalar(
            "select exists (
                 select 1 from workspace_members where workspace_id = $1 and account_id = $2)",
        )
        .bind(workspace_id)
        .bind(self.account_id)
        .fetch_one(&mut *connection)
        .await?;

        let mut everyone = None;
        let mut roles = Vec::new();
        for (id, bits, is_everyone) in rows {
            let permissions = db::workspace_permissions(bits)?;
            if is_everyone {
                everyone = Some(permissions);
            } else {
                roles.push(HeldRole {
                    key: role_key(id),
                    permissions,
                });
            }
        }
        let everyone = everyone.ok_or_else(ApiError::not_found)?;
        Ok(WorkspaceScope {
            membership: if is_member {
                Membership::Member
            } else {
                Membership::NotMember
            },
            everyone,
            roles,
        })
    }

    /// Whether the caller is a member of every workspace in `workspace_ids`.
    pub async fn reach(
        &self,
        connection: &mut PgConnection,
        workspace_ids: &[WorkspaceId],
    ) -> ApiResult<Reach> {
        let joined: i64 = sqlx::query_scalar(
            "select count(*) from workspace_members
             where account_id = $1 and workspace_id = any($2)",
        )
        .bind(self.account_id)
        .bind(workspace_ids)
        .fetch_one(&mut *connection)
        .await?;
        Ok(if usize::try_from(joined) == Ok(workspace_ids.len()) {
            Reach::WithinMemberships
        } else {
            Reach::BeyondMemberships
        })
    }

    /// One channel that is not deleted, with the caller's standing in it.
    pub async fn channel(
        &self,
        connection: &mut PgConnection,
        channel_id: ChannelId,
    ) -> ApiResult<(ChannelRecord, ChannelScope)> {
        let row: Option<ChannelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CHANNEL_COLUMNS} from channels where id = $1 and deleted_at is null"
        )))
        .bind(channel_id)
        .fetch_optional(&mut *connection)
        .await?;
        let record = channel_record(row.ok_or_else(ApiError::not_found)?)?;
        let workspace = self
            .workspace(&mut *connection, record.channel.workspace_id)
            .await?;
        let overrides: Vec<OverrideRow> = sqlx::query_as(
            "select o.channel_id, o.role_id, r.is_everyone, o.allow, o.deny
             from channel_overrides o
             join workspace_roles r on r.id = o.role_id
             where o.channel_id = $1",
        )
        .bind(channel_id)
        .fetch_all(&mut *connection)
        .await?;
        let scope = channel_scope(workspace, &overrides)?;
        Ok((record, scope))
    }

    /// Every channel of a workspace, deleted ones included, with the caller's standing in each.
    pub async fn workspace_channels(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        workspace: &WorkspaceScope,
    ) -> ApiResult<Vec<(ChannelRecord, ChannelScope)>> {
        let rows: Vec<ChannelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CHANNEL_COLUMNS} from channels where workspace_id = $1 order by position, id"
        )))
        .bind(workspace_id)
        .fetch_all(&mut *connection)
        .await?;
        let overrides: Vec<OverrideRow> = sqlx::query_as(
            "select o.channel_id, o.role_id, r.is_everyone, o.allow, o.deny
             from channel_overrides o
             join workspace_roles r on r.id = o.role_id
             where o.workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_all(&mut *connection)
        .await?;
        scoped_channels(rows, overrides, workspace)
    }

    /// The channels of a workspace with the identifiers in `channel_ids`, deleted ones
    /// included, with the caller's standing in each.
    pub async fn named_channels(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        workspace: &WorkspaceScope,
        channel_ids: &[ChannelId],
    ) -> ApiResult<Vec<(ChannelRecord, ChannelScope)>> {
        let rows: Vec<ChannelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CHANNEL_COLUMNS} from channels where workspace_id = $1 and id = any($2)"
        )))
        .bind(workspace_id)
        .bind(channel_ids)
        .fetch_all(&mut *connection)
        .await?;
        let overrides: Vec<OverrideRow> = sqlx::query_as(
            "select o.channel_id, o.role_id, r.is_everyone, o.allow, o.deny
             from channel_overrides o
             join workspace_roles r on r.id = o.role_id
             where o.workspace_id = $1 and o.channel_id = any($2)",
        )
        .bind(workspace_id)
        .bind(channel_ids)
        .fetch_all(&mut *connection)
        .await?;
        scoped_channels(rows, overrides, workspace)
    }
}

fn scoped_channels(
    rows: Vec<ChannelRow>,
    overrides: Vec<OverrideRow>,
    workspace: &WorkspaceScope,
) -> ApiResult<Vec<(ChannelRecord, ChannelScope)>> {
    let mut by_channel: HashMap<ChannelId, Vec<OverrideRow>> = HashMap::new();
    for row in overrides {
        by_channel.entry(row.0).or_default().push(row);
    }
    rows.into_iter()
        .map(|row| {
            let record = channel_record(row)?;
            let overrides = by_channel
                .get(&record.channel.id)
                .map_or(&[][..], Vec::as_slice);
            let scope = channel_scope(workspace.clone(), overrides)?;
            Ok((record, scope))
        })
        .collect()
}

type OverrideRow = (ChannelId, WorkspaceRoleId, bool, i64, i64);

pub fn role_key(id: WorkspaceRoleId) -> RoleKey {
    RoleKey(id.as_uuid().as_u128())
}

pub fn stored_override(allow: i64, deny: i64) -> ApiResult<Override> {
    Override::new(
        db::workspace_permissions(allow)?,
        db::workspace_permissions(deny)?,
    )
    .map_err(|error| ApiError::internal(&error))
}

fn channel_scope(workspace: WorkspaceScope, overrides: &[OverrideRow]) -> ApiResult<ChannelScope> {
    let mut everyone_override = Override::NONE;
    let mut role_overrides = Vec::new();
    for (_, role_id, is_everyone, allow, deny) in overrides {
        let rule = stored_override(*allow, *deny)?;
        if *is_everyone {
            everyone_override = rule;
        } else {
            role_overrides.push(RoleOverride {
                role: role_key(*role_id),
                rule,
            });
        }
    }
    Ok(ChannelScope {
        workspace,
        everyone_override,
        role_overrides,
    })
}

fn channel_record(row: ChannelRow) -> ApiResult<ChannelRecord> {
    let (
        id,
        workspace_id,
        category_id,
        kind,
        name,
        position,
        permanent_messages,
        permanent_message_count,
        deleted_at,
        user_limit,
        hides_occupants,
    ) = row;
    Ok(ChannelRecord {
        channel: Channel {
            id,
            workspace_id,
            category_id,
            kind: ChannelKind::read(&kind)?,
            name,
            position,
            permanent_messages,
            user_limit: user_limit.map(db::count).transpose()?,
            hides_occupants,
        },
        permanent_message_count,
        deleted_at,
    })
}
