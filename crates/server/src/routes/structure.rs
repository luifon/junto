//! The structure of the server in one request (ADR 0204, ADR 0211). The setup tool uses
//! these two routes.

use axum::Json;
use axum::extract::State;
use junto_protocol::structure_file::{
    ApplyStructureChanges, MOST_CHANGES, StructureApplied, StructureExport,
};

use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, Bearer};
use crate::state::AppState;
use crate::structure;
use crate::{read, transact};

/// The workspaces, roles, categories, channels and overrides of the server, with the
/// revision of that structure. The caller needs the administrator permission.
#[utoipa::path(
    get,
    path = "/api/v1/structure",
    tag = "structure",
    security(("session" = [])),
    responses((status = 200, body = StructureExport), ApiError)
)]
pub async fn get_structure(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<StructureExport>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        structure::export(connection, &access).await
    })
    .map(Json)
}

/// Applies changes of the structure: all of them or none. The request names the revision
/// that the changes were computed from; with another current revision the answer is
/// `conflict` and nothing is changed. An error of one change names the number of the change,
/// counted from 1. The caller needs the administrator permission, and each change needs
/// what its own route needs.
#[utoipa::path(
    post,
    path = "/api/v1/structure/changes",
    tag = "structure",
    request_body = ApplyStructureChanges,
    security(("session" = [])),
    responses((status = 200, body = StructureApplied), ApiError)
)]
pub async fn apply_structure_changes(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ApplyStructureChanges>,
) -> ApiResult<Json<StructureApplied>> {
    if body.changes.len() > MOST_CHANGES {
        return Err(ApiError::invalid(format!(
            "a request has at most {MOST_CHANGES} changes"
        )));
    }
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        structure::apply(&app, connection, &access, now, &body).await
    })
    .map(Json)
}
