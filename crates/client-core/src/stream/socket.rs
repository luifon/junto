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

use crate::api::transport::{CONNECT_TIMEOUT, failure, read};
use crate::api::{HttpResponse, TransportError, TransportErrorKind};
use crate::secret::SecretToken;

/// The largest message of a server that the client reads: the size of a whole batch of
/// events on the server (ADR 0116). An event of this API version is far smaller.
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;

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
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .http1_only()
            .user_agent(concat!("junto-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
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
            let names = |name, expected: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.eq_ignore_ascii_case(expected))
            };
            if status != 101 || !names(UPGRADE, "websocket") {
                return Err(invalid("the server did not upgrade the connection"));
            }
            if !names(SEC_WEBSOCKET_ACCEPT, &derive_accept_key(key.as_bytes())) {
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
