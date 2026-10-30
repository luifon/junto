use axum::{
    Json,
    body::Body,
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use junto_protocol::{
    events::EVENT_REVISION,
    message::{Document, DocumentFormat, MessagePage},
};
use serde_json::Value;

use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    extract::Bearer,
    messages::{compat, page},
    read,
    state::AppState,
};

#[utoipa::path(get,path="/api/v1/message-format",tag="messages",security(("session"=[])),responses((status=200,body=DocumentFormat),ApiError))]
pub async fn supported(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<DocumentFormat>> {
    read!(app, |connection| {
        Access::open(connection, &bearer, app.now()).await?;
        Ok(DocumentFormat {
            event_revision: EVENT_REVISION,
        })
    })
    .map(Json)
}

pub fn revision(headers: &HeaderMap) -> u32 {
    headers
        .get("Junto-Event-Revision")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

pub fn require(document: &Document, revision: u32) -> ApiResult<()> {
    if document.required_revision() > revision {
        return Err(ApiError::invalid(
            "update Junto to write this message format",
        ));
    }
    Ok(())
}

fn documents(value: &Value, revision: u32, found: &mut Vec<Document>) {
    match value {
        Value::Object(fields) => {
            if let Some(value) = fields.get("document")
                && let Ok(document) = serde_json::from_value::<Document>(value.clone())
                && document.required_revision() > revision
            {
                found.push(document);
            }
            for child in fields.values() {
                documents(child, revision, found);
            }
        }
        Value::Array(values) => {
            for child in values {
                documents(child, revision, found);
            }
        }
        _ => {}
    }
}

fn rewrite(
    value: &mut Value,
    revision: u32,
    labels: &compat::Labels,
    ascending: bool,
) -> ApiResult<()> {
    match value {
        Value::Object(fields) => {
            if let Some(value) = fields.get_mut("document")
                && let Ok(document) = serde_json::from_value::<Document>(value.clone())
                && document.required_revision() > revision
            {
                *value = serde_json::to_value(compat::legacy(&document, labels))
                    .map_err(|error| ApiError::internal(&error))?;
            }
            for child in fields.values_mut() {
                rewrite(child, revision, labels, ascending)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                rewrite(child, revision, labels, ascending)?;
            }
        }
        _ => {}
    }
    if value.get("messages").is_some() && value.get("has_more").is_some() {
        let mut result: MessagePage =
            serde_json::from_value(value.clone()).map_err(|error| ApiError::internal(&error))?;
        result.has_more |= page::bound(&mut result.messages, ascending)?;
        *value = serde_json::to_value(result).map_err(|error| ApiError::internal(&error))?;
    }
    Ok(())
}

pub async fn middleware(State(app): State<AppState>, request: Request, next: Next) -> Response {
    match project(app, request, next).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn project(app: AppState, request: Request, next: Next) -> ApiResult<Response> {
    let revision = revision(request.headers());
    if revision >= EVENT_REVISION {
        return Ok(next.run(request).await);
    }
    let ascending = reqwest::Url::parse(&format!("http://metadata.invalid{}", request.uri()))
        .ok()
        .is_some_and(|url| {
            url.query_pairs()
                .any(|(key, _)| matches!(key.as_ref(), "after" | "after_message"))
        });
    let (mut parts, body) = request.into_parts();
    let bearer = Bearer::from_request_parts(&mut parts, &app).await.ok();
    let response = next.run(Request::from_parts(parts, body)).await;
    if !response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"))
    {
        return Ok(response);
    }
    let (mut parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    let mut value: Value =
        serde_json::from_slice(&bytes).map_err(|error| ApiError::internal(&error))?;
    let mut found = Vec::new();
    documents(&value, revision, &mut found);
    if found.is_empty() {
        return Ok(Response::from_parts(parts, Body::from(bytes)));
    }
    let bearer = bearer.ok_or_else(ApiError::unauthenticated)?;
    let labels = read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        compat::labels(connection, &access, &found).await
    })?;
    rewrite(&mut value, revision, &labels, ascending)?;
    parts.headers.remove(header::CONTENT_LENGTH);
    Ok(Response::from_parts(
        parts,
        Body::from(serde_json::to_vec(&value).map_err(|error| ApiError::internal(&error))?),
    ))
}
