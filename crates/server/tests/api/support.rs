use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use ed25519_dalek::{Signer, SigningKey};
use junto_protocol::auth::{Challenge, ChallengeOperation, CreateChallenge, Proof, SessionCreated};
use junto_protocol::error::{ErrorBody, ErrorCode};
use junto_protocol::ids::{AccountId, DeviceId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::invite::InviteCreated;
use junto_protocol::keys::{PublicKey, Signature};
use junto_protocol::origin::Origin;
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::server::ServerInfo;
use junto_protocol::structure::Workspace;
use junto_protocol::{ServerPermission, WorkspacePermission};
use junto_server::Tasks;
use junto_server::clock::Clock;
use junto_server::config::{Config, Limits, LogFormat, PublicOrigins, RecorderSettings};
use junto_server::database::{CommitGate, Database};
use junto_server::media::MediaServer;
use junto_server::media::fake::FakeMedia;
use junto_server::recorder::fake::FakeRecorder;
use junto_server::recordings::Backend;
use junto_server::state::AppState;
use junto_server::storage::FakeDisk;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::postgres::PgConnectOptions;
use sqlx::{ConnectOptions, Connection, PgPool};
use time::{Duration, OffsetDateTime};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const DATABASE_URL: &str = "JUNTO_TEST_DATABASE_URL";

/// The public origin of every test server: the address that a device connected to and signs.
pub fn origin() -> Origin {
    Origin::parse("https://chat.example.test").unwrap()
}

/// The clock of a test server. It stands still until a test moves it; a clock that
/// `follows` the time of the machine is for the tests against a real media server, whose
/// tokens carry real times.
pub struct ManualClock {
    now: Mutex<OffsetDateTime>,
    follows: Option<(OffsetDateTime, std::time::Instant)>,
}

impl ManualClock {
    fn starting_at(now: OffsetDateTime, follows: bool) -> Self {
        Self {
            now: Mutex::new(now),
            follows: follows.then(|| (now, std::time::Instant::now())),
        }
    }

    pub fn advance(&self, duration: Duration) {
        *self.now.lock().unwrap() += duration;
    }

    /// The time as the server stores it: cut to the microsecond.
    pub fn now(&self) -> OffsetDateTime {
        let mut now = *self.now.lock().unwrap();
        if let Some((_, started)) = self.follows {
            now += started.elapsed();
        }
        now.replace_nanosecond(now.microsecond() * 1_000).unwrap()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> OffsetDateTime {
        ManualClock::now(self)
    }
}

pub struct DeviceKey(SigningKey);

impl DeviceKey {
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).unwrap();
        Self(SigningKey::from_bytes(&seed))
    }

    pub fn public(&self) -> PublicKey {
        PublicKey::from_bytes(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, challenge: &Challenge) -> Signature {
        self.sign_for(challenge, &origin())
    }

    /// Signs as a device that connected to `origin`.
    pub fn sign_for(&self, challenge: &Challenge, origin: &Origin) -> Signature {
        Signature::from_bytes(self.0.sign(&challenge.signing_input(origin)).to_bytes())
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub retry_after: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    /// Everything a caller can observe, except the date of the response.
    pub fn observable(&self) -> (u16, Vec<(String, String)>, String) {
        (self.status.as_u16(), self.headers.clone(), self.text())
    }

    pub fn code(&self) -> ErrorCode {
        let body: ErrorBody = serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("{} {error}: {}", self.status, self.text()));
        body.error.code
    }

    #[track_caller]
    pub fn ok<T: DeserializeOwned>(&self, status: StatusCode) -> T {
        assert_eq!(self.status, status, "{}", self.text());
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("{error}: {}", self.text()))
    }

    #[track_caller]
    pub fn empty(&self, status: StatusCode) {
        assert_eq!(self.status, status, "{}", self.text());
        assert!(self.body.is_empty(), "{}", self.text());
    }

    #[track_caller]
    pub fn error(&self, code: ErrorCode) {
        let body: ErrorBody = serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("{} {error}: {}", self.status, self.text()));
        assert_eq!(body.error.code, code, "{}", self.text());
        assert_eq!(self.status.as_u16(), code.http_status());
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An account with one device and a session.
pub struct Member {
    pub key: DeviceKey,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub token: String,
}

pub struct TestServer {
    pub base: String,
    pub address: SocketAddr,
    pub db: PgPool,
    pub clock: Arc<ManualClock>,
    pub owner_code: String,
    pub info: ServerInfo,
    pub state: AppState,
    /// The media server of a server that was started for calls.
    pub media: Option<Arc<FakeMedia>>,
    /// The recorder and the disk of a server that was started for calls.
    pub recorder: Option<Arc<FakeRecorder>>,
    pub disk: Option<Arc<FakeDisk>>,
    /// The directory of the recordings of a server that was started for calls. It is
    /// deleted when the last server process on it is dropped.
    scratch: Option<Arc<Scratch>>,
    http: reqwest::Client,
    database: PgConnectOptions,
    config: Config,
    send_buffer: Option<u32>,
    tasks: Tasks,
    shutdown: Option<oneshot::Sender<()>>,
    served: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

/// The fake media server of a test server, with the fake recorder that puts its jobs into
/// the rooms of that media server, and the fake disk of the recordings.
#[derive(Clone)]
struct Fakes {
    media: Arc<FakeMedia>,
    recorder: Arc<FakeRecorder>,
    disk: Arc<FakeDisk>,
    scratch: Option<Arc<Scratch>>,
}

/// A directory of one test that is deleted with its content when it is dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new() -> Self {
        let scratch = Self(scratch_path());
        std::fs::create_dir_all(&scratch.0).unwrap();
        scratch
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch_path() -> PathBuf {
    std::env::temp_dir().join(format!("junto-test-{}", uuid::Uuid::new_v4().simple()))
}

/// The free space of the fake disk of a test server at its start.
pub const FREE_DISK: u64 = 1 << 40;

/// Where the recorder of a test server writes: a new directory of the test, which the
/// recorder names `/out`.
pub fn recorder_settings() -> RecorderSettings {
    RecorderSettings {
        directory: scratch_path(),
        recorder_directory: "/out".to_owned(),
        layout_url: "http://server.example.test/layout/".to_owned(),
        ffmpeg: "ffmpeg".into(),
        ffprobe: "ffprobe".into(),
        transcriber: None,
    }
}

/// The media server of a test server.
enum Media {
    /// The one of the configuration: none, or a LiveKit server.
    Configured,
    /// A fake one: a new one, or the one of the server that is restarted.
    Fake(Option<Fakes>),
}

/// How a test server is started.
struct Setup {
    send_buffer: Option<u32>,
    media: Media,
    tasks: Tasks,
    follows_time: bool,
    listen: Option<String>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            send_buffer: None,
            media: Media::Configured,
            tasks: Tasks { calls: false },
            follows_time: false,
            listen: None,
        }
    }
}

