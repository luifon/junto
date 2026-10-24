//! One connection of the event stream (ADR 0116, ADR 0206). The stream client reads
//! frames and writes `subscribe`; a transport only carries them, so tests replace it.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message};
use url::Url;

use crate::api::transport::{client_builder, failure, read};
use crate::api::{HttpResponse, TransportError, TransportErrorKind};
use crate::secret::SecretToken;

/// The largest message of a server that the client reads (ADR 0116).
pub const MAX_MESSAGE_BYTES: usize = junto_protocol::events::LARGEST_SERVER_MESSAGE_BYTES;

/// How many frames wait between the connection and the stream client. A full queue stops
/// the reading of the connection.
const WAITING_FRAMES: usize = 64;
/// How long the connection waits for the server to take a message of the client.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The upgrade request of one connection. Formatting shows no token.
#[derive(Clone)]
pub struct StreamRequest {
    pub url: Url,
    pub bearer: SecretToken,
}

impl fmt::Debug for StreamRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "event stream {}", self.url)
    }
}

/// What arrives on a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// One text frame: a message of the server.
    Text(String),
    /// A frame without a message arrived: the ping of the server.
    Alive,
    /// The server closed the connection, with the code of its close frame.
    Closed { code: Option<u16> },
    /// The server sent a message that is larger than the client reads. The connection has
    /// ended.
    TooLarge,
    /// The connection failed, or the server sent what a client does not read.
    Failed(String),
}

/// An open connection. Dropping both ends closes it.
pub struct StreamSocket {
    pub incoming: mpsc::Receiver<Incoming>,
    pub outgoing: mpsc::Sender<String>,
}

pub enum OpenError {
    /// The server answered the upgrade request with an error response (ADR 0116).
    Refused(HttpResponse),
    Failed(TransportError),
}

impl fmt::Debug for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(response) => write!(formatter, "refused with {response:?}"),
            Self::Failed(error) => write!(formatter, "{error}"),
        }
    }
}

pub type OpenFuture =
    Pin<Box<dyn Future<Output = Result<StreamSocket, OpenError>> + Send + 'static>>;

pub trait StreamTransport: Send + Sync + 'static {
    fn open(&self, request: StreamRequest) -> OpenFuture;
}

/// The transport of the product. The HTTP library of the API client makes the upgrade
/// request, so the connection has the same TLS, the same trusted certificates and the
/// same refusal of redirects as every other request (ADR 0714). After the upgrade the
/// WebSocket library reads and writes frames on that connection.
#[derive(Debug, Clone)]
pub struct ReqwestStreamTransport {
    client: reqwest::Client,
}

impl ReqwestStreamTransport {
    /// The client has no time limit for a whole request: a connection lasts for hours. The
    /// stream client limits the time until the upgrade and the time without a frame.
    pub fn new() -> Result<Self, TransportError> {
        Self::from_settings(client_builder())
    }

    /// The tests add a certificate of their own to the settings of the product.
    fn from_settings(settings: reqwest::ClientBuilder) -> Result<Self, TransportError> {
        let client = settings.http1_only().build().map_err(|error| {
            TransportError::new(TransportErrorKind::Connect, error.without_url().to_string())
        })?;
        Ok(Self { client })
    }
}

fn invalid(detail: &str) -> OpenError {
    OpenError::Failed(TransportError::new(TransportErrorKind::Interrupted, detail))
}

