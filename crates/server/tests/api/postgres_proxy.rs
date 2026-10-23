//! A proxy between a server process and PostgreSQL that ends a connection at a chosen
//! statement, as when the process stops there or the network loses the answer (ADR 0904).
//! It reads the messages of the protocol of PostgreSQL to find the statement and the end of
//! its answer, and passes every byte through unchanged otherwise, with two exceptions: it
//! answers a request for TLS or GSS encryption itself with `N`, and it passes no part of an
//! answer that a stop holds.

use std::sync::{Arc, Mutex};

use sqlx::postgres::{PgConnectOptions, PgSslMode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// Where a connection ends at its statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// The statement does not reach PostgreSQL, and nothing after it does.
    Before,
    /// PostgreSQL runs the statement to its end; its answer does not reach the process.
    AfterAnswer,
}

/// The statement at which one connection ends: the first statement whose text contains
/// `at`, on a connection that has sent, after the stop was armed and since its last `COMMIT`
/// or `ROLLBACK`, a statement that contains `after`. A statement counts when a `Query` runs
/// it or a `Bind` binds it. Text is compared in lower case.
#[derive(Debug, Clone)]
pub struct Stop {
    pub after: Option<String>,
    pub at: String,
    pub cut: Cut,
}

impl Stop {
    pub fn at(at: &str, cut: Cut) -> Self {
        Self {
            after: None,
            at: at.to_lowercase(),
            cut,
        }
    }

    pub fn after(mut self, after: &str) -> Self {
        self.after = Some(after.to_lowercase());
        self
    }
}

struct Armed {
    stop: Stop,
    struck: Arc<Notify>,
}

pub struct PostgresProxy {
    target: PgConnectOptions,
    address: std::net::SocketAddr,
    armed: Arc<Mutex<Vec<Armed>>>,
    accepting: tokio::task::JoinHandle<()>,
}

impl Drop for PostgresProxy {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// The longest wait for a connection to reach an armed stop. A stop that no statement
/// reaches, because the statement has another text or is not sent, fails the test.
pub const STOP_PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

/// Resolves when the stop of [`PostgresProxy::arm`] has ended its connection.
pub struct Struck {
    struck: Arc<Notify>,
    stop: Stop,
    armed: Arc<Mutex<Vec<Armed>>>,
}

impl Struck {
    pub async fn wait(&self) {
        if tokio::time::timeout(STOP_PATIENCE, self.struck.notified())
            .await
            .is_err()
        {
            let armed = self.armed.lock().unwrap().len();
            panic!(
                "no connection reached the stop {:?} within {STOP_PATIENCE:?}; {armed} stops \
                 of the proxy are armed",
                self.stop
            );
        }
    }
}

impl PostgresProxy {
    /// A proxy in front of the database of `target`.
    pub async fn start(target: PgConnectOptions) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let armed: Arc<Mutex<Vec<Armed>>> = Arc::default();
        let (host, port) = (target.get_host().to_owned(), target.get_port());
        let shared = armed.clone();
        // The set ends the connections when the proxy ends this task.
        let accepting = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let client = match listener.accept().await {
                    Ok((client, _)) => client,
                    Err(error) => panic!("the proxy accepts no further connection: {error}"),
                };
                match TcpStream::connect((host.as_str(), port)).await {
                    Ok(server) => {
                        connections.spawn(Connection::new(client, server, shared.clone()).run());
                    }
                    Err(error) => eprintln!("the proxy did not reach PostgreSQL: {error}"),
                }
                while connections.try_join_next().is_some() {}
            }
        });
        Self {
            target,
            address,
            armed,
            accepting,
        }
    }

    /// The options of the database of the target, through this proxy.
    pub fn options(&self) -> PgConnectOptions {
        self.target
            .clone()
            .host("127.0.0.1")
            .port(self.address.port())
            .ssl_mode(PgSslMode::Disable)
    }

    /// Ends the first connection that reaches `stop`. Each stop ends one connection.
    pub fn arm(&self, stop: Stop) -> Struck {
        let struck = Arc::new(Notify::new());
        self.armed.lock().unwrap().push(Armed {
            stop: stop.clone(),
            struck: struck.clone(),
        });
        Struck {
            struck,
            stop,
            armed: self.armed.clone(),
        }
    }

    /// The stops that no connection has reached yet.
    pub fn armed(&self) -> usize {
        self.armed.lock().unwrap().len()
    }
}