fn admin_options() -> PgConnectOptions {
    let url = std::env::var(DATABASE_URL).unwrap_or_else(|_| {
        panic!(
            "{DATABASE_URL} is not set. Run the tests through tools/with-test-postgres.sh, \
             as crates/server/README.md describes."
        )
    });
    PgConnectOptions::from_str(&url).unwrap()
}

pub fn test_config() -> Config {
    Config {
        attachments_directory: None,
        recorder_layout_directory: None,
        database_url: String::new(),
        listen_address: SocketAddr::from(([127, 0, 0, 1], 0)),
        public_origins: PublicOrigins::of(vec![origin()]),
        server_name: "Acme".to_owned(),
        trusted_proxies: Vec::new(),
        log_format: LogFormat::Text,
        log_level: tracing::Level::ERROR,
        media: None,
        recorder: None,
        limits: Limits::default(),
    }
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// Starts a server on its own new database.
    pub async fn start_with(configure: impl FnOnce(&mut Config)) -> Self {
        Self::start_tuned(configure, None).await
    }

    /// Like [`TestServer::start_with`]. With `send_buffer`, the sockets of the server hold at
    /// most about that many unsent bytes, so that a client that does not read is noticed
    /// after little data.
    pub async fn start_tuned(
        configure: impl FnOnce(&mut Config),
        send_buffer: Option<u32>,
    ) -> Self {
        let setup = Setup {
            send_buffer,
            ..Setup::default()
        };
        Self::start_as(configure, setup).await
    }

    /// Starts a server with a fake media server. The admission check and the publication of
    /// speaking marks do not run by themselves: a test runs each check, so that the order of
    /// things is fixed.
    pub async fn start_for_calls() -> Self {
        Self::start_for_calls_with(|_| {}).await
    }

    pub async fn start_for_calls_with(configure: impl FnOnce(&mut Config)) -> Self {
        let setup = Setup {
            media: Media::Fake(None),
            ..Setup::default()
        };
        let server = Self::start_as(configure, setup).await;
        server.check().await;
        server
    }

    /// Starts a server with a fake media server and the tasks of calls running as in the
    /// product.
    pub async fn start_with_tasks(configure: impl FnOnce(&mut Config)) -> Self {
        let setup = Setup {
            media: Media::Fake(None),
            tasks: Tasks { calls: true },
            ..Setup::default()
        };
        Self::start_as(configure, setup).await
    }

    /// Starts a server as the product runs: with the media server of its configuration, the
    /// tasks of calls, and a clock that follows the time of the machine. With `listen`, the
    /// server listens on that address.
    pub async fn start_live(configure: impl FnOnce(&mut Config), listen: Option<String>) -> Self {
        let setup = Setup {
            tasks: Tasks { calls: true },
            follows_time: true,
            listen,
            ..Setup::default()
        };
        Self::start_as(configure, setup).await
    }

    async fn start_as(configure: impl FnOnce(&mut Config), setup: Setup) -> Self {
        let admin = admin_options();
        let name = format!("junto_test_{}", uuid::Uuid::new_v4().simple());
        let mut connection = admin.connect().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("create database {name}")))
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let mut config = test_config();
        if matches!(setup.media, Media::Fake(_)) {
            config.recorder = Some(recorder_settings());
        }
        configure(&mut config);
        Self::on_database(admin.database(&name), config, None, setup).await
    }

    /// Starts a second server process on the database of this one.
    pub async fn restart(&self) -> Self {
        let media = self.fakes();
        let setup = Setup {
            send_buffer: self.send_buffer,
            media,
            tasks: self.tasks,
            ..Setup::default()
        };
        let now = Some(self.clock.now());
        Self::on_database(self.database.clone(), self.config.clone(), now, setup).await
    }

    /// Starts a second server process on the database of this one, with a changed
    /// configuration. A test that shortens a time limit prepares on the first process, under
    /// the normal limits, and sends only the step that it examines to this one.
    pub async fn strict(&self, configure: impl FnOnce(&mut Config)) -> Self {
        let media = self.fakes();
        let setup = Setup {
            send_buffer: self.send_buffer,
            media,
            tasks: self.tasks,
            ..Setup::default()
        };
        let mut config = self.config.clone();
        configure(&mut config);
        let now = Some(self.clock.now());
        Self::on_database(self.database.clone(), config, now, setup).await
    }

    /// Connects to the database of this server as a server process with a changed
    /// configuration does, and starts no process.
    pub async fn database_with(&self, configure: impl FnOnce(&mut Config)) -> Database {
        let mut config = self.config.clone();
        configure(&mut config);
        let limits = &config.limits;
        Database::connect(
            self.database.clone(),
            4,
            &limits.database,
            &limits.stream.pool(),
        )
        .await
        .unwrap()
    }

    fn fakes(&self) -> Media {
        match (&self.media, &self.recorder, &self.disk) {
            (Some(media), Some(recorder), Some(disk)) => Media::Fake(Some(Fakes {
                media: media.clone(),
                recorder: recorder.clone(),
                disk: disk.clone(),
                scratch: self.scratch.clone(),
            })),
            _ => Media::Configured,
        }
    }

    /// Stops this server process as a stop signal does and waits until it has ended.
    pub async fn stop(&mut self) {
        drop(self.shutdown.take());
        if let Some(served) = self.served.take() {
            served.await.unwrap().unwrap();
        }
    }

    async fn on_database(
        database: PgConnectOptions,
        config: Config,
        now: Option<OffsetDateTime>,
        setup: Setup,
    ) -> Self {
        let Setup {
            send_buffer,
            media,
            tasks,
            follows_time,
            listen,
        } = setup;
        let limits = &config.limits;
        let stream_pool = limits.stream.pool();
        let connected = Database::connect(database.clone(), 8, &limits.database, &stream_pool)
            .await
            .unwrap();
        let db = connected.pool().clone();
        let clock = Arc::new(ManualClock::starting_at(
            now.unwrap_or_else(OffsetDateTime::now_utc),
            follows_time,
        ));
        let fakes = match media {
            Media::Configured => None,
            Media::Fake(Some(fakes)) => Some(fakes),
            Media::Fake(None) => {
                let media = Arc::new(FakeMedia::new(clock.clone()));
                let recorder = Arc::new(FakeRecorder::new(clock.clone(), media.clone()));
                let scratch = config.recorder.as_ref().map(|settings| {
                    std::fs::create_dir_all(&settings.directory).unwrap();
                    recorder.write_files(&settings.recorder_directory, &settings.directory);
                    Arc::new(Scratch(settings.directory.clone()))
                });
                Some(Fakes {
                    recorder,
                    disk: Arc::new(FakeDisk::new(FREE_DISK)),
                    media,
                    scratch,
                })
            }
        };
        let prepared = match &fakes {
            Some(fakes) => {
                let media: Arc<dyn MediaServer> = fakes.media.clone();
                let backend = config.recorder.clone().map(|settings| {
                    let limits = &config.limits.recordings;
                    Backend::new(fakes.recorder.clone(), fakes.disk.clone(), settings, limits)
                });
                let (config, clock) = (config.clone(), clock.clone());
                junto_server::prepare_with(connected, config, clock, Some(media), backend).await
            }
            None => junto_server::prepare(connected, config.clone(), clock.clone()).await,
        }
        .unwrap();
        let (media, recorder, disk, scratch) = match fakes {
            Some(fakes) => (
                Some(fakes.media),
                Some(fakes.recorder),
                Some(fakes.disk),
                fakes.scratch,
            ),
            None => (None, None, None, None),
        };
        let state = prepared.state.clone();
        let owner_code = prepared
            .owner_code
            .map(|code| code.expose().to_owned())
            .unwrap_or_default();

        let listener = match send_buffer {
            None => TcpListener::bind(listen.as_deref().unwrap_or("127.0.0.1:0"))
                .await
                .unwrap(),
            Some(bytes) => {
                let socket = tokio::net::TcpSocket::new_v4().unwrap();
                socket.set_send_buffer_size(bytes).unwrap();
                socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
                socket.listen(1024).unwrap()
            }
        };
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}");
        let (shutdown, stopped) = oneshot::channel::<()>();
        let stop = async move {
            let _ = stopped.await;
        };
        let served = tokio::spawn(junto_server::serve_with(
            listener,
            prepared.state,
            stop,
            tasks,
        ));

        let http = reqwest::Client::new();
        let info = http
            .get(format!("{base}/api/server"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        Self {
            base,
            address,
            db,
            clock,
            owner_code,
            info,
            state,
            media,
            recorder,
            disk,
            scratch,
            http,
            database,
            config,
            send_buffer,
            tasks,
            shutdown: Some(shutdown),
            served: Some(served),
        }
    }

    pub async fn send<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&B>,
        forwarded_for: Option<&str>,
    ) -> Reply {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(address) = forwarded_for {
            request = request.header("x-forwarded-for", address);
        }
        Self::reply(request).await
    }

    /// Sends `bytes` as the body, with `content_type` when one is given.
    pub async fn put_bytes(
        &self,
        member: &Member,
        path: &str,
        content_type: Option<&str>,
        bytes: Vec<u8>,
    ) -> Reply {
        let mut request = self
            .http
            .put(format!("{}{path}", self.base))
            .bearer_auth(&member.token)
            .body(bytes);
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        Self::reply(request).await
    }

    pub async fn reply(request: reqwest::RequestBuilder) -> Reply {
        let response = request.send().await.unwrap();
        let mut headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter(|(name, _)| name.as_str() != "date")
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        headers.sort();
        Reply {
            headers,
            status: response.status(),
            retry_after: response
                .headers()
                .get("retry-after")
                .map(|value| value.to_str().unwrap().to_owned()),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }

    pub fn gate(&self) -> &CommitGate {
        self.state.database.commit_gate()
    }

    /// Resolves when a statement of this server's database waits for a lock.
    pub async fn until_blocked(&self) {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "select count(*) from pg_stat_activity
                 where datname = current_database() and wait_event_type = 'Lock'",
            )
            .fetch_one(&self.db)
            .await
            .unwrap();
            if waiting > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub fn database_url(&self) -> String {
        self.database.to_url_lossy().to_string()
    }

    /// Runs the server binary with `arguments` against the database of this server.
    pub async fn host_command(&self, arguments: &[&str]) -> HostCommand {
        host_command_on(self.database_url(), arguments).await
    }
}

