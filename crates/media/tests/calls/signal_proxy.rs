//! A TCP proxy in front of the signal port of the media server. It forwards each connection
//! of a client, and a test can hold the connections that are open at one moment: their bytes
//! no longer pass in either direction, as on a network that drops them, while the clients
//! open new connections through the proxy. When the test releases them, the bytes that the
//! clients wrote reach the media server, as segments that a network delivers late. The test
//! receives the signal requests that the clients wrote to the held connections.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use livekit_protocol::SignalRequest;
use prost::Message as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One connection of a client through the proxy.
struct Link {
    state: Mutex<LinkState>,
    to_server: tokio::sync::Mutex<OwnedWriteHalf>,
    /// Kept while the connection is held: dropping it would end the connection to the client.
    to_client: tokio::sync::Mutex<OwnedWriteHalf>,
}

#[derive(Default)]
struct LinkState {
    /// The number of bytes of `from_client` that had passed when the connection was held.
    held_from: Option<usize>,
    /// Every byte that the client wrote, in their order: the bytes from `held_from` on have
    /// not reached the media server.
    from_client: Vec<u8>,
}

impl LinkState {
    fn held(&self) -> bool {
        self.held_from.is_some()
    }
}

/// What the clients wrote to the held connections.
pub struct Released {
    /// The number of bytes that reached the media server at the release.
    pub sent: usize,
    /// The signal requests that a client wrote while its connection was held, in their order
    /// on each connection.
    pub requests: Vec<SignalRequest>,
}

#[derive(Default)]
struct Shared {
    links: Vec<Arc<Link>>,
    tasks: Vec<JoinHandle<()>>,
}

pub struct SignalProxy {
    address: SocketAddr,
    shared: Arc<Mutex<Shared>>,
}

impl Drop for SignalProxy {
    fn drop(&mut self) {
        for task in &self.shared.lock().unwrap().tasks {
            task.abort();
        }
    }
}

impl SignalProxy {
    /// A proxy in front of the media server of `tools/with-test-livekit.sh`.
    pub async fn start() -> Self {
        let url = std::env::var("JUNTO_TEST_LIVEKIT_URL")
            .expect("JUNTO_TEST_LIVEKIT_URL names the LiveKit server of the test run");
        Self::start_at(&url).await
    }

    /// A proxy in front of the signal port that `url` names. The host can be a name: in the
    /// Linux build image the media server is the service `livekit`.
    async fn start_at(url: &str) -> Self {
        let authority = url
            .strip_prefix("ws://")
            .filter(|authority| !authority.contains('/'))
            .unwrap_or_else(|| panic!("{url} is not ws://<host>:<port>"));
        let server: Vec<SocketAddr> = tokio::net::lookup_host(authority)
            .await
            .unwrap_or_else(|e| panic!("the host of {url} is not resolved: {e}"))
            .collect();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let accept = tokio::spawn({
            let shared = shared.clone();
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    let Ok(upstream) = TcpStream::connect(server.as_slice()).await else {
                        continue;
                    };
                    let _ = client.set_nodelay(true);
                    let _ = upstream.set_nodelay(true);
                    let (from_client, to_client) = client.into_split();
                    let (from_server, to_server) = upstream.into_split();
                    let link = Arc::new(Link {
                        state: Mutex::default(),
                        to_server: tokio::sync::Mutex::new(to_server),
                        to_client: tokio::sync::Mutex::new(to_client),
                    });
                    let up = tokio::spawn(upward(from_client, link.clone()));
                    let down = tokio::spawn(downward(from_server, link.clone()));
                    let mut shared = shared.lock().unwrap();
                    shared.links.push(link);
                    shared.tasks.extend([up, down]);
                }
            }
        });
        shared.lock().unwrap().tasks.push(accept);
        Self { address, shared }
    }

    /// The address that the clients use for the media server.
    pub fn url(&self) -> String {
        format!("ws://{}", self.address)
    }

    /// Holds every connection that is open now. Connections opened later pass.
    pub fn hold_open(&self) {
        for link in &self.shared.lock().unwrap().links {
            let mut state = link.state.lock().unwrap();
            state.held_from = Some(state.from_client.len());
        }
    }

    /// Sends what the clients wrote on each held connection to the media server, then ends
    /// the connection.
    pub async fn release(&self) -> Released {
        let held: Vec<Arc<Link>> = self
            .shared
            .lock()
            .unwrap()
            .links
            .iter()
            .filter(|link| link.state.lock().unwrap().held())
            .cloned()
            .collect();
        let mut released = Released {
            sent: 0,
            requests: Vec::new(),
        };
        for link in held {
            let (from_client, held_from) = {
                let state = link.state.lock().unwrap();
                (state.from_client.clone(), state.held_from.unwrap())
            };
            released
                .requests
                .extend(requests_written_from(&from_client, held_from));
            let bytes = &from_client[held_from..];
            let mut to_server = link.to_server.lock().await;
            // The media server can have ended the connection already: then the bytes are lost.
            if to_server.write_all(bytes).await.is_ok() {
                released.sent += bytes.len();
            }
            let _ = to_server.shutdown().await;
            let _ = link.to_client.lock().await.shutdown().await;
        }
        released
    }
}

