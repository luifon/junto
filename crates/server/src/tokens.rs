//! Access tokens of administrators (ADR 0203, ADR 0210): which routes a token reaches, and
//! the stored tokens.

use axum::http::Method;
use junto_protocol::access_token::{AccessTokenInfo, TokenAction};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccessTokenId, AccountId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::error::{ApiError, ApiResult};
use crate::events;

use TokenAction::{ManageMembers, ManageStructure, ReadAuditLog, ReadStructure};

/// Every route that an access token reaches, with the action that the token must hold. A
/// route that is not listed refuses every access token. A test compares the list with the
/// generated OpenAPI document.
///
/// No route that gives or takes a role of an account and no route that lifts a ban is
/// listed: a request with a token changes no role assignment and lifts no ban (ADR 0213).
pub const REACH: &[(&str, &str, TokenAction)] = &[
    ("GET", "/api/v1/structure", ReadStructure),
    ("GET", "/api/v1/workspaces", ReadStructure),
    (
        "GET",
        "/api/v1/workspaces/{workspace_id}/roles",
        ReadStructure,
    ),
    (
        "GET",
        "/api/v1/workspaces/{workspace_id}/channels",
        ReadStructure,
    ),
    (
        "GET",
        "/api/v1/workspaces/{workspace_id}/members",
        ReadStructure,
    ),
    (
        "GET",
        "/api/v1/channels/{channel_id}/overrides",
        ReadStructure,
    ),
    ("GET", "/api/v1/server-roles", ReadStructure),
    ("POST", "/api/v1/structure/changes", ManageStructure),
    ("POST", "/api/v1/workspaces", ManageStructure),
    (
        "PATCH",
        "/api/v1/workspaces/{workspace_id}",
        ManageStructure,
    ),
    (
        "POST",
        "/api/v1/workspaces/{workspace_id}/categories",
        ManageStructure,
    ),
    (
        "PATCH",
        "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
        ManageStructure,
    ),
    (
        "POST",
        "/api/v1/workspaces/{workspace_id}/channels",
        ManageStructure,
    ),
    (
        "DELETE",
        "/api/v1/workspaces/{workspace_id}",
        ManageStructure,
    ),
    (
        "DELETE",
        "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
        ManageStructure,
    ),
    (
        "DELETE",
        "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
        ManageStructure,
    ),
    ("DELETE", "/api/v1/server-roles/{role_id}", ManageStructure),
    ("PATCH", "/api/v1/channels/{channel_id}", ManageStructure),
    (
        "PATCH",
        "/api/v1/channels/{channel_id}/overrides/{role_id}",
        ManageStructure,
    ),
    ("DELETE", "/api/v1/channels/{channel_id}", ManageStructure),
    (
        "PATCH",
        "/api/v1/channels/{channel_id}/call/settings",
        ManageStructure,
    ),
    (
        "PUT",
        "/api/v1/channels/{channel_id}/overrides/{role_id}",
        ManageStructure,
    ),
    (
        "POST",
        "/api/v1/workspaces/{workspace_id}/roles",
        ManageStructure,
    ),
    (
        "PATCH",
        "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
        ManageStructure,
    ),
    ("PATCH", "/api/v1/server-roles/positions", ManageStructure),
    (
        "PATCH",
        "/api/v1/workspaces/{workspace_id}/roles/positions",
        ManageStructure,
    ),
    ("POST", "/api/v1/server-roles", ManageStructure),
    ("PATCH", "/api/v1/server-roles/{role_id}", ManageStructure),
    ("GET", "/api/v1/accounts", ManageMembers),
    (
        "POST",
        "/api/v1/accounts/{account_id}/removal",
        ManageMembers,
    ),
    ("PUT", "/api/v1/accounts/{account_id}/ban", ManageMembers),
    (
        "PUT",
        "/api/v1/workspaces/{workspace_id}/members/{account_id}",
        ManageMembers,
    ),
    ("GET", "/api/v1/invites", ManageMembers),
    ("POST", "/api/v1/invites", ManageMembers),
    ("DELETE", "/api/v1/invites/{invite_id}", ManageMembers),
    ("GET", "/api/v1/join-requests", ManageMembers),
    (
        "POST",
        "/api/v1/join-requests/{join_request_id}/approval",
        ManageMembers,
    ),
    (
        "POST",
        "/api/v1/join-requests/{join_request_id}/denial",
        ManageMembers,
    ),
    ("GET", "/api/v1/audit-log", ReadAuditLog),
    (
        "GET",
        "/api/v1/workspaces/{workspace_id}/audit-log",
        ReadAuditLog,
    ),
];

