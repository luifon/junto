//! Starts the real server binary of this checkout on a database of its own, and a proxy
//! that can stand between a client and a server to make requests fail and to cut the
//! connections of the event stream.

use std::io::Read;
use std::net::{SocketAddr, TcpListener as StdListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use junto_client_core::address::ServerAddress;
use junto_client_core::api::{ReqwestTransport, RetryPolicy, server_info};
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::flow::add_server::{AddServerAction, AddServerState};
use junto_client_core::protocol::ids::AccountId;
use junto_client_core::protocol::secret::{InviteCode, OwnerCode};
use junto_client_core::stream::StreamPolicy;
use junto_client_core::version::SupportedVersions;
use junto_client_core::{Client, ClientOptions, ServerHandle};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const DATABASE_URL: &str = "JUNTO_TEST_DATABASE_URL";
const SERVER_BINARY: &str = "JUNTO_SERVER_BIN";

pub fn admin_url() -> String {
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
    /// The log of every process of this server, in order. A test that fails prints it
    /// (`Drop`), so the cause of an `internal` error that the server logged is in the output
    /// of that test.
    log: Arc<Mutex<String>>,
    /// One per thread that copies the output or the log of the running process: it
    /// disconnects when that thread has copied everything.
    copiers: Vec<std::sync::mpsc::Receiver<()>>,
    name: String,
    port: u16,
    proxy: Mutex<Option<StdListener>>,
    proxy_port: u16,
    /// The server uses the LiveKit server of `tools/with-test-livekit.sh`, and gives its
    /// clients this address of it.
    media: Option<String>,
    /// Settings of the process beside the ones of every test server.
    settings: Vec<(String, String)>,
    /// Where the server stores the files of messages.
    attachments: tempfile::TempDir,
    /// Where the server writes the files of exports.
    exports: tempfile::TempDir,
    /// Declared last: the drop ends the process and waits for it, then deletes the database.
    database: TestDatabase,
}

fn setting(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set. Run this test through tools/with-test-livekit.sh.")
    })
}

impl TestServer {
    pub async fn start(name: &str) -> Self {
        Self::start_as(name, None).await
    }

    /// A server with a media server, for the tests that need calls with people inside.
    pub async fn start_with_media(name: &str) -> Self {
        Self::start_with_media_at(name, &setting("JUNTO_TEST_LIVEKIT_URL")).await
    }

    /// A server with a media server whose clients reach it at `address`: the address of a
    /// proxy.
    pub async fn start_with_media_at(name: &str, address: &str) -> Self {
        Self::start_as(name, Some(address.to_owned())).await
    }

    /// A server with these settings of its process, such as `JUNTO_PICTURE_BYTES`. The
    /// window tests use it.
    #[allow(dead_code)]
    pub async fn start_with_settings(name: &str, settings: &[(&str, &str)]) -> Self {
        let settings = settings
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Self::start_configured(name, None, settings).await
    }

    async fn start_as(name: &str, media: Option<String>) -> Self {
        Self::start_configured(name, media, Vec::new()).await
    }