const SSL_REQUEST: u32 = 80_877_103;
const GSS_REQUEST: u32 = 80_877_104;

/// One connection of the process and its connection to PostgreSQL.
struct Connection {
    client: TcpStream,
    server: TcpStream,
    armed: Arc<Mutex<Vec<Armed>>>,
    from_client: Vec<u8>,
    from_server: Vec<u8>,
    /// The client has sent its startup message: every later message has a type byte.
    started: bool,
    /// The `after` texts of armed stops that a statement since the last `COMMIT` or
    /// `ROLLBACK` contained.
    seen_after: Vec<String>,
    /// Answers that end with `ReadyForQuery`: the one of the start, and one for each `Query`
    /// and `Sync` that the client sent.
    asked: u64,
    answered: u64,
    /// The stop that holds the answers from the one with this number on.
    holding: Option<(u64, Arc<Notify>)>,
    /// A stop of the extended protocol whose `Sync` has not passed yet.
    waiting_sync: Option<Arc<Notify>>,
    /// The text of each prepared statement of the connection, by its name.
    prepared: std::collections::HashMap<Vec<u8>, String>,
}

enum Next {
    Continue,
    End,
}

impl Connection {
    fn new(client: TcpStream, server: TcpStream, armed: Arc<Mutex<Vec<Armed>>>) -> Self {
        Self {
            client,
            server,
            armed,
            from_client: Vec::new(),
            from_server: Vec::new(),
            started: false,
            seen_after: Vec::new(),
            asked: 1,
            answered: 0,
            holding: None,
            waiting_sync: None,
            prepared: std::collections::HashMap::new(),
        }
    }

    async fn run(mut self) {
        let mut client_bytes = [0u8; 16 * 1024];
        let mut server_bytes = [0u8; 16 * 1024];
        loop {
            let next = tokio::select! {
                read = self.client.read(&mut client_bytes) => match read {
                    Ok(count @ 1..) => {
                        self.from_client.extend_from_slice(&client_bytes[..count]);
                        self.client_messages().await
                    }
                    _ => Next::End,
                },
                read = self.server.read(&mut server_bytes) => match read {
                    Ok(count @ 1..) => {
                        self.from_server.extend_from_slice(&server_bytes[..count]);
                        self.server_messages().await
                    }
                    _ => Next::End,
                },
            };
            if matches!(next, Next::End) {
                break;
            }
        }
        let _ = self.client.shutdown().await;
        let _ = self.server.shutdown().await;
    }

    /// Passes the complete messages of the client to PostgreSQL, up to a stop, in one write.
    async fn client_messages(&mut self) -> Next {
        let mut out = Vec::new();
        let next = self.client_part(&mut out).await;
        if self.server.write_all(&out).await.is_err() {
            return Next::End;
        }
        next
    }

    async fn client_part(&mut self, out: &mut Vec<u8>) -> Next {
        loop {
            let Some(length) = self.next_client_message() else {
                return Next::Continue;
            };
            let message: Vec<u8> = self.from_client.drain(..length).collect();
            if !self.started {
                let code = u32::from_be_bytes([message[4], message[5], message[6], message[7]]);
                if code == SSL_REQUEST || code == GSS_REQUEST {
                    if self.client.write_all(b"N").await.is_err() {
                        return Next::End;
                    }
                    continue;
                }
                self.started = true;
                out.extend_from_slice(&message);
                continue;
            }
            let kind = message[0];
            if kind == b'Q' || kind == b'S' {
                self.asked += 1;
            }
            if kind == b'S'
                && let Some(struck) = self.waiting_sync.take()
            {
                self.holding = Some((self.asked, struck));
            }
            if kind == b'P'
                && let Some((name, text)) = parsed(&message)
            {
                self.prepared.insert(name, text);
            }
            if kind == b'C'
                && message.get(5) == Some(&b'S')
                && let Some((name, _)) = cstring(&message[6..])
            {
                self.prepared.remove(name);
            }
            if let Some(text) = self.statement(&message) {
                let text = text.to_lowercase();
                self.note_after(&text);
                if let Some(armed) = self.reached(&text) {
                    match armed.stop.cut {
                        Cut::Before => {
                            armed.struck.notify_one();
                            return Next::End;
                        }
                        Cut::AfterAnswer if kind == b'Q' => {
                            self.holding = Some((self.asked, armed.struck));
                        }
                        Cut::AfterAnswer => self.waiting_sync = Some(armed.struck),
                    }
                }
                if text.trim_start().starts_with("commit")
                    || text.trim_start().starts_with("rollback")
                {
                    self.seen_after.clear();
                }
            }
            out.extend_from_slice(&message);
        }
    }

