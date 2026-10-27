//! The permission rules of categories and channels, per role and per person (ADR 0304,
//! ADR 0306).

use axum::Json;
use axum::extract::State;
use junto_authz::{Override, Request, WorkspacePermission, WorkspacePermissions};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, CategoryId, ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::{
    CategoryRules, ChangeChannelOverride, Channel, ChannelRules, OverrideState, PermissionRule,
    RulePlace, RuleSubject, SetChannelOverride,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::access::rules::{self, Place, Rules};
use crate::audit::{self, About};
use crate::calls::Calls;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::routes::server_roles::require_active_account;
use crate::state::AppState;
use crate::{read, transact};

/// A change of one rule: both sets, or one permission.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    Set(Override),
    One(WorkspacePermission, OverrideState),
}

impl Change {
    pub(crate) fn set(body: &SetChannelOverride) -> ApiResult<Self> {
        let allow: WorkspacePermissions = body.allow.iter().copied().collect();
        let deny: WorkspacePermissions = body.deny.iter().copied().collect();
        Override::new(allow, deny)
            .map(Self::Set)
            .map_err(|_| ApiError::invalid("a permission is in both allow and deny"))
    }

    pub(crate) const fn one(body: &ChangeChannelOverride) -> Self {
        Self::One(body.permission, body.state)
    }

    pub(crate) fn applied_to(self, previous: Override) -> ApiResult<Override> {
        match self {
            Self::Set(next) => Ok(next),
            Self::One(permission, state) => {
                let changed = WorkspacePermissions::from(permission);
                let mut allow = previous.allow().difference(changed);
                let mut deny = previous.deny().difference(changed);
                match state {
                    OverrideState::Allow => allow = allow.union(changed),
                    OverrideState::Deny => deny = deny.union(changed),
                    OverrideState::Inherit => {}
                }
                Override::new(allow, deny).map_err(|error| ApiError::internal(&error))
            }
        }
    }
}

/// Requires that the subject of a rule is in the workspace: a role of it, or a person whom
/// the caller outranks or who is the caller (ADR 0306). An absent role is `not_found`. The
/// answer tells whether the subject is a current member: the rule of a person who left stays
/// and can be removed, and nothing else can be set for that person (ADR 0304).
async fn require_subject(
    connection: &mut PgConnection,
    access: &Access,
    workspace_id: WorkspaceId,
    subject: RuleSubject,
) -> ApiResult<bool> {
    match subject {
        RuleSubject::Role { role_id } => {
            let exists: bool = sqlx::query_scalar(
                "select exists (select 1 from workspace_roles where id = $1 and workspace_id = $2)",
            )
            .bind(role_id)
            .bind(workspace_id)
            .fetch_one(&mut *connection)
            .await?;
            if !exists {
                return Err(ApiError::not_found());
            }
        }
        RuleSubject::Person { account_id } => {
            let member = is_member(connection, workspace_id, account_id).await?;
            if account_id != access.account_id {
                access
                    .require_above_workspace_account(connection, workspace_id, account_id)
                    .await?;
            }
            return Ok(member);
        }
    }
    Ok(true)
}