    async fn start_configured(
        name: &str,
        media: Option<String>,
        settings: Vec<(String, String)>,
    ) -> Self {
        let database = TestDatabase::create().await;
        let proxy = StdListener::bind(("127.0.0.1", 0)).unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        let mut server = Self {
            child: None,
            output: Arc::default(),
            log: Arc::default(),
            copiers: Vec::new(),
            name: name.to_owned(),
            port: free_port(),
            proxy: Mutex::new(Some(proxy)),
            proxy_port,
            media,
            settings,
            attachments: tempfile::tempdir().unwrap(),
            exports: tempfile::tempdir().unwrap(),
            database,
        };
        server.run(true).await;
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

    /// Starts the process and waits until it answers at its port. The port was free when
    /// it was chosen, and another process can take it before the server listens there:
    /// then the process is started again, at another port when `new_port` allows it.
    async fn run(&mut self, new_port: bool) {
        for _ in 0..8 {
            if self.launch().await && self.answers().await {
                return;
            }
            self.stop();
            if new_port {
                self.port = free_port();
            } else {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        panic!("the server did not start at its port");
    }

    /// True when the process that answers at the port of this server is this server: it
    /// reports the identifier that the database of this server holds.
    async fn answers(&self) -> bool {
        let options: PgConnectOptions = self.database_url().parse().unwrap();
        let Ok(mut database) = options.connect().await else {
            return false;
        };
        let stored: Option<uuid::Uuid> = sqlx::query_scalar("select server_id from server_state")
            .fetch_optional(&mut database)
            .await
            .unwrap_or(None);
        let transport = ReqwestTransport::new().unwrap();
        let address = ServerAddress::parse(&self.address()).unwrap();
        let policy = RetryPolicy::default();
        let asked = server_info(&transport, &address, &policy);
        let reported = tokio::time::timeout(Duration::from_secs(10), asked).await;
        matches!((stored, reported), (Some(stored), Ok(Ok(info)))
            if info.server_id.as_uuid() == stored)
    }

    /// Starts the process. False when it stopped at once or nothing listens at its port.
    async fn launch(&mut self) -> bool {
        let mut command = Command::new(server_binary());
        if let Some(address) = &self.media {
            command
                .env(
                    "JUNTO_LIVEKIT_API_URL",
                    setting("JUNTO_TEST_LIVEKIT_API_URL"),
                )
                .env("JUNTO_LIVEKIT_URL", address)
                .env(
                    "JUNTO_LIVEKIT_API_KEY",
                    setting("JUNTO_TEST_LIVEKIT_API_KEY"),
                )
                .env(
                    "JUNTO_LIVEKIT_API_SECRET",
                    setting("JUNTO_TEST_LIVEKIT_API_SECRET"),
                );
        }
        let mut child = command
            .env("JUNTO_DATABASE_URL", self.database_url())
            .env("JUNTO_LISTEN_ADDRESS", format!("127.0.0.1:{}", self.port))
            .env("JUNTO_SERVER_NAME", &self.name)
            .env(
                "JUNTO_PUBLIC_ORIGINS",
                format!(
                    "http://127.0.0.1:{},http://127.0.0.1:{}",
                    self.port, self.proxy_port
                ),
            )
            .env(
                "JUNTO_ATTACHMENTS_DIR",
                self.attachments.path().canonicalize().unwrap(),
            )
            .env(
                "JUNTO_EXPORTS_DIR",
                self.exports.path().canonicalize().unwrap(),
            )
            .env("JUNTO_LOG_FORMAT", "text")
            .env("JUNTO_LOG_LEVEL", "warn")
            // The server stops when the process of the tests ends, also when that process is
            // killed (ADR 0915).
            .env("JUNTO_STOP_WITH_PROCESS", std::process::id().to_string())
            .envs(self.settings.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        self.output = Arc::default();
        let echo = std::env::var_os("JUNTO_TEST_SERVER_LOG").is_some();
        self.copiers = vec![
            copy_into(child.stdout.take().unwrap(), self.output.clone(), false),
            copy_into(child.stderr.take().unwrap(), self.log.clone(), echo),
        ];
        self.child = Some(child);
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        for _ in 0..600 {
            if TcpStream::connect(address).await.is_ok() {
                return true;
            }
            if self.child.as_mut().unwrap().try_wait().unwrap().is_some() {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn database_url(&self) -> &str {
        &self.database.url
    }

    pub fn database(&self) -> &TestDatabase {
        &self.database
    }

    /// The number of the running process.
    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// The owner code that the running process printed. A server with an owner prints none.
    pub fn owner_code(&self) -> Option<String> {
        code_in(&self.output.lock().unwrap())
    }

    /// Runs a host command of the server binary on the database of this server (ADR 0114).
    pub fn host_command(&self, arguments: &[&str]) -> String {
        let output = Command::new(server_binary())
            .args(arguments)
            .env("JUNTO_DATABASE_URL", self.database_url())
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
        // A process that another test started can hold the write end of a pipe of this
        // server, so the end of the copy is awaited for a bounded time only.
        for copier in self.copiers.drain(..) {
            let _ = copier.recv_timeout(Duration::from_secs(5));
        }
    }

    /// What the processes of this server have logged so far.
    pub fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }

    /// Starts a new process on the same database and the same port.
    pub async fn restart(&mut self) {
        self.stop();
        self.run(false).await;
    }
}

/// The database of a test server. It does not record statement statistics, and it is
/// deleted when it is dropped: the database server holds the statistics of every table and
/// index of every database in a shared memory of fixed size, and the tests of the server
/// that read statement statistics run on the same database server.
pub struct TestDatabase {
    pub name: String,
    pub url: String,
    /// How the deletion ended, once it has run.
    deletion: Arc<Mutex<Option<Result<Deleted, String>>>>,
}

/// How a test database was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    /// No session used the database.
    Unused,
    /// Sessions still used the database, and the deletion ended them.
    Forced,
}

impl TestDatabase {
    /// Creates a database. The value exists from the moment the database exists, so a
    /// failure of a later step deletes it.
    pub async fn create() -> Self {
        let name = format!("junto_client_{}", uuid::Uuid::new_v4().simple());
        let admin = PgConnectOptions::from_str(&admin_url()).unwrap();
        let mut connection = admin.connect().await.unwrap();
        for statement in [
            "set pg_stat_statements.track = 'none'".to_owned(),
            format!("create database {name}"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&mut connection)
                .await
                .unwrap();
        }
        let mut created = Self {
            name,
            url: String::new(),
            deletion: Arc::default(),
        };
        let tracked = format!(
            "alter database {} set pg_stat_statements.track = 'none'",
            created.name
        );
        sqlx::query(sqlx::AssertSqlSafe(tracked))
            .execute(&mut connection)
            .await
            .unwrap();
        drop(connection);
        let mut url = url::Url::parse(&admin_url()).unwrap();
        url.set_path(&created.name);
        created.url = url.into();
        created
    }

    /// How the deletion of this database ended, once it has run.
    pub fn deletion(&self) -> Arc<Mutex<Option<Result<Deleted, String>>>> {
        self.deletion.clone()
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let deleted = delete_database(self.name.clone(), DATABASE_CLEANUP_LIMIT);
        if let Err(problem) = &deleted {
            eprintln!("the test database {} was not deleted: {problem}", self.name);
        }
        *self.deletion.lock().unwrap() = Some(deleted);
    }
}

/// The longest time that the deletion of a test database may take. A database server that
/// does not answer would otherwise hold the test, its run and the lock of the machine.
const DATABASE_CLEANUP_LIMIT: Duration = Duration::from_secs(30);

/// Deletes a test database through a connection of its own, within `limit`. The deletion
/// waits for sessions that still use the database as PostgreSQL does (5 seconds), then ends
/// them. A drop cannot wait for a future on the runtime of its test, and a window test runs
/// on no runtime, so the work runs on a thread with its own runtime.
pub fn delete_database(name: String, limit: Duration) -> Result<Deleted, String> {
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let deleted = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .and_then(|runtime| {
                runtime.block_on(async {
                    let deleting = async {
                        let admin = PgConnectOptions::from_str(&admin_url())?;
                        let mut connection = admin.connect().await?;
                        sqlx::query("set pg_stat_statements.track = 'none'")
                            .execute(&mut connection)
                            .await?;
                        let unused = format!("drop database if exists {name}");
                        let deleted = match sqlx::query(sqlx::AssertSqlSafe(unused))
                            .execute(&mut connection)
                            .await
                        {
                            Ok(_) => Deleted::Unused,
                            Err(error)
                                if error.as_database_error().and_then(|e| e.code()).as_deref()
                                    == Some("55006") =>
                            {
                                let forced = format!("drop database if exists {name} with (force)");
                                sqlx::query(sqlx::AssertSqlSafe(forced))
                                    .execute(&mut connection)
                                    .await?;
                                Deleted::Forced
                            }
                            Err(error) => return Err(error),
                        };
                        sqlx::Connection::close(connection).await?;
                        Ok(deleted)
                    };
                    match tokio::time::timeout(limit, deleting).await {
                        Ok(deleted) => deleted.map_err(|error| error.to_string()),
                        Err(_) => Err(format!("it did not end within {limit:?}")),
                    }
                })
            });
        let _ = done.send(deleted);
    });
    ended
        .recv_timeout(limit + Duration::from_secs(5))
        .map_err(|_| format!("it did not end within {limit:?}"))?
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
        if std::thread::panicking() && std::env::var_os("JUNTO_TEST_SERVER_LOG").is_none() {
            let log = self.log();
            if !log.is_empty() {
                eprintln!("---- log of the server {} ----\n{log}", self.name);
            }
        }
    }
}

/// Copies what a process writes on one of its outputs into `into` until the process ends,
/// and also into the standard error of the process of the tests, past the capture of the
/// test runner, when `echo` is set. The returned receiver disconnects when the copy ends.
fn copy_into(
    mut from: impl Read + Send + 'static,
    into: Arc<Mutex<String>>,
    echo: bool,
) -> std::sync::mpsc::Receiver<()> {
    let (copying, copied) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _copying = copying;
        let mut buffer = [0u8; 1024];
        while let Ok(count @ 1..) = from.read(&mut buffer) {
            if echo {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &buffer[..count]);
            }
            into.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buffer[..count]));
        }
    });
    copied
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
        // A lost connection of the event stream is tried again sooner and more often than
        // in the product, because the tests cut connections on purpose.
        options.stream = StreamPolicy {
            first_delay: Duration::from_millis(200),
            longest_delay: Duration::from_secs(2),
            connections_per_minute: 120,
            send_again_after: Duration::from_secs(2),
            list_interval: Duration::from_millis(200),
            ..StreamPolicy::default()
        };
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
    /// Ends the connection without passing the request on.
    Cut,
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
    /// Faults for the requests whose path ends with a text.
    faults_ending: Vec<(String, Vec<Fault>)>,
    seen: Vec<Seen>,
    /// An upgrade request is not passed on: its connection is closed.
    streams_blocked: bool,
    /// A request to one of these paths waits at the proxy until the test releases it.
    held: Vec<String>,
    /// A request whose path ends with one of these texts waits too.
    held_ending: Vec<String>,
    /// A request with this method whose path without its query ends with this text waits
    /// too.
    held_requests: Vec<(String, String)>,
    /// The answer to a request with this method whose path without its query ends with
    /// this text waits at the proxy, after the server gave it.
    held_answers: Vec<(String, String)>,
    /// The answer to the request with this index of `seen` waits at the proxy whatever
    /// `held_answers` names.
    kept_answer: Option<usize>,
    /// How many answers wait at the proxy now.
    answers_waiting: usize,
    /// What the server sends on the event stream waits at the proxy.
    events_held: bool,
}