impl StreamTransport for ReqwestStreamTransport {
    fn open(&self, request: StreamRequest) -> OpenFuture {
        use reqwest::header::{
            ACCEPT, CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION,
            UPGRADE,
        };
        let client = self.client.clone();
        Box::pin(async move {
            let key = generate_key();
            let response = client
                .get(request.url)
                .bearer_auth(request.bearer.expose())
                .header(ACCEPT, "application/json")
                .header(CONNECTION, "upgrade")
                .header(UPGRADE, "websocket")
                .header(SEC_WEBSOCKET_VERSION, "13")
                .header(SEC_WEBSOCKET_KEY, &key)
                .send()
                .await
                .map_err(|error| OpenError::Failed(failure(error)))?;
            let status = response.status().as_u16();
            if status >= 400 {
                return Err(match read(response).await {
                    Ok(response) => OpenError::Refused(response),
                    Err(error) => OpenError::Failed(error),
                });
            }
            let value = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
            };
            // The protocol name is compared without case, the accept value, which is base64,
            // exactly (RFC 6455, section 4.1).
            let websocket =
                value(UPGRADE).is_some_and(|name| name.eq_ignore_ascii_case("websocket"));
            if status != 101 || !websocket {
                return Err(invalid("the server did not upgrade the connection"));
            }
            if value(SEC_WEBSOCKET_ACCEPT) != Some(derive_accept_key(key.as_bytes()).as_str()) {
                return Err(invalid("the answer does not belong to the upgrade request"));
            }
            let upgraded = response
                .upgrade()
                .await
                .map_err(|error| OpenError::Failed(failure(error)))?;
            Ok(carry(upgraded).await)
        })
    }
}

