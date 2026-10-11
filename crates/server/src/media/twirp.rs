//! One request to the API of the LiveKit server: Twirp over HTTP with a JSON body, authorized
//! with a token that the server signs for it (ADR 0508). The room service and the recorder
//! use it.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::clock::Clock;
use crate::config::MediaSettings;
use crate::media::token::{self, ApiGrant};

pub struct Twirp {
    http: reqwest::Client,
    settings: MediaSettings,
    clock: Arc<dyn Clock>,
}

pub enum Answer {
    Body(Value),
    /// The API states that the object of the request does not exist.
    NotFound,
}

/// Why a request has no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The token of the request could not be signed.
    Unsigned,
    /// No answer arrived: the connection failed, or the time limit of the request passed.
    NoAnswer(String),
    /// The API answered with an error status and this Twirp code.
    Status { status: u16, code: String },
    /// The answer is not the JSON that the API states.
    Malformed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsigned => formatter.write_str("the API token could not be signed"),
            Self::NoAnswer(reason) | Self::Malformed(reason) => formatter.write_str(reason),
            Self::Status { status, code } => write!(formatter, "{status} {code}"),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct TwirpError {
    #[serde(default)]
    code: String,
}

impl Twirp {
    pub fn new(settings: MediaSettings, clock: Arc<dyn Clock>) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(settings.request_timeout.unsigned_abs())
            .connect_timeout(settings.request_timeout.unsigned_abs())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            http,
            settings,
            clock,
        })
    }

    pub fn settings(&self) -> &MediaSettings {
        &self.settings
    }

    pub fn secret(&self) -> &[u8] {
        self.settings.api_secret.expose().as_bytes()
    }

    /// Sends one request. `time_limit` replaces the time limit of the media settings for a
    /// request that the API answers later than that.
    pub async fn call(
        &self,
        service: &str,
        method: &str,
        grant: ApiGrant<'_>,
        body: Value,
        time_limit: Option<std::time::Duration>,
    ) -> Result<Answer, Failure> {
        let claims = token::api_claims(&self.settings.api_key, grant, self.clock.now());
        let authorization = token::sign(self.secret(), &claims).ok_or(Failure::Unsigned)?;
        let url = format!(
            "{}/twirp/{service}/{method}",
            self.settings.api_url.trim_end_matches('/')
        );
        let mut request = self.http.post(url).bearer_auth(authorization).json(&body);
        if let Some(time_limit) = time_limit {
            request = request.timeout(time_limit);
        }
        let no_answer = |error: reqwest::Error| Failure::NoAnswer(error.without_url().to_string());
        let response = request.send().await.map_err(no_answer)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(no_answer)?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map(Answer::Body)
                .map_err(|error| Failure::Malformed(format!("{method}: {error}")));
        }
        let code = serde_json::from_slice::<TwirpError>(&bytes)
            .map(|error| error.code)
            .unwrap_or_default();
        if code == "not_found" {
            Ok(Answer::NotFound)
        } else {
            Err(Failure::Status {
                status: status.as_u16(),
                code,
            })
        }
    }
}
