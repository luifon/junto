//! One HTTP exchange with a server. The API client builds requests and reads responses;
//! a transport only carries them, so tests replace it.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use junto_protocol::account::ImageType;
use junto_protocol::events::{Cursor, EVENT_CURSOR_HEADER};
use url::Url;

use crate::secret::{SecretBytes, SecretToken};

/// The largest response body that the client reads.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longer than the 30 seconds after which the server ends a request (ADR 0008).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(40);
/// The time for a request that carries one part of a file: 1 MiB on a slow connection.
const PART_TIMEOUT: Duration = Duration::from_secs(600);
/// A download ends when no byte of it arrives for this long.
const READ_TIMEOUT: Duration = Duration::from_secs(40);
const LONGEST_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// The body of a request: a JSON document, or the image of an avatar (ADR 0119). A request
/// that is sent again shares the buffer.
#[derive(Clone)]
pub struct RequestBody {
    pub media_type: &'static str,
    pub bytes: Arc<SecretBytes>,
}

const JSON: &str = "application/json";
const BYTES: &str = "application/octet-stream";
/// What the client reads: JSON, and the image of an avatar.
const ACCEPTED: &str = "application/json, image/png, image/jpeg, image/webp";
/// What the client reads of a download: the file, or the error of the API.
const ACCEPTED_FILE: &str = "application/octet-stream, application/json";

impl RequestBody {
    pub fn json(bytes: SecretBytes) -> Self {
        Self {
            media_type: JSON,
            bytes: Arc::new(bytes),
        }
    }

    pub fn image(kind: ImageType, bytes: SecretBytes) -> Self {
        Self {
            media_type: kind.media_type(),
            bytes: Arc::new(bytes),
        }
    }

    /// A part of a file (ADR 0408).
    pub fn bytes(bytes: SecretBytes) -> Self {
        Self {
            media_type: BYTES,
            bytes: Arc::new(bytes),
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

/// A request. Its body and its token can hold secrets, so formatting shows neither.
#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub bearer: Option<SecretToken>,
    pub body: Option<RequestBody>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} {}", self.method, self.url)
    }
}

/// What the header `Junto-Event-Cursor` of an answer states about the events of a change
/// (ADR 0214).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Receipt {
    /// The answer has no such header, or one that cannot be read: the position of the
    /// events of the change is not stated.
    #[default]
    NotStated,
    /// The change made no event that the account receives.
    Nothing,
    /// The last event of the change that the account receives.
    At(Cursor),
}

impl Receipt {
    /// The receipt of a header value.
    pub fn of_header(value: Option<&[u8]>) -> Self {
        match value.map(serde_json::from_slice::<Option<Cursor>>) {
            Some(Ok(Some(cursor))) => Self::At(cursor),
            Some(Ok(None)) => Self::Nothing,
            Some(Err(_)) | None => Self::NotStated,
        }
    }
}

/// A response. Its body can hold a session token: formatting shows its length only, and
/// the body is overwritten when the response is dropped.
pub struct HttpResponse {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: SecretBytes,
    pub receipt: Receipt,
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

/// The body of a response in the parts in which it arrives.
pub type BodyStream =
    Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, TransportError>> + Send>>;

/// A response whose body is read in parts: a file of any size.
pub struct OpenedResponse {
    pub status: u16,
    pub retry_after: Option<Duration>,
    /// The first byte of the range that the answer states, when it states one.
    pub range_start: Option<u64>,
    pub body: BodyStream,
}

pub type OpenFuture =
    Pin<Box<dyn Future<Output = Result<OpenedResponse, TransportError>> + Send + 'static>>;

pub trait Transport: Send + Sync + 'static {
    fn send(&self, request: HttpRequest) -> TransportFuture;

    /// Sends a request whose body is one part of a file. The time limit allows a slow
    /// connection.
    fn send_part(&self, request: HttpRequest) -> TransportFuture {
        self.send(request)
    }

    /// Asks for a file from the byte `from` on, and returns the body as it arrives. The
    /// status 206 states that the body starts at `from`; with 200 it is the whole file.
    fn open(&self, request: HttpRequest, from: u64) -> OpenFuture {
        let _ = from;
        let sent = self.send(request);
        Box::pin(async move {
            let response = sent.await?;
            let body = bytes::Bytes::copy_from_slice(response.body.as_slice());
            Ok(OpenedResponse {
                status: response.status,
                retry_after: response.retry_after,
                range_start: None,
                body: Box::pin(futures_util::stream::once(async move { Ok(body) })),
            })
        })
    }
}

/// The transport of the product: reqwest with rustls and the certificates that the
/// operating system trusts. It follows no redirect, so a server cannot send a request and
/// its session token to another host.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    /// The client for the parts of files: the same settings, without a time limit for a
    /// whole request, and with one for each read (ADR 0721).
    files: reqwest::Client,
}