/// A person who is not a member keeps the rules that exist and gets no new rule.
fn require_removal_for_former_member(
    member: bool,
    previous: Override,
    next: Override,
) -> ApiResult<()> {
    if member
        || (!previous.is_none() && next.is_none())
        || (!previous.is_none() && previous == next)
    {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

async fn is_member(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    account_id: AccountId,
) -> ApiResult<bool> {
    let member: bool = sqlx::query_scalar(
        "select exists (
             select 1 from workspace_members where workspace_id = $1 and account_id = $2)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(connection)
    .await?;
    Ok(member)
}

pub(crate) async fn require_member(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    account_id: AccountId,
) -> ApiResult<()> {
    if is_member(connection, workspace_id, account_id).await? {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// Requires, for each person whose rule differs between `before` and `after`, that the
/// caller outranks the person or is the person.
async fn require_people(
    connection: &mut PgConnection,
    access: &Access,
    workspace_id: WorkspaceId,
    before: &Rules,
    after: &Rules,
) -> ApiResult<()> {
    let changed = before.keys().chain(after.keys()).filter(|subject| {
        rules::of_subject(before, **subject) != rules::of_subject(after, **subject)
    });
    let mut people: Vec<AccountId> = changed
        .filter_map(|subject| match subject {
            RuleSubject::Person { account_id } if *account_id != access.account_id => {
                Some(*account_id)
            }
            _ => None,
        })
        .collect();
    people.sort_unstable();
    people.dedup();
    for account_id in people {
        access
            .require_above_workspace_account(connection, workspace_id, account_id)
            .await?;
    }
    Ok(())
}

/// A rule that adds a permission to a person needs an active account, as a role does.
async fn require_active_person(
    connection: &mut PgConnection,
    subject: RuleSubject,
    granting: WorkspacePermissions,
) -> ApiResult<()> {
    match subject {
        RuleSubject::Person { account_id } if !granting.is_empty() => {
            require_active_account(connection, account_id).await
        }
        _ => Ok(()),
    }
}

const fn affected(workspace_id: WorkspaceId, subject: RuleSubject) -> Affected {
    match subject {
        RuleSubject::Role { role_id } => Affected::WorkspaceRoleHolders(workspace_id, role_id),
        RuleSubject::Person { account_id } => Affected::Account(account_id),
    }
}

async fn lock_channel(connection: &mut PgConnection, channel_id: ChannelId) -> ApiResult<()> {
    sqlx::query("select id from channels where id = $1 for update")
        .bind(channel_id)
        .fetch_optional(connection)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(())
}

pub(crate) async fn channel_rules_of(
    connection: &mut PgConnection,
    channel: &Channel,
) -> ApiResult<ChannelRules> {
    let synced = rules::synced(connection, channel.id).await?;
    let held = rules::effective(connection, channel.id).await?;
    Ok(ChannelRules {
        channel_id: channel.id,
        category_id: channel.category_id,
        synced,
        rules: rules::listed(&held),
    })
}

/// Records that a channel takes the rules of its category again (`synced`) or no longer.
pub(crate) async fn announce_sync(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    actor: Option<AccountId>,
    (workspace_id, channel_id): (WorkspaceId, ChannelId),
    category_id: Option<CategoryId>,
    synced: bool,
) -> ApiResult<()> {
    audit::record(
        connection,
        now,
        actor,
        About::Channel(workspace_id, channel_id),
        &AuditEvent::ChannelRulesSynced {
            category_id: category_id.filter(|_| synced),
            synced,
        },
    )
    .await?;
    let event = Event::ChannelRulesSynced {
        workspace_id,
        channel_id,
        synced,
    };
    events::publish(connection, now, &event).await
}

/// The rules of a channel: the rules of its category while it is synced with it.
#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/rules",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ChannelRules), ApiError)
)]
pub async fn get_channel_rules(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<ChannelRules>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (record, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        channel_rules_of(connection, &record.channel).await
    })
    .map(Json)
}

/// Sets the allow set and the deny set of one workspace role in a channel. Two empty sets
/// remove the rule. The first change of a synced channel first copies the rules of its
/// category to the channel, which is no longer synced afterwards.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/rules/roles/{role_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path), ("role_id" = WorkspaceRoleId, Path)),
    request_body = SetChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn set_channel_role_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, role_id)): ApiPath<(ChannelId, WorkspaceRoleId)>,
    ApiJson(body): ApiJson<SetChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::set(&body)?;
    channel_rule(
        app,
        bearer,
        channel_id,
        RuleSubject::Role { role_id },
        change,
    )
    .await
}

