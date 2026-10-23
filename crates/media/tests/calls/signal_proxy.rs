//! A TCP proxy in front of the signal port of the media server. It forwards each connection
//! of a client, and a test can hold the connections that are open at one moment: their bytes
//! no longer pass in either direction, as on a network that drops them, while the clients
//! open new connections through the proxy. When the test releases them, the bytes that the
//! clients wrote reach the media server, as segments that a network delivers late. The test
//! receives the signal requests that the clients wrote to the held connections, and can read
//! the requests that the clients write to every connection while it records them. A test can
//! also hold the answers of the media server to the connections that the clients open, so
//! that a client waits for the connection that restores its session.
//!
//! The proxy reads the WebSocket frames of each client as they pass. Of a connection that is
//! not held it keeps only a frame that is not complete and the message that such a frame
//! continues; of a held connection it keeps what the client wrote since the hold.

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
    reader: Reader,
    held: Option<Held>,
    /// The bytes of the media server that wait for [`SignalProxy::pass_answers`].
    answers: Option<Vec<u8>>,
}

/// What a client wrote to a held connection.
#[derive(Default)]
struct Held {
    /// The bytes that have not reached the media server.
    bytes: Vec<u8>,
    /// The signal requests whose last frame the client wrote after the hold.
    requests: Vec<SignalRequest>,
}

/// The reading of the WebSocket connection of a client. The stream starts with the HTTP
/// request of the upgrade.
#[derive(Default)]
struct Reader {
    upgraded: bool,
    /// The bytes that are not read yet: the rest of the request of the upgrade, or a frame
    /// that the client has not written completely.
    pending: Vec<u8>,
    /// The opcode of the first frame of the message that is read and the payload of its
    /// frames so far.
    message: (u8, Vec<u8>),
}

impl Reader {
    /// Reads the next bytes of the client. Returns the payload of each binary message that
    /// they complete: the requests are binary messages.
    fn read(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        let mut at = 0;
        if !self.upgraded {
            let Some(header) = self.pending.windows(4).position(|end| end == b"\r\n\r\n") else {
                return Vec::new();
            };
            self.upgraded = true;
            at = header + 4;
        }
        let mut messages = Vec::new();
        while let Some((fin, opcode, payload, end)) = frame(&self.pending, at) {
            at = end;
            // Control frames (close, ping, pong) can come between the frames of a message.
            if opcode >= 0x8 {
                continue;
            }
            if opcode != 0x0 {
                self.message = (opcode, Vec::new());
            }
            self.message.1.extend_from_slice(&payload);
            if fin {
                let (opcode, payload) = std::mem::take(&mut self.message);
                if opcode == 0x2 {
                    messages.push(payload);
                }
            }
        }
        self.pending.drain(..at);
        messages
    }

