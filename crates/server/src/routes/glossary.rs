//! The glossary of a workspace (ADR 0603): the names and terms that the speech model
//! receives before it transcribes a recording of the workspace.

use std::collections::HashSet;

use axum::Json;
use axum::extract::State;
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::WorkspaceId;
use junto_protocol::transcript::{Glossary, SetGlossary};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::audit::{self, About};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::recordings::{contract, transcripts};
use crate::state::AppState;
use crate::{read, transact};

/// The terms of a new glossary, each with the space at its start and end removed. The
/// glossary route and the structure changes route (ADR 0211) check a glossary with this
/// function.
pub(crate) fn checked_terms(terms: &[String], most: u32) -> ApiResult<Vec<String>> {
    let most = usize::try_from(most).unwrap_or(usize::MAX);
    if terms.len() > most {
        return Err(ApiError::invalid(format!(
            "the glossary holds at most {most} terms"
        )));
    }
    let terms: Vec<String> = terms.iter().map(|term| term.trim().to_owned()).collect();
    if let Some(refused) = terms.iter().position(|term| !contract::is_term(term)) {
        return Err(ApiError::invalid(format!(
            "term {} is empty, longer than 200 characters, or contains a control character, \
             \"<|\" or \"|>\"",
            refused + 1
        )));
    }
    let distinct: HashSet<&str> = terms.iter().map(String::as_str).collect();
    if distinct.len() != terms.len() {
        return Err(ApiError::invalid("a term is in the glossary twice"));
    }
    Ok(terms)
}

/// Replaces the glossary of a workspace with checked terms in the transaction of the caller,
/// with the audit entry and the event. The caller holds `manage_glossary` in the workspace.
/// With `replaces`, the glossary is replaced only while it holds exactly those terms.
pub(crate) async fn replace_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
    terms: &[String],
    replaces: Option<&[String]>,
) -> ApiResult<()> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::ViewWorkspace(&scope))?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageGlossary,
    })?;
    if let Some(read) = replaces {
        let held = transcripts::glossary(connection, workspace_id).await?;
        if read != held.as_slice() {
            return Err(ApiError::conflict(
                "the glossary changed after the terms in \"replaces\" were read",
            ));
        }
    }
    transcripts::replace_glossary(connection, workspace_id, terms).await?;
    let entry = AuditEvent::GlossaryReplaced {
        terms: u32::try_from(terms.len()).unwrap_or(u32::MAX),
    };
    let (actor, about) = (Some(access.account_id), About::Workspace(workspace_id));
    audit::record(connection, now, actor, about, &entry).await?;
    events::publish(connection, now, &Event::GlossaryReplaced { workspace_id }).await
}

/// The glossary of a workspace, most important term first. Every member of the workspace
/// reads it.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/glossary",
    tag = "recordings",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = Glossary), ApiError)
)]
pub async fn get_glossary(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Glossary>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        Ok(Glossary {
            workspace_id,
            terms: transcripts::glossary(connection, workspace_id).await?,
        })
    })
    .map(Json)
}

/// Replaces the glossary of a workspace. The caller holds `manage_glossary`. The speech
/// model reads the first terms that fit in its limit; a transcript states which terms it
/// used (ADR 0613). With `replaces`, the glossary is replaced only while it holds exactly
/// those terms; otherwise the answer is `conflict` and nothing changes (ADR 0607).
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/glossary",
    tag = "recordings",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = SetGlossary,
    security(("session" = [])),
    responses((status = 200, body = Glossary), ApiError)
)]
pub async fn set_glossary(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<SetGlossary>,
) -> ApiResult<Json<Glossary>> {
    let most = app.config.limits.recordings.most_glossary_terms;
    let terms = checked_terms(&body.terms, most)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let replaces = body.replaces.as_deref();
        replace_in(connection, &access, now, workspace_id, &terms, replaces).await?;
        Ok(Glossary {
            workspace_id,
            terms: terms.clone(),
        })
    })
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(terms: &[&str]) -> Vec<String> {
        terms.iter().map(|term| (*term).to_owned()).collect()
    }

    #[test]
    fn terms_are_trimmed_bounded_and_distinct() {
        let terms = checked_terms;
        assert_eq!(
            terms(&set(&[" Quorvex ", "Ysolde Varkonen"]), 200).unwrap(),
            ["Quorvex", "Ysolde Varkonen"]
        );
        assert!(terms(&set(&[]), 200).unwrap().is_empty());
        assert!(terms(&set(&["a", "b", "c"]), 2).is_err());
        assert!(terms(&set(&["a", " a"]), 200).is_err());
        for refused in ["", "   ", "<|notimestamps|>", "a\nb"] {
            assert!(terms(&set(&["fine", refused]), 200).is_err(), "{refused:?}");
        }
        assert!(terms(&set(&[&"a".repeat(201)]), 200).is_err());
    }
}