/// The database that the tests create their own databases from. No server has started on it.
pub fn unused_database_url() -> String {
    admin_options().to_url_lossy().to_string()
}

pub async fn host_command_on(database_url: String, arguments: &[&str]) -> HostCommand {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_junto-server"));
    command
        .args(arguments)
        .env_clear()
        .env("JUNTO_DATABASE_URL", database_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().unwrap();
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > std::time::Duration::from_secs(20) {
                child.kill().unwrap();
                panic!("the host command did not exit");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        HostCommand {
            code: output.status.code(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    })
    .await
    .unwrap()
}

impl TestServer {
    pub async fn get(&self, member: &Member, path: &str) -> Reply {
        self.send::<Value>(Method::GET, path, Some(&member.token), None, None)
            .await
    }

    pub async fn post<B: Serialize + ?Sized>(
        &self,
        member: &Member,
        path: &str,
        body: &B,
    ) -> Reply {
        self.send(Method::POST, path, Some(&member.token), Some(body), None)
            .await
    }

    pub async fn patch<B: Serialize + ?Sized>(
        &self,
        member: &Member,
        path: &str,
        body: &B,
    ) -> Reply {
        self.send(Method::PATCH, path, Some(&member.token), Some(body), None)
            .await
    }

    pub async fn put<B: Serialize + ?Sized>(&self, member: &Member, path: &str, body: &B) -> Reply {
        self.send(Method::PUT, path, Some(&member.token), Some(body), None)
            .await
    }

    pub async fn put_empty(&self, member: &Member, path: &str) -> Reply {
        self.send::<Value>(Method::PUT, path, Some(&member.token), None, None)
            .await
    }

    pub async fn delete(&self, member: &Member, path: &str) -> Reply {
        self.send::<Value>(Method::DELETE, path, Some(&member.token), None, None)
            .await
    }

    /// A request without a session.
    pub async fn open<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> Reply {
        self.send(Method::POST, path, None, Some(body), None).await
    }

    pub async fn challenge_reply(&self, key: &PublicKey, operation: ChallengeOperation) -> Reply {
        self.open(
            "/api/v1/auth/challenges",
            &CreateChallenge {
                public_key: *key,
                operation,
            },
        )
        .await
    }

    pub async fn challenge(&self, key: &DeviceKey, operation: ChallengeOperation) -> Challenge {
        self.challenge_reply(&key.public(), operation)
            .await
            .ok(StatusCode::CREATED)
    }

    pub async fn proof(&self, key: &DeviceKey, operation: ChallengeOperation) -> Proof {
        let challenge = self.challenge(key, operation).await;
        Proof {
            challenge_id: challenge.id,
            origin: origin(),
            signature: key.sign(&challenge),
        }
    }

    pub async fn login_reply(&self, key: &DeviceKey) -> Reply {
        let proof = self.proof(key, ChallengeOperation::Login).await;
        self.open("/api/v1/auth/sessions", &json!({ "proof": proof }))
            .await
    }

    pub async fn login(&self, key: &DeviceKey) -> SessionCreated {
        self.login_reply(key).await.ok(StatusCode::CREATED)
    }

    pub async fn claim_owner_reply(&self, key: &DeviceKey, code: &str) -> Reply {
        let proof = self.proof(key, ChallengeOperation::ClaimOwner).await;
        self.open(
            "/api/v1/owner/claim",
            &json!({ "proof": proof, "owner_code": code, "name": "Owner", "device_name": "desk" }),
        )
        .await
    }

    pub async fn owner(&self) -> Member {
        let key = DeviceKey::generate();
        let session: SessionCreated = self
            .claim_owner_reply(&key, &self.owner_code)
            .await
            .ok(StatusCode::CREATED);
        member(key, session)
    }

    pub async fn invite(&self, inviter: &Member, body: Value) -> InviteCreated {
        self.post(inviter, "/api/v1/invites", &body)
            .await
            .ok(StatusCode::CREATED)
    }

    pub async fn join_reply(&self, key: &DeviceKey, code: &str, name: &str) -> Reply {
        let proof = self.proof(key, ChallengeOperation::Join).await;
        self.open(
            "/api/v1/join",
            &json!({ "proof": proof, "invite_code": code, "name": name }),
        )
        .await
    }

    /// A new account that joins the default workspace with a fresh invite of `inviter`.
    pub async fn join(&self, inviter: &Member, name: &str) -> Member {
        self.join_into(inviter, name, &[]).await
    }

    pub async fn join_into(
        &self,
        inviter: &Member,
        name: &str,
        workspace_ids: &[WorkspaceId],
    ) -> Member {
        let invite = self
            .invite(inviter, json!({ "workspace_ids": workspace_ids }))
            .await;
        let key = DeviceKey::generate();
        let session: SessionCreated = self
            .join_reply(&key, invite.code.expose(), name)
            .await
            .ok(StatusCode::CREATED);
        member(key, session)
    }

    pub async fn default_workspace(&self, member: &Member) -> Workspace {
        let workspaces: Vec<Workspace> = self
            .get(member, "/api/v1/workspaces")
            .await
            .ok(StatusCode::OK);
        workspaces
            .into_iter()
            .find(|workspace| workspace.is_default)
            .unwrap()
    }

    /// Gives `target` a new server role with `permissions`.
    pub async fn grant_server(
        &self,
        granter: &Member,
        target: &Member,
        permissions: &[ServerPermission],
    ) -> ServerRoleId {
        let role: ServerRole = self
            .post(
                granter,
                "/api/v1/server-roles",
                &json!({ "name": "role", "permissions": permissions }),
            )
            .await
            .ok(StatusCode::CREATED);
        self.put_empty(
            granter,
            &format!(
                "/api/v1/accounts/{}/server-roles/{}",
                target.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
        role.id
    }

    /// Gives `target` a new role of `workspace_id` with `permissions`.
    pub async fn grant_workspace(
        &self,
        granter: &Member,
        target: &Member,
        workspace_id: WorkspaceId,
        permissions: &[WorkspacePermission],
    ) -> WorkspaceRoleId {
        let role: WorkspaceRole = self
            .post(
                granter,
                &format!("/api/v1/workspaces/{workspace_id}/roles"),
                &json!({ "name": "role", "permissions": permissions }),
            )
            .await
            .ok(StatusCode::CREATED);
        self.put_empty(
            granter,
            &format!(
                "/api/v1/workspaces/{workspace_id}/members/{}/roles/{}",
                target.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
        role.id
    }
}

pub struct HostCommand {
    pub code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl HostCommand {
    /// The owner code that the command printed.
    pub fn owner_code(&self) -> String {
        self.stdout
            .split_whitespace()
            .find(|word| word.len() == 24 && word.matches('-').count() == 4)
            .unwrap_or_else(|| panic!("no owner code in {:?}", self.stdout))
            .to_owned()
    }
}

/// Polls `request` until `event` happens. The request must not finish first.
pub async fn drive_until<F>(request: &mut F, event: impl Future<Output = ()>)
where
    F: Future<Output = Reply> + Unpin,
{
    tokio::select! {
        biased;
        () = event => {}
        reply = request => panic!("the request finished early: {} {}", reply.status, reply.text()),
    }
}

/// Runs the futures concurrently on this task and returns their outputs in order.
pub async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pending: Vec<_> = futures
        .into_iter()
        .map(|future| (Box::pin(future), None))
        .collect();
    std::future::poll_fn(|context| {
        let mut all_ready = true;
        for (future, output) in &mut pending {
            if output.is_none() {
                match future.as_mut().poll(context) {
                    std::task::Poll::Ready(value) => *output = Some(value),
                    std::task::Poll::Pending => all_ready = false,
                }
            }
        }
        if all_ready {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    pending
        .into_iter()
        .map(|(_, output)| output.unwrap())
        .collect()
}

pub fn member(key: DeviceKey, session: SessionCreated) -> Member {
    Member {
        key,
        account_id: session.account_id,
        device_id: session.device_id,
        token: session.token.expose().to_owned(),
    }
}
