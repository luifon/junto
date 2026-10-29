use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Invitation {
    code: String,
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(super) async fn page(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(invite): Query<Invitation>,
) -> Response {
    let code = &invite.code;
    if code.len() > 32
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || code.bytes().filter(|byte| *byte != b'-').count() != 16
    {
        return (StatusCode::BAD_REQUEST, "This invite link is invalid.").into_response();
    }
    let origins = app.config.public_origins.list();
    let host = headers.get("host").and_then(|host| host.to_str().ok());
    let origin = origins
        .iter()
        .find(|origin| {
            host.is_some_and(|host| {
                origin
                    .as_str()
                    .split_once("://")
                    .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host))
            })
        })
        .or_else(|| origins.first());
    let Some(origin) = origin else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("server", origin.as_str())
        .append_pair("code", code)
        .finish();
    let link = escaped(&format!("junto://invite?{query}"));
    let info = match super::server_info::get_server_info(State(app.clone())).await {
        Ok(info) => info.0,
        Err(error) => return error.into_response(),
    };
    let name = escaped(&info.name);
    let address = escaped(origin.as_str());
    let download = match app.config.client_download_url.as_ref() {
        Some(url) => format!("<p><a href=\"{}\" rel=\"noreferrer\">Download Junto</a>, then return here to open the invite. You can also add the server and enter this code in Junto.</p>", escaped(url.as_str())),
        None => "<p>Ask the server administrator for a Junto download. After installing, return here to open the invite, or add the server and enter this code.</p>".to_owned(),
    };
    let body = format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Join {name} on Junto</title><style>body{{font:18px system-ui;line-height:1.6;max-width:38rem;margin:10vh auto;padding:1.5rem;background:#f5f2ec;color:#272b28}}a{{color:#255e48}}code{{overflow-wrap:anywhere}}.open{{display:inline-block;padding:.6rem 1.2rem;background:#255e48;color:white;border-radius:.4rem}}dt{{font-weight:600}}dd{{margin:0 0 1rem}}</style><main><h1>Join {name} on Junto</h1><p>You have an invitation to this server. Opening the invite lets you review the server before joining.</p><p><a class="open" href="{link}">Open Junto</a></p>{download}<dl><dt>Server</dt><dd><code>{address}</code></dd><dt>Invite code</dt><dd><code>{code}</code></dd></dl><p>The server may refuse an expired, revoked or used invite. Keep this link private.</p></main></html>"#
    );
    ([("cache-control", "no-store"), ("referrer-policy", "no-referrer"), ("x-content-type-options", "nosniff"), ("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'")], Html(body)).into_response()
}