/// An HTTP proxy in front of one server. It passes every request on unless a fault is
/// planned for its path, and it records what the client sent.
pub struct FaultProxy {
    port: u16,
    state: Arc<Mutex<ProxyState>>,
    /// A new value ends every upgraded connection that the proxy carries.
    cut: watch::Sender<u64>,
    /// A new value passes on every request that the proxy holds.
    released: watch::Sender<u64>,
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

    /// The proxy on `listener`, in front of the server at `upstream_port`: a server of the
    /// tests that runs in the test process lists the address of `listener` as a public
    /// origin.
    #[allow(dead_code)]
    pub async fn start_on(listener: StdListener, upstream_port: u16) -> Self {
        listener.set_nonblocking(true).unwrap();
        Self::serve_on(TcpListener::from_std(listener).unwrap(), upstream_port)
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
        let cut = watch::Sender::new(0);
        let cuts = cut.clone();
        let released = watch::Sender::new(0);
        let releases = released.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (state, cut) = (shared.clone(), cuts.subscribe());
                let released = releases.subscribe();
                tokio::spawn(async move {
                    let _ = serve(stream, upstream_port, state, cut, released).await;
                });
            }
        });
        Self {
            port,
            state,
            cut,
            released,
            task,
        }
    }

    /// Ends every connection of the event stream that passes through the proxy, without a
    /// close frame, as a network that fails does.
    pub fn cut_streams(&self) {
        self.cut.send_modify(|cuts| *cuts += 1);
    }

    /// From now on a request to `path` is recorded and then waits at the proxy until
    /// [`FaultProxy::release`]. A test uses it to act while a request is on its way.
    pub fn hold(&self, path: &str) {
        self.state.lock().unwrap().held.push(path.to_owned());
    }

    /// As [`FaultProxy::hold`], for every request whose path ends with `end`: a path with
    /// an identifier that the test does not know yet.
    pub fn hold_ending(&self, end: &str) {
        self.state.lock().unwrap().held_ending.push(end.to_owned());
    }

    /// As [`FaultProxy::hold_ending`], only for the requests with `method`, and with the
    /// path compared without its query.
    #[allow(dead_code)]
    pub fn hold_request(&self, method: &str, end: &str) {
        self.state
            .lock()
            .unwrap()
            .held_requests
            .push((method.to_owned(), end.to_owned()));
    }

    /// From now on the answer to a request with `method` whose path without its query ends
    /// with `end` waits at the proxy until [`FaultProxy::release`]. The server has handled the request by
    /// then. A test of the window uses it to act between a change on the server and its
    /// answer.
    #[allow(dead_code)]
    pub fn hold_answer_ending(&self, method: &str, end: &str) {
        self.state
            .lock()
            .unwrap()
            .held_answers
            .push((method.to_owned(), end.to_owned()));
    }

    /// Passes on every answer that waits and holds no further one, except the answer to the
    /// request with this index of [`FaultProxy::all`], which waits until
    /// [`FaultProxy::release`]. A test of the window uses it to bring an answer after the
    /// answers of later requests.
    #[allow(dead_code)]
    pub fn release_answers_except(&self, kept: usize) {
        {
            let mut state = self.state.lock().unwrap();
            state.held_answers.clear();
            state.kept_answer = Some(kept);
        }
        self.released.send_modify(|releases| *releases += 1);
    }

    /// How many answers of the server wait at the proxy now.
    #[allow(dead_code)]
    pub fn answers_waiting(&self) -> usize {
        self.state.lock().unwrap().answers_waiting
    }

    /// From now on what the server sends on every connection of the event stream waits at
    /// the proxy, in its order, until [`FaultProxy::release`]. A test of the window uses it
    /// to bring a change of the state after the answer to the request that made it.
    #[allow(dead_code)]
    pub fn hold_events(&self) {
        self.state.lock().unwrap().events_held = true;
    }

    /// Passes on the events that wait and holds no further one. Requests and answers that
    /// wait keep waiting.
    #[allow(dead_code)]
    pub fn release_events(&self) {
        self.state.lock().unwrap().events_held = false;
        self.released.send_modify(|releases| *releases += 1);
    }

    /// Passes on the requests that [`FaultProxy::hold_request`] holds, and holds no further
    /// one. Answers and events that wait keep waiting.
    #[allow(dead_code)]
    pub fn release_requests(&self) {
        self.state.lock().unwrap().held_requests.clear();
        self.released.send_modify(|releases| *releases += 1);
    }

    /// Passes on held answers while requests and events keep waiting.
    #[allow(dead_code)]
    pub fn release_answers(&self) {
        self.state.lock().unwrap().held_answers.clear();
        self.released.send_modify(|releases| *releases += 1);
    }

    /// Passes on every request, every answer and the events that wait, and holds no further
    /// one.
    pub fn release(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.held.clear();
            state.held_ending.clear();
            state.held_requests.clear();
            state.held_answers.clear();
            state.kept_answer = None;
            state.events_held = false;
        }
        self.released.send_modify(|releases| *releases += 1);
    }

    /// While blocked, a new connection of the event stream is closed at once.
    pub fn block_streams(&self, blocked: bool) {
        self.state.lock().unwrap().streams_blocked = blocked;
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

    /// The next requests whose path ends with `end` fail in the given ways, in order.
    pub fn fail_ending(&self, end: &str, faults: impl IntoIterator<Item = Fault>) {
        let mut faults: Vec<Fault> = faults.into_iter().collect();
        faults.reverse();
        self.state
            .lock()
            .unwrap()
            .faults_ending
            .push((end.to_owned(), faults));
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
    mut cut: watch::Receiver<u64>,
    mut released: watch::Receiver<u64>,
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

    let upgrade = head
        .to_ascii_lowercase()
        .lines()
        .any(|line| line.starts_with("upgrade:") && line.contains("websocket"));
    if upgrade {
        let blocked = {
            let mut state = state.lock().unwrap();
            state.seen.push(Seen {
                method,
                path,
                body,
                answer: None,
            });
            state.streams_blocked
        };
        if blocked {
            return Ok(());
        }
        // The upgraded connection is carried as bytes in both directions until one side
        // ends it or the test cuts it. What the server sends waits while the test holds
        // the events.
        let mut upstream = TcpStream::connect(("127.0.0.1", upstream_port)).await?;
        upstream.write_all(&request).await?;
        cut.borrow_and_update();
        released.borrow_and_update();
        let (mut from_client, mut to_client) = client.split();
        let (mut from_server, mut to_server) = upstream.split();
        let up = async {
            tokio::io::copy(&mut from_client, &mut to_server).await?;
            to_server.shutdown().await
        };
        let down = async {
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                let count = from_server.read(&mut buffer).await?;
                if count == 0 {
                    return to_client.shutdown().await;
                }
                while state.lock().unwrap().events_held {
                    if released.changed().await.is_err() {
                        return Ok(());
                    }
                }
                to_client.write_all(&buffer[..count]).await?;
            }
        };
        tokio::select! {
            carried = async { tokio::try_join!(up, down) } => {
                carried?;
            }
            _ = cut.changed() => {}
        }
        return Ok(());
    }

    let (index, fault) = {
        let mut state = state.lock().unwrap();
        state.seen.push(Seen {
            method: method.clone(),
            path: path.clone(),
            body,
            answer: None,
        });
        let index = state.seen.len() - 1;
        let state = &mut *state;
        let fault = state
            .faults
            .iter_mut()
            .find(|(planned, faults)| *planned == path && !faults.is_empty())
            .or_else(|| {
                state
                    .faults_ending
                    .iter_mut()
                    .find(|(end, faults)| path.ends_with(end.as_str()) && !faults.is_empty())
            })
            .and_then(|(_, faults)| faults.pop());
        (index, fault)
    };
    let answered = |status: u16, body: &[u8]| {
        state.lock().unwrap().seen[index].answer = Some((status, body.to_vec()));
    };
    released.borrow_and_update();
    let bare = path.split('?').next().unwrap_or_default();
    let held = |state: &ProxyState| {
        state.held.contains(&path)
            || state.held_ending.iter().any(|end| path.ends_with(end))
            || state
                .held_requests
                .iter()
                .any(|(of, end)| *of == method && bare.ends_with(end.as_str()))
    };
    while held(&state.lock().unwrap()) {
        if released.changed().await.is_err() {
            return Ok(());
        }
    }
    if fault == Some(Fault::Cut) {
        return Ok(());
    }
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
    let answer_held = |state: &ProxyState| {
        state.kept_answer == Some(index)
            || state
                .held_answers
                .iter()
                .any(|(of, end)| *of == method && bare.ends_with(end.as_str()))
    };
    if answer_held(&state.lock().unwrap()) {
        state.lock().unwrap().answers_waiting += 1;
        let mut open = true;
        while open && answer_held(&state.lock().unwrap()) {
            open = released.changed().await.is_ok();
        }
        state.lock().unwrap().answers_waiting -= 1;
        if !open {
            return Ok(());
        }
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