/// Reads and writes the frames of a connection that was upgraded, in a task of its own.
async fn carry<S>(connection: S) -> StreamSocket
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let socket = WebSocketStream::from_raw_socket(connection, Role::Client, Some(config)).await;
    let (arrived, incoming) = mpsc::channel(WAITING_FRAMES);
    let (outgoing, mut to_send) = mpsc::channel::<String>(4);
    tokio::spawn(async move {
        let (mut sink, mut source) = socket.split();
        loop {
            tokio::select! {
                // The client let go of the connection.
                () = arrived.closed() => break,
                frame = source.next() => {
                    let incoming = match frame {
                        Some(Ok(Message::Text(text))) => Incoming::Text(text.as_str().to_owned()),
                        // The library answers a ping with its next read (ADR 0116).
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => Incoming::Alive,
                        Some(Ok(Message::Close(frame))) => Incoming::Closed {
                            code: frame.map(|frame| frame.code.into()),
                        },
                        Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                            Incoming::Failed("the server sent a frame that is not text".to_owned())
                        }
                        Some(Err(tungstenite::Error::Capacity(_))) => Incoming::TooLarge,
                        Some(Err(error)) => Incoming::Failed(error.to_string()),
                        None => Incoming::Closed { code: None },
                    };
                    let goes_on = matches!(incoming, Incoming::Text(_) | Incoming::Alive);
                    if arrived.send(incoming).await.is_err() || !goes_on {
                        break;
                    }
                }
                text = to_send.recv() => {
                    let Some(text) = text else {
                        break;
                    };
                    let sent = tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::text(text)));
                    let sent = tokio::select! {
                        () = arrived.closed() => break,
                        sent = sent => sent,
                    };
                    if !matches!(sent, Ok(Ok(()))) {
                        let failed = Incoming::Failed("a message was not sent".to_owned());
                        let _ = arrived.send(failed).await;
                        break;
                    }
                }
            }
        }
        // The connection ends here without a closing handshake: the server releases the
        // place of a connection when its socket ends (ADR 0116), and a handshake with a
        // server that does not read would hold the place for its whole time limit.
    });
    StreamSocket { incoming, outgoing }
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    use super::*;

    async fn pair() -> (StreamSocket, WebSocketStream<tokio::io::DuplexStream>) {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let server = WebSocketStream::from_raw_socket(far, Role::Server, None).await;
        (carry(near).await, server)
    }

    #[test]
    fn formatting_a_request_shows_no_token() {
        let request = StreamRequest {
            url: Url::parse("https://chat.example.test/api/v1/events").unwrap(),
            bearer: SecretToken::new("jst_secret-token".to_owned()),
        };
        assert_eq!(
            format!("{request:?}"),
            "event stream https://chat.example.test/api/v1/events"
        );
    }

    #[tokio::test]
    async fn text_frames_arrive_in_order_and_a_ping_is_answered() {
        let (mut socket, mut server) = pair().await;
        server.send(Message::text("one")).await.unwrap();
        server
            .send(Message::Ping(b"p".to_vec().into()))
            .await
            .unwrap();
        server.send(Message::text("two")).await.unwrap();
        assert_eq!(
            socket.incoming.recv().await,
            Some(Incoming::Text("one".to_owned()))
        );
        assert_eq!(socket.incoming.recv().await, Some(Incoming::Alive));
        assert_eq!(
            socket.incoming.recv().await,
            Some(Incoming::Text("two".to_owned()))
        );
        assert_eq!(
            server.next().await.unwrap().unwrap(),
            Message::Pong(b"p".to_vec().into())
        );

        socket.outgoing.send("subscribe".to_owned()).await.unwrap();
        assert_eq!(
            server.next().await.unwrap().unwrap(),
            Message::text("subscribe")
        );
    }

    #[tokio::test]
    async fn a_close_frame_gives_its_code_and_ends_the_connection() {
        let (mut socket, mut server) = pair().await;
        let frame = CloseFrame {
            code: CloseCode::from(4003),
            reason: "slow".into(),
        };
        server.send(Message::Close(Some(frame))).await.unwrap();
        assert_eq!(
            socket.incoming.recv().await,
            Some(Incoming::Closed { code: Some(4003) })
        );
        assert_eq!(socket.incoming.recv().await, None);
    }

    #[tokio::test]
    async fn a_binary_frame_and_a_frame_above_the_limit_end_the_connection() {
        let (mut socket, mut server) = pair().await;
        server
            .send(Message::Binary(vec![1, 2].into()))
            .await
            .unwrap();
        assert!(matches!(
            socket.incoming.recv().await,
            Some(Incoming::Failed(_))
        ));
        assert_eq!(socket.incoming.recv().await, None);

        let (mut socket, mut server) = pair().await;
        let largest = "x".repeat(MAX_MESSAGE_BYTES);
        let too_large = format!("{largest}x");
        let sending = tokio::spawn(async move {
            let _ = server.send(Message::text(largest)).await;
            let _ = server.send(Message::text(too_large)).await;
            server
        });
        assert!(matches!(
            socket.incoming.recv().await,
            Some(Incoming::Text(text)) if text.len() == MAX_MESSAGE_BYTES
        ));
        assert_eq!(socket.incoming.recv().await, Some(Incoming::TooLarge));
        assert_eq!(socket.incoming.recv().await, None);
        drop(sending);
    }

    /// A connection that tells when the client has let go of it.
    struct Tracked {
        connection: tokio::io::DuplexStream,
        _let_go: tokio::sync::oneshot::Sender<()>,
    }

    impl AsyncRead for Tracked {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.connection).poll_read(context, buffer)
        }
    }

    impl AsyncWrite for Tracked {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            Pin::new(&mut self.connection).poll_write(context, bytes)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.connection).poll_flush(context)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.connection).poll_shutdown(context)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_socket_that_is_let_go_ends_its_connection_at_once_also_during_a_write() {
        // The server takes nothing, so the message of the client does not leave.
        let (near, _far) = tokio::io::duplex(64);
        let (sender, let_go) = tokio::sync::oneshot::channel();
        let socket = carry(Tracked {
            connection: near,
            _let_go: sender,
        })
        .await;
        socket.outgoing.send("s".repeat(4096)).await.unwrap();
        tokio::task::yield_now().await;
        let dropped = tokio::time::Instant::now();
        drop(socket);
        let _ = let_go.await;
        assert_eq!(dropped.elapsed(), Duration::ZERO);
    }

    const TOKEN: &str = "jst_stream-token";
    const AUTHORITY: &[u8] = include_bytes!("../../tests/data/stream-tls/authority.der");
    const SERVER_CERTIFICATE: &[u8] = include_bytes!("../../tests/data/stream-tls/server.der");
    const SERVER_KEY: &[u8] = include_bytes!("../../tests/data/stream-tls/server-key.der");

    /// The path of the stream with a value that is new in each call, so that a test server can
    /// tell the request of its own client from a connection of another program on the machine.
    fn marked_path() -> String {
        format!("/api/v1/events?test={}", uuid::Uuid::new_v4().simple())
    }

    fn request_to(address: std::net::SocketAddr, scheme: &str, path: &str) -> StreamRequest {
        StreamRequest {
            url: Url::parse(&format!("{scheme}://{address}{path}")).unwrap(),
            bearer: SecretToken::new(TOKEN.to_owned()),
        }
    }

    /// The headers of one request, with the names in lower case.
    type Headers = Vec<(String, String)>;

    /// The request line and the headers of one request, or `None` when the connection ends or
    /// sends something else first.
    async fn read_head<S: AsyncRead + Unpin>(connection: &mut S) -> Option<(String, Headers)> {
        use tokio::io::AsyncReadExt;
        const LARGEST: usize = 16 * 1024;
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() == LARGEST {
                return None;
            }
            head.push(connection.read_u8().await.ok()?);
        }
        let head = String::from_utf8(head).ok()?;
        let mut lines = head.split("\r\n").filter(|line| !line.is_empty());
        let request_line = lines.next()?.to_owned();
        let headers = lines
            .map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect::<Option<_>>()?;
        Some((request_line, headers))
    }

    /// Accepts connections on `listener` until one carries the request `GET <path>`, and gives
    /// that connection, made by `open`, with the headers of its request. Any program on the
    /// machine can connect to the port of a listener; the other connections are closed. Each
    /// connection is read in a task of its own, so one that sends nothing holds up no other.
    async fn accept_request<S, F>(
        listener: tokio::net::TcpListener,
        path: &str,
        open: impl Fn(tokio::net::TcpStream) -> F,
    ) -> (S, Headers)
    where
        S: AsyncRead + Unpin + Send + 'static,
        F: Future<Output = Option<S>> + Send + 'static,
    {
        let expected = format!("GET {path} HTTP/1.1");
        let (found, mut arrived) = mpsc::channel(1);
        loop {
            tokio::select! {
                Some(request) = arrived.recv() => return request,
                accepted = listener.accept() => {
                    let opening = open(accepted.unwrap().0);
                    let (found, expected) = (found.clone(), expected.clone());
                    tokio::spawn(async move {
                        let Some(mut connection) = opening.await else {
                            return;
                        };
                        if let Some((line, headers)) = read_head(&mut connection).await
                            && line == expected
                        {
                            let _ = found.send((connection, headers)).await;
                        }
                    });
                }
            }
        }
    }

    async fn plain(connection: tokio::net::TcpStream) -> Option<tokio::net::TcpStream> {
        Some(connection)
    }

    fn values<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
        headers
            .iter()
            .filter(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn upgraded(accept: &str) -> String {
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        )
    }

    /// Checks the headers of the upgrade request, upgrades the connection, sends `hello` and
    /// gives the first message of the client.
    async fn serve_upgrade<S>(mut connection: S, headers: Headers) -> Message
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        use tokio::io::AsyncWriteExt;
        let bearer = format!("Bearer {TOKEN}");
        assert_eq!(values(&headers, "authorization"), [bearer.as_str()]);
        assert_eq!(values(&headers, "upgrade"), ["websocket"]);
        assert_eq!(values(&headers, "sec-websocket-version"), ["13"]);
        let key = values(&headers, "sec-websocket-key")[0];
        let answer = upgraded(&derive_accept_key(key.as_bytes()));
        connection.write_all(answer.as_bytes()).await.unwrap();
        let mut socket = WebSocketStream::from_raw_socket(connection, Role::Server, None).await;
        socket.send(Message::text("hello")).await.unwrap();
        socket.next().await.unwrap().unwrap()
    }

    async fn says_hello_and_takes_subscribe(
        opened: Result<StreamSocket, OpenError>,
        server: tokio::task::JoinHandle<Message>,
    ) {
        let mut socket = opened.unwrap();
        assert_eq!(
            socket.incoming.recv().await,
            Some(Incoming::Text("hello".to_owned()))
        );
        socket.outgoing.send("subscribe".to_owned()).await.unwrap();
        assert_eq!(server.await.unwrap(), Message::text("subscribe"));
    }

    #[tokio::test]
    async fn the_upgrade_request_is_http_1_1_with_the_session_token_and_frames_follow() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let path = marked_path();
        let request = request_to(listener.local_addr().unwrap(), "http", &path);
        let server = tokio::spawn(async move {
            let (connection, headers) = accept_request(listener, &path, plain).await;
            serve_upgrade(connection, headers).await
        });
        let opened = ReqwestStreamTransport::new().unwrap().open(request).await;
        says_hello_and_takes_subscribe(opened, server).await;
    }

    /// Connections of other programs reach the listener of a test before its client: one that
    /// sends the start of a PostgreSQL connection and waits, and one with a request for another
    /// path. The test server closes them and answers its client.
    #[tokio::test]
    async fn a_connection_of_another_program_does_not_take_the_place_of_the_client() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let path = marked_path();
        let request = request_to(address, "http", &path);
        let mut waiting = tokio::net::TcpStream::connect(address).await.unwrap();
        waiting
            .write_all(b"\x00\x00\x00\x08\x04\xd2\x16\x2f")
            .await
            .unwrap();
        let mut elsewhere = tokio::net::TcpStream::connect(address).await.unwrap();
        elsewhere
            .write_all(b"GET /api/v1/events HTTP/1.1\r\nHost: other\r\n\r\n")
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            let (connection, headers) = accept_request(listener, &path, plain).await;
            serve_upgrade(connection, headers).await
        });
        let opened = ReqwestStreamTransport::new().unwrap().open(request).await;
        says_hello_and_takes_subscribe(opened, server).await;
        drop((waiting, elsewhere));
    }

    /// Opens a connection to a server that answers the upgrade request with what `answer`
    /// makes of the key of the request.
    async fn answered(
        answer: impl FnOnce(&str) -> String + Send + 'static,
    ) -> Result<StreamSocket, OpenError> {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let path = marked_path();
        let request = request_to(listener.local_addr().unwrap(), "http", &path);
        let server = tokio::spawn(async move {
            let (mut connection, headers) = accept_request(listener, &path, plain).await;
            let answer = answer(values(&headers, "sec-websocket-key")[0]);
            connection.write_all(answer.as_bytes()).await.unwrap();
            connection
        });
        let opened = ReqwestStreamTransport::new().unwrap().open(request).await;
        drop(server.await.unwrap());
        opened
    }

    fn refused_with(opened: Result<StreamSocket, OpenError>, detail: &str) -> bool {
        matches!(
            opened,
            Err(OpenError::Failed(error))
                if error == TransportError::new(TransportErrorKind::Interrupted, detail)
        )
    }

    #[tokio::test]
    async fn an_answer_that_is_not_the_upgrade_of_this_request_opens_no_connection() {
        const NOT_UPGRADED: &str = "the server did not upgrade the connection";
        const OTHER_KEY: &str = "the answer does not belong to the upgrade request";
        let accept = |key: &str| derive_accept_key(key.as_bytes());

        let ok = answered(|_| "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_owned()).await;
        assert!(refused_with(ok, NOT_UPGRADED));
        let without_upgrade = answered(move |key| {
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\r\n",
                accept(key)
            )
        })
        .await;
        assert!(refused_with(without_upgrade, NOT_UPGRADED));
        let other_protocol =
            answered(move |key| upgraded(&accept(key)).replace("websocket", "h2c")).await;
        assert!(refused_with(other_protocol, NOT_UPGRADED));
        let for_another_key = answered(|_| upgraded(&generate_key())).await;
        assert!(refused_with(for_another_key, OTHER_KEY));
        let without_accept = answered(move |key| {
            upgraded(&accept(key))
                .replace(&format!("Sec-WebSocket-Accept: {}\r\n", accept(key)), "")
        })
        .await;
        assert!(refused_with(without_accept, OTHER_KEY));
        let not_101 = answered(move |key| {
            upgraded(&accept(key)).replace("101 Switching Protocols", "200 OK")
        })
        .await;
        assert!(refused_with(not_101, NOT_UPGRADED));
        // The accept value is base64, which is case-sensitive (RFC 6455, section 4.1).
        let in_other_case = answered(move |key| {
            let exact = accept(key);
            let changed: String = exact
                .chars()
                .map(|symbol| match symbol {
                    'a'..='z' => symbol.to_ascii_uppercase(),
                    'A'..='Z' => symbol.to_ascii_lowercase(),
                    other => other,
                })
                .collect();
            assert_ne!(changed, exact);
            upgraded(&changed)
        })
        .await;
        assert!(refused_with(in_other_case, OTHER_KEY));

        let unauthenticated = answered(|_| {
            let body = r#"{"error":{"code":"unauthenticated","message":"no session"}}"#;
            format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
        })
        .await;
        assert!(matches!(
            unauthenticated,
            Err(OpenError::Refused(response)) if response.status == 401
        ));
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_so_the_token_goes_to_no_other_server() {
        use tokio::io::AsyncWriteExt;
        // The other server records the first request of these clients that reaches it, before
        // its answer.
        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let path = marked_path();
        let elsewhere = format!("http://{}{path}", other.local_addr().unwrap());
        let reached = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = reached.clone();
        let other = tokio::spawn(async move {
            let (mut connection, headers) = accept_request(other, &path, plain).await;
            recorded.lock().unwrap().push(headers);
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            connection.write_all(answer.as_bytes()).await.unwrap();
        });
        for status in [
            "301 Moved Permanently",
            "307 Temporary Redirect",
            "308 Permanent Redirect",
        ] {
            let location = elsewhere.clone();
            let opened = answered(move |_| {
                format!("HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
            })
            .await;
            assert!(refused_with(
                opened,
                "the server did not upgrade the connection"
            ));
        }
        // A redirect that was followed has reached the other server before its answer
        // reached the caller.
        assert!(reached.lock().unwrap().is_empty());
        other.abort();
    }

    fn tls_acceptor() -> tokio_rustls::TlsAcceptor {
        use tokio_rustls::rustls;
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let chain = vec![rustls::pki_types::CertificateDer::from(
            SERVER_CERTIFICATE.to_vec(),
        )];
        let key = rustls::pki_types::PrivateKeyDer::try_from(SERVER_KEY.to_vec()).unwrap();
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        // The server prefers HTTP/2: it gets HTTP/1.1 only when the client offers nothing else.
        // The HTTP library is built without HTTP/2 today, so this check fails only after
        // HTTP/2 is added to it without `http1_only` on the stream client.
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config))
    }

    #[tokio::test]
    async fn the_upgrade_goes_over_tls_to_a_trusted_certificate_only_and_in_http_1_1() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = tls_acceptor();
        let path = marked_path();
        let request = request_to(address, "https", &path);
        // The connection of the client that does not trust the certificate ends in the
        // handshake, so only the trusting client sends its request.
        let server = tokio::spawn(async move {
            let open = move |connection| {
                let acceptor = acceptor.clone();
                async move { acceptor.accept(connection).await.ok() }
            };
            let (connection, headers) = accept_request(listener, &path, open).await;
            assert_eq!(
                connection.get_ref().1.alpn_protocol(),
                Some(&b"http/1.1"[..])
            );
            serve_upgrade(connection, headers).await
        });

        // The certificates that the system trusts do not include the test authority.
        let untrusted = ReqwestStreamTransport::new()
            .unwrap()
            .open(request.clone())
            .await;
        assert!(matches!(
            untrusted,
            Err(OpenError::Failed(error)) if error.kind == TransportErrorKind::Connect
        ));

        let authority = reqwest::Certificate::from_der(AUTHORITY).unwrap();
        let trusting = client_builder().add_root_certificate(authority);
        let opened = ReqwestStreamTransport::from_settings(trusting)
            .unwrap()
            .open(request)
            .await;
        says_hello_and_takes_subscribe(opened, server).await;
    }

    #[tokio::test]
    async fn dropping_the_socket_closes_the_connection() {
        let (socket, mut server) = pair().await;
        drop(socket);
        let closed = tokio::time::timeout(Duration::from_secs(10), server.next()).await;
        assert!(matches!(
            closed,
            Ok(Some(Ok(Message::Close(_))) | None | Some(Err(_)))
        ));
    }
}