/// The signal requests of the WebSocket connection whose client wrote `stream`, for each
/// request whose frames end after byte `from`. The stream starts with the HTTP request of the
/// upgrade. A frame that the client had not written completely is not read.
fn requests_written_from(stream: &[u8], from: usize) -> Vec<SignalRequest> {
    let Some(header) = stream.windows(4).position(|end| end == b"\r\n\r\n") else {
        return Vec::new();
    };
    let mut at = header + 4;
    // The opcode of the first frame of the message and the payload of its frames so far.
    let mut message = (0, Vec::new());
    let mut requests = Vec::new();
    while let Some((fin, opcode, payload, end)) = frame(stream, at) {
        at = end;
        // Control frames (close, ping, pong) can come between the frames of a message.
        if opcode >= 0x8 {
            continue;
        }
        if opcode != 0x0 {
            message = (opcode, Vec::new());
        }
        message.1.extend_from_slice(&payload);
        // The requests are binary messages.
        if fin && message.0 == 0x2 && end > from {
            let request = SignalRequest::decode(message.1.as_slice())
                .unwrap_or_else(|e| panic!("a client wrote a request that is not read: {e}"));
            requests.push(request);
        }
    }
    requests
}

/// The frame of a client at `at`: whether it ends its message, its opcode, its unmasked
/// payload and the position after it. `None` when the stream ends before the frame does.
fn frame(stream: &[u8], at: usize) -> Option<(bool, u8, Vec<u8>, usize)> {
    let first = *stream.get(at)?;
    let second = *stream.get(at + 1)?;
    let mut at = at + 2;
    let length = match second & 0x7f {
        126 => {
            let bytes = stream.get(at..at + 2)?;
            at += 2;
            u64::from(u16::from_be_bytes([bytes[0], bytes[1]]))
        }
        127 => {
            let bytes = stream.get(at..at + 8)?;
            at += 8;
            u64::from_be_bytes(bytes.try_into().unwrap())
        }
        short => u64::from(short),
    };
    let length = usize::try_from(length).unwrap();
    // A client masks every frame that it sends.
    assert!(second & 0x80 != 0, "a client frame is not masked");
    let mask = stream.get(at..at + 4)?;
    at += 4;
    let payload = stream
        .get(at..at + length)?
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % 4])
        .collect();
    Some((first & 0x80 != 0, first & 0x0f, payload, at + length))
}

/// Forwards the bytes of the client to the media server, or keeps them while the connection
/// is held.
async fn upward(mut from_client: OwnedReadHalf, link: Arc<Link>) {
    let mut buffer = vec![0; 16 * 1024];
    loop {
        let read = match from_client.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        {
            let mut state = link.state.lock().unwrap();
            state.from_client.extend_from_slice(&buffer[..read]);
            if state.held() {
                continue;
            }
        }
        if link
            .to_server
            .lock()
            .await
            .write_all(&buffer[..read])
            .await
            .is_err()
        {
            break;
        }
    }
    // The end of a held connection reaches the media server with its bytes, at the release.
    if !link.state.lock().unwrap().held() {
        let _ = link.to_server.lock().await.shutdown().await;
    }
}

/// Forwards the bytes of the media server to the client. While the connection is held, the
/// bytes and the end of the connection are dropped.
async fn downward(mut from_server: OwnedReadHalf, link: Arc<Link>) {
    let mut buffer = vec![0; 16 * 1024];
    loop {
        let read = match from_server.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if link.state.lock().unwrap().held() {
            continue;
        }
        if link
            .to_client
            .lock()
            .await
            .write_all(&buffer[..read])
            .await
            .is_err()
        {
            break;
        }
    }
    if !link.state.lock().unwrap().held() {
        let _ = link.to_client.lock().await.shutdown().await;
    }
}

#[tokio::test]
async fn the_proxy_reaches_a_media_server_named_by_its_host_name() {
    let server = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = server.local_addr().unwrap().port();
    let proxy = SignalProxy::start_at(&format!("ws://localhost:{port}")).await;
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(b"upgrade").await.unwrap();
    let (mut upstream, _) = server.accept().await.unwrap();
    let mut received = [0; 7];
    upstream.read_exact(&mut received).await.unwrap();
    assert_eq!(&received, b"upgrade");
}
