//! Starts the real server binary of this checkout on a database of its own, and a proxy
//! that can stand between a client and a server to make requests fail.

use std::io::Read;
use std::net::{SocketAddr, TcpListener as StdListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::flow::add_server::{AddServerAction, AddServerState};
use junto_client_core::protocol::ids::AccountId;
use junto_client_core::protocol::secret::{InviteCode, OwnerCode};
use junto_client_core::version::SupportedVersions;
use junto_client_core::{Client, ClientOptions, ServerHandle};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DATABASE_URL: &str = "JUNTO_TEST_DATABASE_URL";
const SERVER_BINARY: &str = "JUNTO_SERVER_BIN";

fn admin_url() -> String {
    std::env::var(DATABASE_URL).unwrap_or_else(|_| {
        panic!("{DATABASE_URL} is not set. Run the tests through tools/with-test-postgres.sh.")
    })
}

fn server_binary() -> PathBuf {
    if let Some(path) = std::env::var_os(SERVER_BINARY) {
        return path.into();
    }
    let test_binary = std::env::current_exe().unwrap();
    let profile = test_binary.parent().unwrap().parent().unwrap();
    let path = profile.join(format!("junto-server{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} does not exist. Build it with `cargo build -p junto-server`, or set {SERVER_BINARY}.",
        path.display()
    );
    path
}

fn free_port() -> u16 {
    StdListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One server process on its own database. Its public origins are its own address and the
/// address of one proxy, whose port is held open from the start of the server until
/// [`FaultProxy::start`] takes it (ADR 0117).
pub struct TestServer {
    child: Option<Child>,
    output: Arc<Mutex<String>>,
    name: String,
    port: u16,
    proxy: Mutex<Option<StdListener>>,
    proxy_port: u16,
    pub database_url: String,
}

impl TestServer {
    pub async fn start(name: &str) -> Self {
        let database = format!("junto_client_{}", uuid::Uuid::new_v4().simple());
        let admin = PgConnectOptions::from_str(&admin_url()).unwrap();
        let mut connection = admin.connect().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("create database {database}")))
            .execute(&mut connection)
            .await
            .unwrap();
        drop(connection);
        let mut database_url = url::Url::parse(&admin_url()).unwrap();
        database_url.set_path(&database);
        let proxy = StdListener::bind(("127.0.0.1", 0)).unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        let mut server = Self {
            child: None,
            output: Arc::default(),
            name: name.to_owned(),
            port: free_port(),
            proxy: Mutex::new(Some(proxy)),
            proxy_port,
            database_url: database_url.into(),
        };
        server.run().await;
        server.wait_for_owner_code().await;
        server
    }

    /// A server on a new database prints an owner code before it listens. Another thread
    /// copies the output, so the code can arrive here after the port answers.
    async fn wait_for_owner_code(&self) {
        for _ in 0..600 {
            if self.owner_code().is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the server printed no owner code");
    }

    async fn run(&mut self) {
        let logs = if std::env::var_os("JUNTO_TEST_SERVER_LOG").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };
        let mut child = Command::new(server_binary())
            .env("JUNTO_DATABASE_URL", &self.database_url)
            .env("JUNTO_LISTEN_ADDRESS", format!("127.0.0.1:{}", self.port))
            .env("JUNTO_SERVER_NAME", &self.name)
            .env(
                "JUNTO_PUBLIC_ORIGINS",
                format!(
                    "http://127.0.0.1:{},http://127.0.0.1:{}",
                    self.port, self.proxy_port
                ),
            )
            .env("JUNTO_LOG_FORMAT", "text")
            .env("JUNTO_LOG_LEVEL", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(logs)
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        self.output = Arc::default();
        let output = self.output.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            while let Ok(count @ 1..) = stdout.read(&mut buffer) {
                output
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..count]));
            }
        });
        self.child = Some(child);
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        for _ in 0..600 {
            if TcpStream::connect(address).await.is_ok() {
                return;
            }
            let exited = self.child.as_mut().unwrap().try_wait().unwrap();
            assert!(exited.is_none(), "the server stopped at its start");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the server did not start to listen");
    }

    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The owner code that the running process printed. A server with an owner prints none.
    pub fn owner_code(&self) -> Option<String> {
        code_in(&self.output.lock().unwrap())
    }

    /// Runs a host command of the server binary on the database of this server (ADR 0114).
    pub fn host_command(&self, arguments: &[&str]) -> String {
        let output = Command::new(server_binary())
            .args(arguments)
            .env("JUNTO_DATABASE_URL", &self.database_url)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Starts a new process on the same database and the same port.
    pub async fn restart(&mut self) {
        self.stop();
        self.run().await;
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The owner code in the output of the server: five groups of four symbols.
pub fn code_in(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| {
            let groups: Vec<&str> = line.split('-').collect();
            groups.len() == 5
                && groups.iter().all(|group| {
                    group.len() == 4 && group.chars().all(|c| c.is_ascii_alphanumeric())
                })
        })
        .map(str::to_owned)
}

/// One simulated device: a data directory of its own and a client on it.
pub struct Device {
    pub directory: tempfile::TempDir,
    pub client: Client,
}

impl Device {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let client = Self::open(directory.path(), SupportedVersions::THIS_BUILD);
        Self { directory, client }
    }

    pub fn open(directory: &std::path::Path, versions: SupportedVersions) -> Client {
        let mut options = ClientOptions::new(directory.join("junto"));
        options.versions = versions;
        Client::open(options).unwrap()
    }

    /// A new client on the same data directory, as after a restart of the application.
    pub fn restarted(&self) -> Client {
        Self::open(self.directory.path(), SupportedVersions::THIS_BUILD)
    }

    pub async fn add(&self, address: &str) -> ServerHandle {
        add(&self.client, address).await
    }
}