/// Changes one permission of the rule of a workspace role in a channel.
#[utoipa::path(
    patch,
    path = "/api/v1/channels/{channel_id}/rules/roles/{role_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path), ("role_id" = WorkspaceRoleId, Path)),
    request_body = ChangeChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn change_channel_role_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, role_id)): ApiPath<(ChannelId, WorkspaceRoleId)>,
    ApiJson(body): ApiJson<ChangeChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::one(&body);
    channel_rule(
        app,
        bearer,
        channel_id,
        RuleSubject::Role { role_id },
        change,
    )
    .await
}

/// Sets the allow set and the deny set of one member of the workspace in a channel. The
/// caller outranks the member or is the member.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/rules/people/{account_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    request_body = SetChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn set_channel_person_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
    ApiJson(body): ApiJson<SetChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::set(&body)?;
    let subject = RuleSubject::Person { account_id };
    channel_rule(app, bearer, channel_id, subject, change).await
}

/// Changes one permission of the rule of one member in a channel.
#[utoipa::path(
    patch,
    path = "/api/v1/channels/{channel_id}/rules/people/{account_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    request_body = ChangeChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn change_channel_person_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
    ApiJson(body): ApiJson<ChangeChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::one(&body);
    let subject = RuleSubject::Person { account_id };
    channel_rule(app, bearer, channel_id, subject, change).await
}

async fn channel_rule(
    app: AppState,
    bearer: Bearer,
    channel_id: ChannelId,
    subject: RuleSubject,
    change: Change,
) -> ApiResult<Json<PermissionRule>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let ceiling = app.config.limits.most_person_rules_per_place;
        set_channel_rule_in(
            connection,
            (&app.calls, ceiling),
            &access,
            now,
            (channel_id, subject),
            change,
        )
        .await
    })
    .map(Json)
}

/// Changes the rule of `subject` in a channel with the grant ceiling on what the change adds
/// (ADR 0306). A change that changes nothing writes nothing and keeps the channel synced.
pub(crate) async fn set_channel_rule_in(
    connection: &mut PgConnection,
    (calls, ceiling): (&Calls, u32),
    access: &Access,
    now: OffsetDateTime,
    (channel_id, subject): (ChannelId, RuleSubject),
    change: Change,
) -> ApiResult<PermissionRule> {
    let (record, scope) = access.channel(connection, channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    let workspace_id = record.channel.workspace_id;
    let member = require_subject(connection, access, workspace_id, subject).await?;
    lock_channel(connection, channel_id).await?;
    let previous = rules::of_subject(&rules::effective(connection, channel_id).await?, subject);
    let next = change.applied_to(previous)?;
    require_removal_for_former_member(member, previous, next)?;
    let granting = previous.granted_by_change_to(next);
    access.require(Request::ManageChannelOverrides {
        scope: &scope,
        granting,
    })?;
    require_active_person(connection, subject, granting).await?;
    let result = rules::rule(subject, next);
    if previous == next {
        return Ok(result);
    }

    let detached = rules::synced(connection, channel_id).await?;
    crate::access::inheritance::detach_channels(connection, &[channel_id]).await?;
    if previous.is_none() && matches!(subject, RuleSubject::Person { .. }) {
        rules::require_room_for_person(connection, Place::Channel(channel_id), ceiling).await?;
    }
    rules::write(
        connection,
        Place::Channel(channel_id),
        workspace_id,
        subject,
        next,
    )
    .await?;
    let entry = match subject {
        RuleSubject::Role { role_id } => AuditEvent::ChannelOverrideSet {
            role_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        },
        RuleSubject::Person { account_id } => AuditEvent::ChannelPersonRuleSet {
            account_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        },
    };
    let about = About::Channel(workspace_id, channel_id);
    audit::record(connection, now, Some(access.account_id), about, &entry).await?;
    if detached {
        let ids = (workspace_id, channel_id);
        let actor = Some(access.account_id);
        announce_sync(connection, now, actor, ids, None, false).await?;
    }
    let set = Event::PermissionRuleSet {
        workspace_id,
        place: RulePlace::Channel { channel_id },
        subject,
        allow: result.allow.clone(),
        deny: result.deny.clone(),
    };
    events::publish(connection, now, &set).await?;
    crate::calls::store::mark_channel(connection, calls, channel_id).await?;
    let affected = affected(workspace_id, subject);
    events::permissions_changed(connection, now, affected, Some(workspace_id)).await?;
    Ok(result)
}

/// Removes the own rules of a channel, which takes the rules of its category again. The
/// grant ceiling applies to what this adds (ADR 0306).
#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/rules/sync",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ChannelRules), ApiError)
)]
pub async fn sync_channel_rules(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<ChannelRules>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        sync_channel_in(connection, &app.calls, &access, now, channel_id).await
    })
    .map(Json)
}