/// The action that an access token needs on the route with this method and this path
/// pattern. The server-role list also accepts manage_members (ADR 0309).
/// `None`: no access token reaches the route.
pub fn reach(method: &Method, path: &str) -> Option<TokenAction> {
    REACH
        .iter()
        .find(|(listed_method, listed_path, _)| {
            *listed_method == method.as_str() && *listed_path == path
        })
        .map(|(_, _, action)| *action)
}

/// The refusal of a route that no access token reaches. It is the same for a token that
/// exists and for one that does not.
pub fn outside_reach() -> ApiError {
    ApiError::new(
        ErrorCode::Forbidden,
        "an access token cannot use this route; it needs a session",
    )
}

pub fn action_bits(actions: &[TokenAction]) -> Option<i64> {
    let mut bits = 0i64;
    for action in actions {
        bits |= 1i64 << action.bit()?;
    }
    Some(bits)
}

pub fn stored_actions(bits: i64) -> ApiResult<Vec<TokenAction>> {
    let known = action_bits(TokenAction::ALL).unwrap_or(0);
    if bits <= 0 || bits & !known != 0 {
        return Err(ApiError::internal(&format_args!(
            "unknown access token action bits {bits}"
        )));
    }
    Ok(TokenAction::ALL
        .iter()
        .copied()
        .filter(|action| action.bit().is_some_and(|bit| bits & (1i64 << bit) != 0))
        .collect())
}

pub type TokenRow = (
    AccessTokenId,
    String,
    AccountId,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);

pub const TOKEN_COLUMNS: &str = "id, name, created_by, actions, created_at, expires_at";

pub fn info(row: TokenRow) -> ApiResult<AccessTokenInfo> {
    let (id, name, created_by, actions, created_at, expires_at) = row;
    Ok(AccessTokenInfo {
        id,
        name,
        created_by,
        actions: stored_actions(actions)?,
        created_at,
        expires_at,
    })
}

/// Deletes every access token of an account whose access ends: at a ban, a removal and a
/// recovery (ADR 0210).
pub async fn end_all_of(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<()> {
    let deleted = sqlx::query("delete from access_tokens where created_by = $1")
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
    if deleted.rows_affected() > 0 {
        let revoked = Event::AccessTokensOfCreatorRevoked {
            created_by: account_id,
        };
        events::publish(connection, now, &revoked).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_bits_round_trip_and_unknown_bits_are_refused() {
        let all = action_bits(TokenAction::ALL).unwrap();
        assert_eq!(all, 15);
        assert_eq!(stored_actions(all).unwrap(), TokenAction::ALL);
        assert_eq!(
            stored_actions(action_bits(&[ReadAuditLog, ReadStructure]).unwrap()).unwrap(),
            [ReadStructure, ReadAuditLog]
        );
        assert!(stored_actions(0).is_err());
        assert!(stored_actions(16).is_err());
        assert!(stored_actions(-1).is_err());
        assert_eq!(action_bits(&[TokenAction::Unknown]), None);
    }

    #[test]
    fn a_route_is_reached_only_with_its_method_and_its_whole_path() {
        assert_eq!(
            reach(&Method::GET, "/api/v1/structure"),
            Some(ReadStructure)
        );
        assert_eq!(reach(&Method::POST, "/api/v1/structure"), None);
        assert_eq!(reach(&Method::GET, "/api/v1/structure/"), None);
        assert_eq!(reach(&Method::POST, "/api/v1/access-tokens"), None);
        assert_eq!(reach(&Method::GET, "/api/v1/events"), None);
    }

    #[test]
    fn no_route_is_listed_twice() {
        let mut listed: Vec<(&str, &str)> = REACH
            .iter()
            .map(|(method, path, _)| (*method, *path))
            .collect();
        listed.sort_unstable();
        let before = listed.len();
        listed.dedup();
        assert_eq!(listed.len(), before);
    }
}