pub async fn add(client: &Client, address: &str) -> ServerHandle {
    let flow = client.add_server();
    flow.dispatch(AddServerAction::Fetch {
        address: address.to_owned(),
    })
    .await
    .unwrap();
    flow.dispatch(AddServerAction::Save {
        connection_mode: Default::default(),
    })
    .await
    .unwrap();
    let AddServerState::Saved { server_id } = flow.view().state else {
        panic!("the server was not saved");
    };
    client.server(server_id).unwrap()
}

pub fn claim(server: &TestServer, name: &str) -> AccessAction {
    AccessAction::ClaimOwner {
        code: OwnerCode::new(server.owner_code().expect("an owner code")),
        name: name.to_owned(),
        device_name: "First device".to_owned(),
    }
}

pub fn join(code: &InviteCode, name: &str) -> AccessAction {
    AccessAction::Join {
        code: code.clone(),
        name: name.to_owned(),
        device_name: "Second device".to_owned(),
    }
}

#[track_caller]
pub fn logged_in(handle: &ServerHandle) -> AccountId {
    match handle.access().state {
        AccessState::LoggedIn { account_id, .. } => account_id,
        other => panic!("not logged in: {other:?}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Answers `busy` and does not pass the request on.
    Busy,
    /// Passes the request on, waits for the answer of the server and drops it.
    LoseAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    /// The status and the body that the client got. Absent while the server works and
    /// when the answer was dropped.
    pub answer: Option<(u16, Vec<u8>)>,
}

#[derive(Default)]
struct ProxyState {
    faults: Vec<(String, Vec<Fault>)>,
    seen: Vec<Seen>,
}

/// An HTTP proxy in front of one server. It passes every request on unless a fault is
/// planned for its path, and it records what the client sent.
pub struct FaultProxy {
    port: u16,
    state: Arc<Mutex<ProxyState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FaultProxy {
    /// The proxy at the address that the server lists as a public origin. A server has one.
    pub async fn start(server: &TestServer) -> Self {
        let listener = server
            .proxy
            .lock()
            .unwrap()
            .take()
            .expect("the proxy of this server was started before");
        listener.set_nonblocking(true).unwrap();
        Self::serve_on(TcpListener::from_std(listener).unwrap(), server.port)
    }

    /// A proxy at an address that the server does not list as a public origin.
    pub async fn start_unlisted(server: &TestServer) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        Self::serve_on(listener, server.port)
    }

    fn serve_on(listener: TcpListener, upstream_port: u16) -> Self {
        let port = listener.local_addr().unwrap().port();
        let state = Arc::<Mutex<ProxyState>>::default();
        let shared = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = shared.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, upstream_port, state).await;
                });
            }
        });
        Self { port, state, task }
    }

    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The next requests to `path` fail in the given ways, in order.
    pub fn fail(&self, path: &str, faults: impl IntoIterator<Item = Fault>) {
        let mut faults: Vec<Fault> = faults.into_iter().collect();
        faults.reverse();
        self.state
            .lock()
            .unwrap()
            .faults
            .push((path.to_owned(), faults));
    }

    /// Every request in the order of its arrival.
    pub fn all(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    pub fn seen(&self, path: &str) -> Vec<Seen> {
        self.state
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|seen| seen.path == path)
            .cloned()
            .collect()
    }
}

async fn serve(
    mut client: TcpStream,
    upstream_port: u16,
    state: Arc<Mutex<ProxyState>>,
) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    let (head_end, length) = loop {
        let count = client.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..count]);
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..position]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (position + 4, length);
        }
    };
    while request.len() < head_end + length {
        let count = client.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..count]);
    }
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let mut first_line = head.lines().next().unwrap_or_default().split(' ');
    let method = first_line.next().unwrap_or_default().to_owned();
    let path = first_line.next().unwrap_or_default().to_owned();
    let body = request[head_end..head_end + length].to_vec();

    let (index, fault) = {
        let mut state = state.lock().unwrap();
        state.seen.push(Seen {
            method,
            path: path.clone(),
            body,
            answer: None,
        });
        let index = state.seen.len() - 1;
        let fault = state
            .faults
            .iter_mut()
            .find(|(planned, faults)| *planned == path && !faults.is_empty())
            .and_then(|(_, faults)| faults.pop());
        (index, fault)
    };
    let answered = |status: u16, body: &[u8]| {
        state.lock().unwrap().seen[index].answer = Some((status, body.to_vec()));
    };
    if fault == Some(Fault::Busy) {
        let body = br#"{"error":{"code":"busy","message":"the proxy of the test answered"}}"#;
        let head = format!(
            "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\n\
             retry-after: 1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        answered(503, body);
        client.write_all(head.as_bytes()).await?;
        client.write_all(body).await?;
        return client.shutdown().await;
    }

    let mut forwarded = Vec::new();
    for line in head.trim_end().split("\r\n") {
        if !line.to_ascii_lowercase().starts_with("connection:") {
            forwarded.extend_from_slice(line.as_bytes());
            forwarded.extend_from_slice(b"\r\n");
        }
    }
    forwarded.extend_from_slice(b"connection: close\r\n\r\n");
    forwarded.extend_from_slice(&request[head_end..]);
    let mut upstream = TcpStream::connect(("127.0.0.1", upstream_port)).await?;
    upstream.write_all(&forwarded).await?;
    let mut response = Vec::new();
    upstream.read_to_end(&mut response).await?;
    if fault == Some(Fault::LoseAnswer) {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&response);
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    let body_start = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(response.len(), |position| position + 4);
    answered(status, &response[body_start..]);
    client.write_all(&response).await?;
    client.shutdown().await
}