pub(crate) async fn sync_channel_in(
    connection: &mut PgConnection,
    calls: &Calls,
    access: &Access,
    now: OffsetDateTime,
    channel_id: ChannelId,
) -> ApiResult<ChannelRules> {
    let (record, scope) = access.channel(connection, channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    let channel = record.channel;
    let category_id = channel
        .category_id
        .ok_or_else(|| ApiError::invalid("the channel has no category to take the rules of"))?;
    lock_channel(connection, channel_id).await?;
    if rules::synced(connection, channel_id).await? {
        return channel_rules_of(connection, &channel).await;
    }
    let before = rules::effective(connection, channel_id).await?;
    let after = rules::of_category(connection, category_id).await?;
    access.require(Request::ManageChannelOverrides {
        scope: &scope,
        granting: rules::granted(&before, &after),
    })?;
    require_people(connection, access, channel.workspace_id, &before, &after).await?;

    rules::sync_channels(connection, &[channel_id]).await?;
    let ids = (channel.workspace_id, channel_id);
    let actor = Some(access.account_id);
    announce_sync(connection, now, actor, ids, Some(category_id), true).await?;
    crate::calls::store::mark_channel(connection, calls, channel_id).await?;
    if before != after {
        let affected = Affected::Workspace(channel.workspace_id);
        events::permissions_changed(connection, now, affected, Some(channel.workspace_id)).await?;
    }
    channel_rules_of(connection, &channel).await
}

/// Sets whether a channel takes the rules of its category: `true` runs the sync of
/// [`sync_channel_in`]; `false` keeps the rules of the category as the channel's own, which
/// changes no permission. A channel already in the requested state is not changed.
pub(crate) async fn set_channel_sync_in(
    connection: &mut PgConnection,
    calls: &Calls,
    access: &Access,
    now: OffsetDateTime,
    (channel_id, synced): (ChannelId, bool),
) -> ApiResult<ChannelRules> {
    if synced {
        return sync_channel_in(connection, calls, access, now, channel_id).await;
    }
    let (record, scope) = access.channel(connection, channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    let channel = record.channel;
    if channel.category_id.is_none() {
        return Err(ApiError::invalid(
            "the channel has no category whose rules it could keep",
        ));
    }
    lock_channel(connection, channel_id).await?;
    if rules::synced(connection, channel_id).await? {
        crate::access::inheritance::detach_channels(connection, &[channel_id]).await?;
        let ids = (channel.workspace_id, channel_id);
        announce_sync(connection, now, Some(access.account_id), ids, None, false).await?;
    }
    channel_rules_of(connection, &channel).await
}

/// What a move of a channel to another category changes in its rules: a synced channel takes
/// the rules of the destination category, and a synced channel that leaves its category for
/// none keeps the rules it had, as its own. The caller must hold what the new rules add
/// (ADR 0306). Called before the category of the channel is changed.
pub(crate) enum MovedRules {
    Unchanged,
    /// The channel keeps the rules of its former category as its own.
    Detach,
    /// The channel takes the rules of the destination category.
    Take(CategoryId),
}

pub(crate) async fn check_move(
    connection: &mut PgConnection,
    access: &Access,
    scope: &junto_authz::ChannelScope,
    channel: &Channel,
    destination: Option<CategoryId>,
) -> ApiResult<MovedRules> {
    let stored_synced: bool =
        sqlx::query_scalar("select permissions_synced from channels where id = $1 for update")
            .bind(channel.id)
            .fetch_one(&mut *connection)
            .await?;
    if !stored_synced || destination == channel.category_id {
        return Ok(MovedRules::Unchanged);
    }
    let Some(destination) = destination else {
        return Ok(MovedRules::Detach);
    };
    let before = rules::effective(connection, channel.id).await?;
    let after = rules::of_category(connection, destination).await?;
    access.require(Request::ManageChannelOverrides {
        scope,
        granting: rules::granted(&before, &after),
    })?;
    require_people(connection, access, channel.workspace_id, &before, &after).await?;
    Ok(MovedRules::Take(destination))
}

/// The rules of a category, which its synced channels take.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path), ("category_id" = CategoryId, Path)),
    security(("session" = [])),
    responses((status = 200, body = CategoryRules), ApiError)
)]
pub async fn get_category_rules(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id)): ApiPath<(WorkspaceId, CategoryId)>,
) -> ApiResult<Json<CategoryRules>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (_, scope) = access
            .visible_category(connection, workspace_id, category_id)
            .await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let held = rules::of_category(connection, category_id).await?;
        Ok(CategoryRules {
            category_id,
            rules: rules::listed(&held),
        })
    })
    .map(Json)
}

