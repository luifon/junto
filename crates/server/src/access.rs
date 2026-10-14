pub mod visibility;

use std::collections::{HashMap, HashSet};

use junto_authz::{
    Actor, ChannelScope, Decision, HeldRole, Membership, Override, Position, Reach, Request,
    RoleKey, RoleOverride, Target, WorkspaceScope, authorize,
};
use junto_authz::{ServerPermission, ServerPermissions, WorkspacePermission};
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
    token_actions: Vec<TokenAction>,
    actor: Actor,
    owner_id: Option<AccountId>,
}

pub struct ChannelRecord {
    pub channel: Channel,
    pub permanent_message_count: i64,
    pub deleted_at: Option<OffsetDateTime>,
    /// Present for an archived channel, which is read-only (ADR 0410).
    pub archived_at: Option<OffsetDateTime>,
}

impl ChannelRecord {
    /// Refuses a change of the content or of the settings of an archived channel.
    pub fn require_open(&self) -> ApiResult<()> {
        match self.archived_at {
            Some(_) => Err(archived()),
            None => Ok(()),
        }
    }
}

pub fn archived() -> ApiError {
    ApiError::conflict("the channel is archived and takes no change; restore it first")
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
    Option<OffsetDateTime>,
);

const CHANNEL_COLUMNS: &str = "id, workspace_id, category_id, kind, name, position, \
     permanent_messages, permanent_message_count, deleted_at, user_limit, hides_occupants, \
     archived_at";

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
            return Self::open_with_token(connection, &bearer.0, action, false, now).await;
        }
        if bearer.1 == Credential::RolePositionsToken {
            return Self::open_with_token(
                connection,
                &bearer.0,
                TokenAction::ReadStructure,
                true,
                now,
            )
            .await;
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
        let access = Self::load(connection, account_id, device_id, session_id).await?;
        crate::receipts::authorize_session(account_id);
        Ok(access)
    }

    /// The caller of a request with an access token on a route of `action`: the account that
    /// created the token, as it stands now (ADR 0203). The token must hold the action, or
    /// the narrowly allowed alternative for reading role positions. The
    /// owner position is not carried: a token of the server owner acts with the
    /// administrator permission (ADR 0210).
    async fn open_with_token(
        connection: &mut PgConnection,
        hash: &[u8; 32],
        action: TokenAction,
        reads_role_positions: bool,
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
        let held = tokens::stored_actions(actions)?;
        if !(held.contains(&action)
            || reads_role_positions && held.contains(&TokenAction::ManageMembers))
        {
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
        access.token_actions = held;
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

    /// The current standing of an active account, including a person who is invited or moved
    /// and the caller after a commit. `None` when the account does not exist or is not active.
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
            token_actions: Vec::new(),
            actor: Actor {
                position,
                server_roles,
            },
            owner_id,
        })
    }

    pub async fn rank(
        &self,
        connection: &mut PgConnection,
        workspace: Option<WorkspaceId>,
    ) -> ApiResult<junto_authz::RoleRank> {
        Self::rank_of(connection, self.account_id, self.owner_id, workspace).await
    }

    pub async fn rank_of(
        connection: &mut PgConnection,
        account: AccountId,
        owner: Option<AccountId>,
        workspace: Option<WorkspaceId>,
    ) -> ApiResult<junto_authz::RoleRank> {
        let rows: Vec<(i32, i64)> = sqlx::query_as(
            "select r.position, r.permissions from account_server_roles a
             join server_roles r on r.id = a.role_id where a.account_id = $1",
        )
        .bind(account)
        .fetch_all(&mut *connection)
        .await?;
        let server = rows
            .into_iter()
            .map(|(position, bits)| {
                db::server_permissions(bits).map(|permissions| (position, permissions))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let workspace_positions: Option<Vec<i32>> = if let Some(workspace) = workspace {
            Some(sqlx::query_scalar(
                "select r.position from member_roles m join workspace_roles r on r.id = m.role_id
                 where m.workspace_id = $1 and m.account_id = $2",
            ).bind(workspace).bind(account).fetch_all(&mut *connection).await?)
        } else {
            None
        };
        Ok(junto_authz::role_rank(
            owner == Some(account),
            &server,
            workspace_positions.as_deref(),
        ))
    }

    pub fn permits_token_action(&self, action: TokenAction) -> bool {
        self.token_id.is_none() || self.token_actions.contains(&action)
    }

    pub async fn require_above_restored_roles(
        &self,
        connection: &mut PgConnection,
        account: AccountId,
    ) -> ApiResult<()> {
        let ranks = RestoredRanks::load(connection, self, &[account]).await?;
        ranks.require(self, account)
    }

    pub async fn require_above_account(
        &self,
        connection: &mut PgConnection,
        account: AccountId,
    ) -> ApiResult<()> {
        let actor = self.rank(connection, None).await?;
        let target = Self::rank_of(connection, account, self.owner_id, None).await?;
        Self::require_rank(actor, target)
    }

    pub async fn require_above_workspace_account(
        &self,
        connection: &mut PgConnection,
        workspace: WorkspaceId,
        account: AccountId,
    ) -> ApiResult<()> {
        let actor = self.rank(connection, Some(workspace)).await?;
        let target = Self::rank_of(connection, account, self.owner_id, Some(workspace)).await?;
        Self::require_rank(actor, target)
    }

    pub fn require_rank(
        actor: junto_authz::RoleRank,
        target: junto_authz::RoleRank,
    ) -> ApiResult<()> {
        if junto_authz::outranks(actor, target) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "the role order refuses this action",
            ))
        }
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

    /// The workspace permissions that the caller holds in a workspace: each permission for
    /// which the permission function allows a request in the workspace (ADR 0310).
    pub fn workspace_permissions(&self, scope: &WorkspaceScope) -> Vec<WorkspacePermission> {
        WorkspacePermission::ALL
            .iter()
            .copied()
            .filter(|permission| {
                self.allows(Request::Workspace {
                    scope,
                    permission: *permission,
                })
            })
            .collect()
    }

    /// The workspace permissions that the caller holds in a channel: each permission for
    /// which the permission function allows a request in the channel (ADR 0310).
    pub fn channel_permissions(&self, scope: &ChannelScope) -> Vec<WorkspacePermission> {
        WorkspacePermission::ALL
            .iter()
            .copied()
            .filter(|permission| {
                self.allows(Request::Channel {
                    scope,
                    permission: *permission,
                })
            })
            .collect()
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
        Ok(self
            .workspace_channels_and_overrides(connection, workspace_id, workspace)
            .await?
            .0)
    }

    pub(crate) async fn workspace_channels_and_overrides(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        workspace: &WorkspaceScope,
    ) -> ApiResult<(Vec<(ChannelRecord, ChannelScope)>, Vec<OverrideRow>)> {
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
        Ok((
            scoped_channels(rows, overrides.clone(), workspace)?,
            overrides,
        ))
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

/// The workspaces in `workspace_ids` that an account is a member of.
pub async fn memberships_among(
    connection: &mut PgConnection,
    account_id: AccountId,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<HashSet<WorkspaceId>> {
    let joined: Vec<WorkspaceId> = sqlx::query_scalar(
        "select workspace_id from workspace_members
         where account_id = $1 and workspace_id = any($2)",
    )
    .bind(account_id)
    .bind(workspace_ids)
    .fetch_all(connection)
    .await?;
    Ok(joined.into_iter().collect())
}

/// Whether an account reads an invite: its list entry, its events and its revocation. The
/// account holds `create_invites`, and it created the invite or may see every workspace
/// that the invite names (ADR 0111). `member_of` holds the workspaces, of the ones that the
/// invite names, that the account is a member of.
pub fn reads_invite(
    actor: &Actor,
    is_creator: bool,
    workspace_ids: &[WorkspaceId],
    member_of: &HashSet<WorkspaceId>,
) -> bool {
    let allows = |request: Request<'_>| authorize(actor, &request).is_allowed();
    let sees = |workspace_id: &WorkspaceId| {
        let scope = WorkspaceScope {
            membership: if member_of.contains(workspace_id) {
                Membership::Member
            } else {
                Membership::NotMember
            },
            everyone: junto_authz::WorkspacePermissions::EMPTY,
            roles: Vec::new(),
        };
        allows(Request::ViewWorkspace(&scope))
    };
    allows(Request::Server(ServerPermission::CreateInvites))
        && (is_creator || workspace_ids.iter().all(sees))
}

/// One channel by its identifier, deleted or not, for the work of the server itself, which
/// asks no permission.
pub async fn stored_channel(
    connection: &mut PgConnection,
    channel_id: ChannelId,
) -> ApiResult<Option<ChannelRecord>> {
    let row: Option<ChannelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {CHANNEL_COLUMNS} from channels where id = $1"
    )))
    .bind(channel_id)
    .fetch_optional(connection)
    .await?;
    row.map(channel_record).transpose()
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