    /// The bytes that the reading keeps.
    fn kept(&self) -> usize {
        self.pending.len() + self.message.1.len()
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
    /// The requests that the clients wrote since the test asked to record them, with the
    /// number of the connection: the connections are numbered in the order of their start.
    /// `None` while the test does not record.
    recorded: Option<Vec<(usize, SignalRequest)>>,
    /// The answers of the media server to the connections that open now wait.
    hold_answers: bool,
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
                    let mut held = shared.lock().unwrap();
                    let link = Arc::new(Link {
                        state: Mutex::new(LinkState {
                            answers: held.hold_answers.then(Vec::new),
                            ..LinkState::default()
                        }),
                        to_server: tokio::sync::Mutex::new(to_server),
                        to_client: tokio::sync::Mutex::new(to_client),
                    });
                    let number = held.links.len();
                    let up =
                        tokio::spawn(upward(from_client, link.clone(), number, shared.clone()));
                    let down = tokio::spawn(downward(from_server, link.clone()));
                    held.links.push(link);
                    held.tasks.extend([up, down]);
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

    /// The number of connections that the clients have opened so far. The next connection
    /// has this number.
    pub fn connections(&self) -> usize {
        self.shared.lock().unwrap().links.len()
    }

    /// Records the requests that the clients write from now on, to every connection.
    pub fn record(&self) {
        self.shared.lock().unwrap().recorded = Some(Vec::new());
    }

    /// The requests that the clients wrote since [`SignalProxy::record`], in the order in
    /// which the proxy read them, each with the number of its connection.
    pub fn recorded(&self) -> Vec<(usize, SignalRequest)> {
        self.shared
            .lock()
            .unwrap()
            .recorded
            .clone()
            .unwrap_or_default()
    }

    /// The bytes of the media server to the connections that the clients open from now on
    /// wait, until [`SignalProxy::pass_answers`]. A client then waits for the answer to its
    /// request of the upgrade, and for the answers to its signal requests.
    pub fn hold_answers(&self) {
        self.shared.lock().unwrap().hold_answers = true;
    }

    /// Gives each client the answers that waited, in their order, and lets the next ones
    /// pass.
    pub async fn pass_answers(&self) {
        let links = {
            let mut shared = self.shared.lock().unwrap();
            shared.hold_answers = false;
            shared.links.clone()
        };
        for link in links {
            // The connection to the client is taken first: an answer that arrives meanwhile
            // waits for it and follows the ones that waited.
            let mut to_client = link.to_client.lock().await;
            let Some(answers) = link.state.lock().unwrap().answers.take() else {
                continue;
            };
            let _ = to_client.write_all(&answers).await;
        }
    }

    /// Holds every connection that is open now. Connections opened later pass.
    pub fn hold_open(&self) {
        for link in &self.shared.lock().unwrap().links {
            link.state.lock().unwrap().held.get_or_insert_default();
        }
    }

    /// Sends what the clients wrote on each held connection to the media server, then ends
    /// the connection.
    pub async fn release(&self) -> Released {
        let held: Vec<Arc<Link>> = self.shared.lock().unwrap().links.clone();
        let mut released = Released {
            sent: 0,
            requests: Vec::new(),
        };
        for link in held {
            let Some(held) = link.state.lock().unwrap().held.take() else {
                continue;
            };
            released.requests.extend(held.requests);
            let mut to_server = link.to_server.lock().await;
            // The media server can have ended the connection already: then the bytes are lost.
            if to_server.write_all(&held.bytes).await.is_ok() {
                released.sent += held.bytes.len();
            }
            let _ = to_server.shutdown().await;
            let _ = link.to_client.lock().await.shutdown().await;
        }
        released
    }

    /// The bytes that the proxy keeps of what the clients wrote, over all connections.
    fn kept(&self) -> usize {
        self.shared
            .lock()
            .unwrap()
            .links
            .iter()
            .map(|link| {
                let state = link.state.lock().unwrap();
                state.reader.kept() + state.held.as_ref().map_or(0, |held| held.bytes.len())
            })
            .sum()
    }
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
async fn upward(
    mut from_client: OwnedReadHalf,
    link: Arc<Link>,
    number: usize,
    shared: Arc<Mutex<Shared>>,
) {
    let mut buffer = vec![0; 16 * 1024];
    loop {
        let read = match from_client.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let bytes = &buffer[..read];
        let held = {
            let mut shared = shared.lock().unwrap();
            let mut state = link.state.lock().unwrap();
            let messages = state.reader.read(bytes);
            let decoded = || {
                messages.iter().map(|message| {
                    SignalRequest::decode(message.as_slice()).unwrap_or_else(|e| {
                        panic!("a client wrote a request that is not read: {e}")
                    })
                })
            };
            if let Some(recorded) = &mut shared.recorded {
                recorded.extend(decoded().map(|request| (number, request)));
            }
            match &mut state.held {
                Some(held) => {
                    held.requests.extend(decoded());
                    held.bytes.extend_from_slice(bytes);
                    true
                }
                None => false,
            }
        };
        if held {
            continue;
        }
        if link.to_server.lock().await.write_all(bytes).await.is_err() {
            break;
        }
    }
    // The end of a held connection reaches the media server with its bytes, at the release.
    if link.state.lock().unwrap().held.is_none() {
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
        {
            let mut state = link.state.lock().unwrap();
            if state.held.is_some() {
                continue;
            }
            if let Some(answers) = &mut state.answers {
                answers.extend_from_slice(&buffer[..read]);
                continue;
            }
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
    // The end of a connection whose answers wait reaches the client after them, when the
    // test passes them.
    let ends = {
        let state = link.state.lock().unwrap();
        state.held.is_none() && state.answers.is_none()
    };
    if ends {
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

/// A connection that is not held passes any number of bytes, and the proxy keeps at most
/// the frame that is not complete yet: a call of an hour does not fill the memory of the
/// test with what it sent.
#[tokio::test]
async fn the_proxy_keeps_no_more_than_one_frame_of_a_connection_that_is_not_held() {
    const FRAME: usize = 64 * 1024;
    let server = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = server.local_addr().unwrap().port();
    let proxy = SignalProxy::start_at(&format!("ws://127.0.0.1:{port}")).await;
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    let upgrade = b"GET /rtc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
    client.write_all(upgrade).await.unwrap();
    let (mut upstream, _) = server.accept().await.unwrap();
    let mut header = vec![0; upgrade.len()];
    upstream.read_exact(&mut header).await.unwrap();
    // 64 frames of 64 KiB, each a whole binary message, and the start of one more.
    let frame = client_frame(&vec![7; FRAME]);
    let reading = tokio::spawn(async move {
        let mut received = vec![0; frame_length(FRAME) * 64 + 10];
        upstream.read_exact(&mut received).await.unwrap();
        upstream
    });
    for _ in 0..64 {
        client.write_all(&frame).await.unwrap();
    }
    client.write_all(&frame[..10]).await.unwrap();
    let _upstream = reading.await.unwrap();
    let kept = proxy.kept();
    assert!(
        kept <= FRAME,
        "the proxy keeps {kept} bytes of a connection that passed {} bytes",
        frame_length(FRAME) * 64
    );
}

/// The length of a masked client frame with a payload of `payload` bytes.
fn frame_length(payload: usize) -> usize {
    2 + 8 + 4 + payload
}

/// One masked binary frame of a client that ends its message.
fn client_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [1, 2, 3, 4];
    let mut frame = vec![0x82, 0x80 | 127];
    frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

/// The requests that a client wrote to a held connection are the ones whose last frame it
/// wrote after the hold, also when a frame was cut by the hold, and they reach the media
/// server at the release, after the ones that passed before the hold.
#[tokio::test]
async fn the_requests_written_to_a_held_connection_reach_the_server_at_the_release() {
    use livekit_protocol::{MuteTrackRequest, signal_request};
    let server = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = server.local_addr().unwrap().port();
    let proxy = SignalProxy::start_at(&format!("ws://127.0.0.1:{port}")).await;
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    let upgrade = b"GET /rtc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
    let request = |sid: &str| {
        let request = SignalRequest {
            message: Some(signal_request::Message::Mute(MuteTrackRequest {
                sid: sid.to_owned(),
                muted: true,
            })),
        };
        client_frame(&request.encode_to_vec())
    };
    let (first, cut, last) = (request("first"), request("cut"), request("last"));
    client.write_all(upgrade).await.unwrap();
    client.write_all(&first).await.unwrap();
    client.write_all(&cut[..5]).await.unwrap();
    let (mut upstream, _) = server.accept().await.unwrap();
    let mut passed = vec![0; upgrade.len() + first.len() + 5];
    upstream.read_exact(&mut passed).await.unwrap();

    proxy.hold_open();
    client.write_all(&cut[5..]).await.unwrap();
    client.write_all(&last).await.unwrap();
    // The proxy has read the bytes when it keeps them.
    let written = cut.len() - 5 + last.len();
    let started = tokio::time::Instant::now();
    while proxy.kept() < written {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let released = proxy.release().await;
    let sids: Vec<String> = released
        .requests
        .iter()
        .map(|request| match &request.message {
            Some(signal_request::Message::Mute(mute)) => mute.sid.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(sids, ["cut", "last"]);
    assert_eq!(released.sent, written);
    let mut late = Vec::new();
    upstream.read_to_end(&mut late).await.unwrap();
    assert_eq!(late, [&cut[5..], last.as_slice()].concat());
}