/// The settings of every client object of the HTTP library: no redirect, rustls with the
/// certificates that the operating system trusts, and the time for a connection attempt
/// (ADR 0714, ADR 0718).
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("junto-client/", env!("CARGO_PKG_VERSION")))
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        let built = |client: reqwest::Result<reqwest::Client>| {
            client.map_err(|error| {
                TransportError::new(TransportErrorKind::Connect, error.without_url().to_string())
            })
        };
        Ok(Self {
            client: built(client_builder().timeout(REQUEST_TIMEOUT).build())?,
            files: built(client_builder().read_timeout(READ_TIMEOUT).build())?,
        })
    }
}

fn build(
    client: &reqwest::Client,
    request: HttpRequest,
    accepted: &str,
) -> reqwest::RequestBuilder {
    let method = match request.method {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
    };
    let mut builder = client
        .request(method, request.url)
        .header(reqwest::header::ACCEPT, accepted)
        .header("Junto-Notification-Preferences", "1");
    if let Some(token) = &request.bearer {
        builder = builder.bearer_auth(token.expose());
    }
    if let Some(body) = &request.body {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, body.media_type)
            .body(shared_body(&body.bytes));
    }
    builder
}

impl Transport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let builder = build(&client, request, ACCEPTED);
            read(builder.send().await.map_err(failure)?).await
        })
    }

    fn send_part(&self, request: HttpRequest) -> TransportFuture {
        let client = self.files.clone();
        Box::pin(async move {
            let builder = build(&client, request, ACCEPTED).timeout(PART_TIMEOUT);
            read(builder.send().await.map_err(failure)?).await
        })
    }

    fn open(&self, request: HttpRequest, from: u64) -> OpenFuture {
        let client = self.files.clone();
        Box::pin(async move {
            let mut builder = build(&client, request, ACCEPTED_FILE);
            if from > 0 {
                builder = builder.header(reqwest::header::RANGE, format!("bytes={from}-"));
            }
            let response = builder.send().await.map_err(failure)?;
            let status = response.status().as_u16();
            let retry_after = retry_after_of(&response);
            let range_start = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(range_start_of);
            let body = futures_util::stream::unfold(Some(response), |response| async move {
                let mut response = response?;
                match response.chunk().await {
                    Ok(Some(chunk)) => Some((Ok(chunk), Some(response))),
                    Ok(None) => None,
                    Err(error) => Some((Err(failure(error)), None)),
                }
            });
            Ok(OpenedResponse {
                status,
                retry_after,
                range_start,
                body: Box::pin(body),
            })
        })
    }
}

/// The first byte of a `Content-Range` header, as in `bytes 100-199/200`.
pub(crate) fn range_start_of(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes")?.trim_start();
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
}

fn retry_after_of(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
}

/// Reads a response up to the largest body that the client accepts.
pub(crate) async fn read(mut response: reqwest::Response) -> Result<HttpResponse, TransportError> {
    let status = response.status().as_u16();
    let retry_after = retry_after_of(&response);
    let receipt = Receipt::of_header(
        response
            .headers()
            .get(EVENT_CURSOR_HEADER)
            .map(reqwest::header::HeaderValue::as_bytes),
    );
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
        receipt,
    })
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

pub(crate) fn failure(error: reqwest::Error) -> TransportError {
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
            body: Some(RequestBody::json(
                br#"{"invite_code":"ABCD-EFGH"}"#.to_vec().into(),
            )),
        };
        let shown = format!("{request:?}");
        assert_eq!(shown, "Post https://chat.example.test/api/v1/join");
        let response = HttpResponse {
            status: 201,
            retry_after: None,
            body: br#"{"token":"jst_secret-token"}"#.to_vec().into(),
            receipt: Receipt::NotStated,
        };
        assert_eq!(format!("{response:?}"), "201 with 28 bytes");
        assert!(response.is_success());
    }

    #[test]
    fn a_body_states_what_it_is() {
        let json = RequestBody::json(b"{}".to_vec().into());
        assert_eq!(json.media_type, "application/json");
        assert_eq!(json.as_slice(), b"{}");
        for kind in ImageType::ALL {
            let image = RequestBody::image(*kind, b"image".to_vec().into());
            assert_eq!(image.media_type, kind.media_type());
            assert!(ACCEPTED.contains(kind.media_type()));
        }
        let again = json.clone();
        assert_eq!(again.as_slice().as_ptr(), json.as_slice().as_ptr());
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
    fn the_receipt_header_states_a_cursor_no_event_or_nothing() {
        let epoch = "0b3c6a52-6ad1-4a4f-9b55-0f0a7f8c2d11";
        let cursor = format!(r#"{{"epoch":"{epoch}","position":123}}"#);
        match Receipt::of_header(Some(cursor.as_bytes())) {
            Receipt::At(read) => {
                assert_eq!(read.position, 123);
                assert_eq!(read.epoch.to_string(), epoch);
            }
            other => panic!("read {other:?}"),
        }
        assert_eq!(Receipt::of_header(Some(b"null")), Receipt::Nothing);
        assert_eq!(Receipt::of_header(None), Receipt::NotStated);
        assert_eq!(
            Receipt::of_header(Some(b"{\"position\":1}")),
            Receipt::NotStated
        );
        assert_eq!(Receipt::of_header(Some(b"soon")), Receipt::NotStated);
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
