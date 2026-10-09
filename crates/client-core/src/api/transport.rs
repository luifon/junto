//! One HTTP exchange with a server. The API client builds requests and reads responses;
//! a transport only carries them, so tests replace it.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use url::Url;

use crate::secret::{SecretBytes, SecretToken};

/// The largest response body that the client reads.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longer than the 30 seconds after which the server ends a request (ADR 0008).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(40);
const LONGEST_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// A request. Its body and its token can hold secrets, so formatting shows neither.
#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub bearer: Option<SecretToken>,
    /// A JSON document.
    pub body: Option<Arc<SecretBytes>>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} {}", self.method, self.url)
    }
}

/// A response. Its body can hold a session token: formatting shows its length only, and
/// the body is overwritten when the response is dropped.
pub struct HttpResponse {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: SecretBytes,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} with {} bytes", self.status, self.body.len())
    }
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportErrorKind {
    /// No connection was made, so the server did not receive the request.
    Connect,
    /// No complete response arrived in time.
    Timeout,
    /// The connection failed after the request was sent, or the response could not be read.
    Interrupted,
    /// The response is larger than the client reads.
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.describe())]
pub struct TransportError {
    pub kind: TransportErrorKind,
    detail: String,
}

impl TransportError {
    pub fn new(kind: TransportErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// False when the server cannot have received the request.
    pub fn request_may_have_arrived(&self) -> bool {
        self.kind != TransportErrorKind::Connect
    }

    fn describe(&self) -> String {
        let summary = match self.kind {
            TransportErrorKind::Connect => "the server cannot be reached",
            TransportErrorKind::Timeout => "the server did not answer in time",
            TransportErrorKind::Interrupted => "the connection to the server failed",
            TransportErrorKind::TooLarge => "the response of the server is too large",
        };
        if self.detail.is_empty() {
            summary.to_owned()
        } else {
            format!("{summary} ({})", self.detail)
        }
    }
}

pub type TransportFuture =
    Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'static>>;

pub trait Transport: Send + Sync + 'static {
    fn send(&self, request: HttpRequest) -> TransportFuture;
}

/// The transport of the product: reqwest with rustls and the certificates that the
/// operating system trusts. It follows no redirect, so a server cannot send a request and
/// its session token to another host.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("junto-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                TransportError::new(TransportErrorKind::Connect, error.without_url().to_string())
            })?;
        Ok(Self { client })
    }
}

impl Transport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let method = match request.method {
                Method::Get => reqwest::Method::GET,
                Method::Post => reqwest::Method::POST,
                Method::Put => reqwest::Method::PUT,
                Method::Patch => reqwest::Method::PATCH,
                Method::Delete => reqwest::Method::DELETE,
            };
            let mut builder = client
                .request(method, request.url)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(token) = &request.bearer {
                builder = builder.bearer_auth(token.expose());
            }
            if let Some(body) = &request.body {
                builder = builder
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(shared_body(body));
            }
            let mut response = builder.send().await.map_err(failure)?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_retry_after);
            let too_large = || {
                TransportError::new(
                    TransportErrorKind::TooLarge,
                    format!("more than {MAX_RESPONSE_BYTES} bytes"),
                )
            };
            if response
                .content_length()
                .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
            {
                return Err(too_large());
            }
            let expected = response.content_length().unwrap_or(0) as usize;
            let mut body = SecretBytes::with_capacity(expected.min(MAX_RESPONSE_BYTES));
            while let Some(chunk) = response.chunk().await.map_err(failure)? {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(too_large());
                }
                body.extend(&chunk);
            }
            Ok(HttpResponse {
                status,
                retry_after,
                body,
            })
        })
    }
}

/// The body of a request as the HTTP library takes it: the buffer of the request itself,
/// not a copy. The buffer stays with this crate and is overwritten when the request and
/// the HTTP library have both let go of it.
fn shared_body(body: &Arc<SecretBytes>) -> bytes::Bytes {
    struct Shared(Arc<SecretBytes>);

    impl AsRef<[u8]> for Shared {
        fn as_ref(&self) -> &[u8] {
            self.0.as_slice()
        }
    }

    bytes::Bytes::from_owner(Shared(body.clone()))
}

fn failure(error: reqwest::Error) -> TransportError {
    let kind = if error.is_connect() {
        TransportErrorKind::Connect
    } else if error.is_timeout() {
        TransportErrorKind::Timeout
    } else {
        TransportErrorKind::Interrupted
    };
    let mut detail = String::new();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        detail = cause.to_string();
        source = cause.source();
    }
    TransportError::new(kind, detail)
}

/// The `Retry-After` header as whole seconds. The server sends no date form.
pub(crate) fn parse_retry_after(text: &str) -> Option<Duration> {
    let seconds: u64 = text.trim().parse().ok()?;
    Some(Duration::from_secs(seconds).min(LONGEST_RETRY_AFTER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_a_request_or_a_response_shows_no_secret() {
        let request = HttpRequest {
            method: Method::Post,
            url: Url::parse("https://chat.example.test/api/v1/join").unwrap(),
            bearer: Some(SecretToken::new("jst_secret-token".to_owned())),
            body: Some(Arc::new(br#"{"invite_code":"ABCD-EFGH"}"#.to_vec().into())),
        };
        let shown = format!("{request:?}");
        assert_eq!(shown, "Post https://chat.example.test/api/v1/join");
        let response = HttpResponse {
            status: 201,
            retry_after: None,
            body: br#"{"token":"jst_secret-token"}"#.to_vec().into(),
        };
        assert_eq!(format!("{response:?}"), "201 with 28 bytes");
        assert!(response.is_success());
    }

    #[test]
    fn the_http_library_gets_the_buffer_of_the_request_and_no_copy() {
        let body: Arc<SecretBytes> = Arc::new(br#"{"owner_code":"ABCD"}"#.to_vec().into());
        let given = shared_body(&body);
        assert_eq!(given.as_ptr(), body.as_slice().as_ptr());
        assert_eq!(&given[..], body.as_slice());
        assert_eq!(Arc::strong_count(&body), 2);
        drop(given);
        assert_eq!(Arc::strong_count(&body), 1);
    }

    #[test]
    fn retry_after_is_whole_seconds_with_an_upper_bound() {
        assert_eq!(parse_retry_after("1"), Some(Duration::from_secs(1)));
        assert_eq!(parse_retry_after(" 600 "), Some(Duration::from_secs(600)));
        assert_eq!(
            parse_retry_after("99999999999"),
            Some(Duration::from_secs(86_400))
        );
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("soon"), None);
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[test]
    fn only_a_failed_connection_proves_that_the_request_did_not_arrive() {
        let arrived = |kind| TransportError::new(kind, "").request_may_have_arrived();
        assert!(!arrived(TransportErrorKind::Connect));
        assert!(arrived(TransportErrorKind::Timeout));
        assert!(arrived(TransportErrorKind::Interrupted));
        assert!(arrived(TransportErrorKind::TooLarge));
    }
}