/// Sets the allow set and the deny set of one workspace role in a category. The grant
/// ceiling applies in the category and in every synced channel of it.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/roles/{role_id}",
    tag = "channels",
    params(
        ("workspace_id" = WorkspaceId, Path),
        ("category_id" = CategoryId, Path),
        ("role_id" = WorkspaceRoleId, Path)
    ),
    request_body = SetChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn set_category_role_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id, role_id)): ApiPath<(
        WorkspaceId,
        CategoryId,
        WorkspaceRoleId,
    )>,
    ApiJson(body): ApiJson<SetChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::set(&body)?;
    let place = (workspace_id, category_id);
    category_rule(app, bearer, place, RuleSubject::Role { role_id }, change).await
}

/// Changes one permission of the rule of a workspace role in a category.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/roles/{role_id}",
    tag = "channels",
    params(
        ("workspace_id" = WorkspaceId, Path),
        ("category_id" = CategoryId, Path),
        ("role_id" = WorkspaceRoleId, Path)
    ),
    request_body = ChangeChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn change_category_role_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id, role_id)): ApiPath<(
        WorkspaceId,
        CategoryId,
        WorkspaceRoleId,
    )>,
    ApiJson(body): ApiJson<ChangeChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::one(&body);
    let place = (workspace_id, category_id);
    category_rule(app, bearer, place, RuleSubject::Role { role_id }, change).await
}

/// Sets the allow set and the deny set of one member in a category. The caller outranks
/// the member or is the member.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/people/{account_id}",
    tag = "channels",
    params(
        ("workspace_id" = WorkspaceId, Path),
        ("category_id" = CategoryId, Path),
        ("account_id" = AccountId, Path)
    ),
    request_body = SetChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn set_category_person_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id, account_id)): ApiPath<(WorkspaceId, CategoryId, AccountId)>,
    ApiJson(body): ApiJson<SetChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::set(&body)?;
    let place = (workspace_id, category_id);
    let subject = RuleSubject::Person { account_id };
    category_rule(app, bearer, place, subject, change).await
}