pub(crate) type OverrideRow = (ChannelId, WorkspaceRoleId, bool, i64, i64);

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
        archived_at,
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
        archived_at,
    })
}

pub(crate) struct RestoredRanks {
    server: HashMap<AccountId, Vec<(i32, ServerPermissions)>>,
    workspace: HashMap<AccountId, HashMap<WorkspaceId, Vec<i32>>>,
}

impl RestoredRanks {
    pub(crate) async fn load(
        connection: &mut PgConnection,
        access: &Access,
        accounts: &[AccountId],
    ) -> ApiResult<Self> {
        let mut ids = accounts.to_vec();
        ids.push(access.account_id);
        let rows: Vec<(AccountId, i32, i64)> = sqlx::query_as("select a.account_id,r.position,r.permissions from account_server_roles a join server_roles r on r.id=a.role_id where a.account_id=any($1)")
            .bind(&ids).fetch_all(&mut *connection).await?;
        let mut server: HashMap<AccountId, Vec<(i32, ServerPermissions)>> = HashMap::new();
        for (account, position, bits) in rows {
            server
                .entry(account)
                .or_default()
                .push((position, db::server_permissions(bits)?));
        }
        let rows: Vec<(AccountId, WorkspaceId, i32)> = sqlx::query_as("select m.account_id,m.workspace_id,r.position from member_roles m join workspace_roles r on r.id=m.role_id where m.account_id=any($1)")
            .bind(&ids).fetch_all(connection).await?;
        let mut workspace: HashMap<AccountId, HashMap<WorkspaceId, Vec<i32>>> = HashMap::new();
        for (account, id, position) in rows {
            workspace
                .entry(account)
                .or_default()
                .entry(id)
                .or_default()
                .push(position);
        }
        Ok(Self { server, workspace })
    }

    fn rank(
        &self,
        access: &Access,
        account: AccountId,
        workspace: Option<WorkspaceId>,
    ) -> junto_authz::RoleRank {
        let server = self.server.get(&account).map_or(&[][..], Vec::as_slice);
        let positions = workspace.map(|id| {
            self.workspace
                .get(&account)
                .and_then(|rows| rows.get(&id))
                .map_or(&[][..], Vec::as_slice)
        });
        junto_authz::role_rank(access.owner_id == Some(account), server, positions)
    }

    pub(crate) fn require(&self, access: &Access, account: AccountId) -> ApiResult<()> {
        if access.owner_id == Some(access.account_id) {
            return Ok(());
        }
        Access::require_rank(
            self.rank(access, access.account_id, None),
            self.rank(access, account, None),
        )?;
        if let Some(workspaces) = self.workspace.get(&account) {
            for id in workspaces.keys() {
                Access::require_rank(
                    self.rank(access, access.account_id, Some(*id)),
                    self.rank(access, account, Some(*id)),
                )?;
            }
        }
        Ok(())
    }
}
