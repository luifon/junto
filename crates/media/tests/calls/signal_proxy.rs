//! A TCP proxy in front of the signal port of the media server. It forwards each connection
//! of a client, and a test can hold the connections that are open at one moment: their bytes
//! no longer pass in either direction, as on a network that drops them, while the clients
//! open new connections through the proxy. When the test releases them, the bytes that the
//! clients wrote reach the media server, as segments that a network delivers late.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

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
    held: bool,
    /// The bytes that the client wrote while the connection was held, in their order.
    from_client: Vec<u8>,
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
        let server: SocketAddr = url
            .strip_prefix("ws://")
            .and_then(|address| address.parse().ok())
            .unwrap_or_else(|| panic!("{url} is not ws://<address>:<port>"));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let accept = tokio::spawn({
            let shared = shared.clone();
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    let Ok(upstream) = TcpStream::connect(server).await else {
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
            link.state.lock().unwrap().held = true;
        }
    }

    /// Sends what the clients wrote on each held connection to the media server, then ends
    /// the connection. Returns the number of bytes sent.
    pub async fn release(&self) -> usize {
        let held: Vec<Arc<Link>> = self
            .shared
            .lock()
            .unwrap()
            .links
            .iter()
            .filter(|link| link.state.lock().unwrap().held)
            .cloned()
            .collect();
        let mut sent = 0;
        for link in held {
            let bytes = std::mem::take(&mut link.state.lock().unwrap().from_client);
            let mut to_server = link.to_server.lock().await;
            // The media server can have ended the connection already: then the bytes are lost.
            if to_server.write_all(&bytes).await.is_ok() {
                sent += bytes.len();
            }
            let _ = to_server.shutdown().await;
            let _ = link.to_client.lock().await.shutdown().await;
        }
        sent
    }
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
            if state.held {
                state.from_client.extend_from_slice(&buffer[..read]);
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
    if !link.state.lock().unwrap().held {
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
        if link.state.lock().unwrap().held {
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
    if !link.state.lock().unwrap().held {
        let _ = link.to_client.lock().await.shutdown().await;
    }
}