/// Changes one permission of the rule of one member in a category.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/people/{account_id}",
    tag = "channels",
    params(
        ("workspace_id" = WorkspaceId, Path),
        ("category_id" = CategoryId, Path),
        ("account_id" = AccountId, Path)
    ),
    request_body = ChangeChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PermissionRule), ApiError)
)]
pub async fn change_category_person_rule(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id, account_id)): ApiPath<(WorkspaceId, CategoryId, AccountId)>,
    ApiJson(body): ApiJson<ChangeChannelOverride>,
) -> ApiResult<Json<PermissionRule>> {
    let change = Change::one(&body);
    let place = (workspace_id, category_id);
    let subject = RuleSubject::Person { account_id };
    category_rule(app, bearer, place, subject, change).await
}

async fn category_rule(
    app: AppState,
    bearer: Bearer,
    place: (WorkspaceId, CategoryId),
    subject: RuleSubject,
    change: Change,
) -> ApiResult<Json<PermissionRule>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let ceiling = app.config.limits.most_person_rules_per_place;
        let calls = (&app.calls, ceiling);
        set_category_rule_in(connection, calls, &access, now, place, (subject, change)).await
    })
    .map(Json)
}

/// Changes the rule of `subject` in a category. The caller must hold what the change adds
/// in the category and in every synced channel of it (ADR 0306).
pub(crate) async fn set_category_rule_in(
    connection: &mut PgConnection,
    (calls, ceiling): (&Calls, u32),
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, category_id): (WorkspaceId, CategoryId),
    (subject, change): (RuleSubject, Change),
) -> ApiResult<PermissionRule> {
    let (_, scope) = access
        .visible_category(connection, workspace_id, category_id)
        .await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    let member = require_subject(connection, access, workspace_id, subject).await?;
    sqlx::query("select id from categories where id = $1 and workspace_id = $2 for update")
        .bind(category_id)
        .bind(workspace_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let previous = rules::of_subject(&rules::of_category(connection, category_id).await?, subject);
    let next = change.applied_to(previous)?;
    require_removal_for_former_member(member, previous, next)?;
    let granting = previous.granted_by_change_to(next);
    access.require(Request::ManageChannelOverrides {
        scope: &scope,
        granting,
    })?;
    let children = rules::synced_children(connection, category_id).await?;
    if !granting.is_empty() && !children.is_empty() {
        let workspace = access.workspace(connection, workspace_id).await?;
        let scoped = access
            .named_channels(connection, workspace_id, &workspace, &children)
            .await?;
        for (_, child) in &scoped {
            access.require(Request::ManageChannelOverrides {
                scope: child,
                granting,
            })?;
        }
    }
    require_active_person(connection, subject, granting).await?;
    let result = rules::rule(subject, next);
    if previous == next {
        return Ok(result);
    }

    if previous.is_none() && matches!(subject, RuleSubject::Person { .. }) {
        rules::require_room_for_person(connection, Place::Category(category_id), ceiling).await?;
    }
    rules::write(
        connection,
        Place::Category(category_id),
        workspace_id,
        subject,
        next,
    )
    .await?;
    let entry = match subject {
        RuleSubject::Role { role_id } => AuditEvent::CategoryRoleRuleSet {
            category_id,
            role_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        },
        RuleSubject::Person { account_id } => AuditEvent::CategoryPersonRuleSet {
            category_id,
            account_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        },
    };
    let about = About::Workspace(workspace_id);
    audit::record(connection, now, Some(access.account_id), about, &entry).await?;
    let set = Event::PermissionRuleSet {
        workspace_id,
        place: RulePlace::Category { category_id },
        subject,
        allow: result.allow.clone(),
        deny: result.deny.clone(),
    };
    events::publish(connection, now, &set).await?;
    for child in children {
        crate::calls::store::mark_channel(connection, calls, child).await?;
    }
    let affected = affected(workspace_id, subject);
    events::permissions_changed(connection, now, affected, Some(workspace_id)).await?;
    Ok(result)
}