    /// Passes the complete messages of PostgreSQL to the client in one write, except the
    /// answers that a stop holds; ends the connection when the held answer is complete.
    async fn server_messages(&mut self) -> Next {
        let mut out = Vec::new();
        let next = self.server_part(&mut out);
        if self.client.write_all(&out).await.is_err() {
            return Next::End;
        }
        next
    }

    fn server_part(&mut self, out: &mut Vec<u8>) -> Next {
        loop {
            if self.from_server.len() < 5 {
                return Next::Continue;
            }
            let length = 1 + read_length(&self.from_server[1..5]);
            if self.from_server.len() < length {
                return Next::Continue;
            }
            let message: Vec<u8> = self.from_server.drain(..length).collect();
            let held = self
                .holding
                .as_ref()
                .is_some_and(|(from, _)| self.answered + 1 >= *from);
            if message[0] == b'Z' {
                self.answered += 1;
            }
            if held {
                if let Some((from, struck)) = &self.holding
                    && message[0] == b'Z'
                    && self.answered == *from
                {
                    struck.notify_one();
                    return Next::End;
                }
                continue;
            }
            out.extend_from_slice(&message);
        }
    }

    /// The length of the next complete message of the client, if it has arrived.
    fn next_client_message(&self) -> Option<usize> {
        let buffer = &self.from_client;
        let (header, offset) = if self.started { (5, 1) } else { (4, 0) };
        if buffer.len() < header {
            return None;
        }
        let length = offset + read_length(&buffer[offset..offset + 4]);
        (buffer.len() >= length).then_some(length)
    }

    /// The text of the statement that a `Query` runs, or that a `Bind` binds to be run.
    fn statement(&self, message: &[u8]) -> Option<String> {
        let body = &message[5..];
        match message[0] {
            b'Q' => cstring(body).map(|(text, _)| String::from_utf8_lossy(text).into_owned()),
            b'B' => {
                let (_portal, rest) = cstring(body)?;
                let (name, _) = cstring(rest)?;
                self.prepared.get(name).cloned()
            }
            _ => None,
        }
    }

    /// Records the `after` text of each armed stop that the statement `text` contains.
    fn note_after(&mut self, text: &str) {
        let armed = self.armed.lock().unwrap();
        for after in armed.iter().filter_map(|armed| armed.stop.after.as_ref()) {
            if text.contains(after.as_str()) && !self.seen_after.contains(after) {
                self.seen_after.push(after.clone());
            }
        }
    }

    /// Takes the first armed stop that the statement `text` reaches on this connection.
    fn reached(&self, text: &str) -> Option<Armed> {
        let mut armed = self.armed.lock().unwrap();
        let place = armed.iter().position(|armed| {
            text.contains(&armed.stop.at)
                && armed
                    .stop
                    .after
                    .as_ref()
                    .is_none_or(|after| self.seen_after.contains(after))
        })?;
        Some(armed.remove(place))
    }
}

fn read_length(bytes: &[u8]) -> usize {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
}

/// The name and the text of the statement of a `Parse` message.
fn parsed(message: &[u8]) -> Option<(Vec<u8>, String)> {
    let (name, rest) = cstring(&message[5..])?;
    let (text, _) = cstring(rest)?;
    Some((name.to_vec(), String::from_utf8_lossy(text).into_owned()))
}

/// The text up to the first zero byte, and the bytes after that byte.
fn cstring(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes.iter().position(|byte| *byte == 0)?;
    Some((&bytes[..end], &bytes[end + 1..]))
}

/// A stop that no statement reaches ends the wait for it with a failure that names the stop.
#[tokio::test]
#[should_panic(expected = "no connection reached the stop")]
async fn the_wait_for_a_stop_that_no_statement_reaches_fails_and_names_the_stop() {
    let target: PgConnectOptions =
        std::str::FromStr::from_str(&crate::support::unused_database_url()).unwrap();
    let proxy = PostgresProxy::start(target).await;
    let struck = proxy.arm(Stop::at("a statement that nobody sends", Cut::Before));
    tokio::time::timeout(STOP_PATIENCE * 2, struck.wait())
        .await
        .expect("the wait for an unreached stop ended within twice its limit");
}
