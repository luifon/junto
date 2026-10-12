//! The two requests of the tool to the public API of a server.

use std::time::Duration;

use junto_protocol::error::{ErrorBody, ErrorCode};
use junto_protocol::origin::{Origin, Scheme};
use junto_protocol::secret::AccessToken;
use junto_protocol::structure_file::{ApplyStructureChanges, StructureApplied, StructureExport};
use serde::Serialize;
use serde::de::DeserializeOwned;

const EXPORT_PATH: &str = "/api/v1/structure";
const CHANGES_PATH: &str = "/api/v1/structure/changes";
/// The server ends a request after 30 seconds (ADR 0008).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// A request that the server answered with `busy` or `timeout` changed nothing and is sent
/// again (ADR 0205).
const ATTEMPTS: u32 = 4;
const PAUSE: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum ApiFailure {
    #[error("{0}")]
    Address(String),
    /// The server answered with an error. Nothing was changed.
    #[error("the server answered {code}: {message}")]
    Refused {
        code: String,
        status: u16,
        message: String,
    },
    /// No answer arrived. For a request that changes the structure, the tool cannot tell
    /// whether the server applied it.
    #[error("the server at {address} gave no answer: {reason}")]
    NoAnswer { address: String, reason: String },
    #[error("the answer of the server is not what this version of the tool reads: {0}")]
    Unreadable(String),
}

pub struct Server {
    origin: Origin,
    token: AccessToken,
    http: reqwest::Client,
}

impl Server {
    /// A server at `address`. The token is sent only over `https`, or over `http` to a
    /// loopback address, where no other machine reads it.
    pub fn new(address: &str, token: AccessToken) -> Result<Self, ApiFailure> {
        let origin = Origin::parse(address.trim().trim_end_matches('/'))
            .map_err(|error| ApiFailure::Address(format!("the address {address:?}: {error}")))?;
        if origin.scheme() == Scheme::Http && !origin.is_loopback() {
            return Err(ApiFailure::Address(format!(
                "the address {origin} uses http; the access token is sent over https only, or \
                 over http to a loopback address"
            )));
        }
        // The API answers no request with a redirect. The tool follows none, so that the
        // token goes to the given address only.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| ApiFailure::Address(format!("no HTTP client: {error}")))?;
        Ok(Self {
            origin,
            token,
            http,
        })
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub async fn structure(&self) -> Result<StructureExport, ApiFailure> {
        self.request::<(), _>(reqwest::Method::GET, EXPORT_PATH, None)
            .await
    }

    pub async fn apply(
        &self,
        changes: &ApplyStructureChanges,
    ) -> Result<StructureApplied, ApiFailure> {
        self.request(reqwest::Method::POST, CHANGES_PATH, Some(changes))
            .await
    }

    async fn request<B: Serialize, T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ApiFailure> {
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| ApiFailure::Unreadable(error.to_string()))?;
        let mut attempt = 1;
        loop {
            let outcome = self.once(method.clone(), path, body.as_deref()).await;
            match outcome {
                Err(ApiFailure::Refused { ref code, .. })
                    if attempt < ATTEMPTS && (code == "busy" || code == "timeout") =>
                {
                    attempt += 1;
                    tokio::time::sleep(PAUSE).await;
                }
                Ok(bytes) => {
                    return serde_json::from_slice(&bytes)
                        .map_err(|error| ApiFailure::Unreadable(error.to_string()));
                }
                Err(failure) => return Err(failure),
            }
        }
    }

    async fn once(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>, ApiFailure> {
        let no_answer = |error: reqwest::Error| ApiFailure::NoAnswer {
            address: self.origin.to_string(),
            reason: error.without_url().to_string(),
        };
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.origin))
            .bearer_auth(self.token.expose());
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_vec());
        }
        let response = request.send().await.map_err(no_answer)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(no_answer)?;
        if status.is_success() {
            return Ok(bytes.to_vec());
        }
        let refused = match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => {
                let code = value["error"]["code"].as_str().map(str::to_owned);
                let message = serde_json::from_value::<ErrorBody>(value)
                    .map(|body| body.error.message)
                    .unwrap_or_default();
                code.map(|code| (code, message))
            }
            Err(_) => None,
        };
        match refused {
            Some((code, message)) => Err(ApiFailure::Refused {
                code,
                status: status.as_u16(),
                message,
            }),
            None => Err(ApiFailure::Unreadable(format!(
                "status {status} without an error body"
            ))),
        }
    }
}

/// True for an answer that tells that the plan no longer fits the server: its revision is
/// not current, or an entry changed. A new plan is needed.
pub fn needs_new_plan(failure: &ApiFailure) -> bool {
    let conflict = serde_json::to_value(ErrorCode::Conflict)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned));
    matches!(failure, ApiFailure::Refused { code, .. } if Some(code) == conflict.as_ref())
}
