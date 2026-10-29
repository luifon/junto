use crate::{
    access::{Access, memberships_among},
    error::{ApiError, ApiResult},
    extract::{ApiPath, Bearer},
    read,
    state::AppState,
    store,
};
use axum::{Json, extract::State};
use junto_authz::{Membership, Request, WorkspacePermissions, WorkspaceScope};
use junto_protocol::{
    account::{AccountCard, ProfileRole, WorkspaceProfileRoles},
    ids::{AccountId, WorkspaceId},
};
use uuid::Uuid;

#[utoipa::path(get, path="/api/v1/accounts/{account_id}/card",tag="account",params(("account_id"=AccountId,Path)),security(("session"=[])),responses((status=200,body=AccountCard),ApiError))]
pub async fn get_card(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<Json<AccountCard>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(Request::OwnAccount)?;
        super::account::require_sight(connection, &access, account_id).await?;
        let active: bool = sqlx::query_scalar("select exists(select 1 from accounts where id=$1 and status='active')").bind(account_id).fetch_one(&mut *connection).await?;
        if !active { return Err(ApiError::not_found()); }
        let profile = store::profile_of(connection, account_id).await?.filter(|profile| profile.departure.is_none()).ok_or_else(ApiError::not_found)?;
        let roles: Vec<(Uuid, String, i32)> = sqlx::query_as("select r.id,r.name,r.position from server_roles r join account_server_roles h on h.role_id=r.id where h.account_id=$1 order by r.position desc,r.id")
            .bind(account_id).fetch_all(&mut *connection).await?;
        let server_roles = roles.into_iter().map(|(id,name,position)| ProfileRole { id,name,position }).collect();
        let workspaces: Vec<(WorkspaceId, String)> = sqlx::query_as("select w.id,w.name from workspaces w join workspace_members m on m.workspace_id=w.id where m.account_id=$1 and w.deleted_at is null order by lower(w.name),w.id")
            .bind(account_id).fetch_all(&mut *connection).await?;
        let ids: Vec<_> = workspaces.iter().map(|(id,_)| *id).collect();
        let joined = memberships_among(connection, access.account_id, &ids).await?;
        let workspaces: Vec<_> = workspaces.into_iter().filter(|(id,_)| {
            let scope = WorkspaceScope { membership: if joined.contains(id) { Membership::Member } else { Membership::NotMember }, everyone: WorkspacePermissions::EMPTY, roles: Vec::new(), person_override: Default::default() };
            access.allows(Request::ViewWorkspace(&scope))
        }).collect();
        let ids: Vec<_> = workspaces.iter().map(|(id,_)| *id).collect();
        let roles: Vec<(WorkspaceId, Uuid, String, i32)> = sqlx::query_as("select r.workspace_id,r.id,r.name,r.position from workspace_roles r join member_roles h on h.role_id=r.id where h.account_id=$1 and r.workspace_id=any($2) and not r.is_everyone order by r.position desc,r.id")
            .bind(account_id).bind(&ids).fetch_all(&mut *connection).await?;
        let workspaces = workspaces.into_iter().filter_map(|(workspace_id,name)| {
            let roles: Vec<_> = roles.iter().filter(|(workspace,_,_,_)| *workspace == workspace_id).map(|(_,id,name,position)| ProfileRole { id:*id,name:name.clone(),position:*position }).collect();
            (!roles.is_empty()).then_some(WorkspaceProfileRoles {workspace_id,name,roles})
        }).collect();
        Ok(AccountCard { profile, server_roles, workspaces })
    }).map(Json)
}
