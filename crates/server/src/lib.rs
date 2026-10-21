//! The Junto server: accounts, permissions and the public application API.

#![cfg_attr(not(windows), forbid(unsafe_code))]

pub mod access;
pub mod archive;
pub mod audit;
pub mod backup;
pub mod bodies;
pub mod bootstrap;
pub mod calls;
pub mod clock;
pub mod config;
pub mod database;
pub mod db;
pub mod download;
pub mod error;
pub mod events;
pub mod exports;
pub mod extract;
pub mod image;
pub mod maintenance;
pub mod media;
pub mod messages;
pub mod owner;
pub mod proof;
pub mod purge;
pub mod rate_limit;
pub mod receipts;
pub mod recorder;
pub mod recordings;
pub mod routes;
pub mod secrets;
pub mod starter;
pub mod state;
pub mod storage;
pub mod store;
pub mod structure;
pub mod telemetry;
pub mod tokens;
pub mod validate;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::bootstrap::StartError;
use crate::clock::Clock;
use crate::config::Config;
use crate::database::Database;
use crate::state::AppState;

/// The relative path of the committed OpenAPI document, from the repository root.
pub const OPENAPI_DOCUMENT_PATH: &str = "docs/api/openapi.json";

/// The relative path of the committed description of the event stream.
pub const EVENTS_DOCUMENT_PATH: &str = "docs/api/events.json";

/// The relative path of the committed JSON Schema of the structure file.
pub const STRUCTURE_FILE_DOCUMENT_PATH: &str = "docs/api/structure-file.json";

/// The JSON Schema of the structure file that the setup tool writes and reads (ADR 0204).
pub fn structure_file_document() -> Result<String, serde_json::Error> {
    let mut document = serde_json::to_string_pretty(&structure::file_schema()?)?;
    document.push('\n');
    Ok(document)
}

pub fn openapi_document() -> Result<String, serde_json::Error> {
    let mut document = serde_json::to_string_pretty(&routes::openapi())?;
    document.push('\n');
    Ok(document)
}

pub struct Prepared {
    pub state: AppState,
    /// Present while the server has no owner.
    pub owner_code: Option<junto_protocol::secret::OwnerCode>,
}

/// Brings the database to the current schema, performs the first start when it has not
/// happened, and builds the state of the application. The media server is the LiveKit
/// server of the configuration, when one is configured, and the recorder is LiveKit Egress
/// behind it, when the directory of the recordings is configured.
pub async fn prepare(
    database: Database,
    config: Config,
    clock: Arc<dyn Clock>,
) -> Result<Prepared, StartError> {
    let api = match &config.media {
        Some(settings) => {
            let api = media::twirp::Twirp::new(settings.clone(), clock.clone())
                .map_err(StartError::MediaServer)?;
            Some(Arc::new(api))
        }
        None => None,
    };
    let media = api.clone().map(|api| {
        let client: Arc<dyn media::MediaServer> = Arc::new(media::livekit::LiveKit::new(api));
        client
    });
    let recorder = api.zip(config.recorder.clone()).map(|(api, settings)| {
        let limits = &config.limits.recordings;
        let egress = recorder::livekit::Egress::new(api, limits);
        let disk = storage::Directory::new(settings.directory.clone());
        recordings::Backend::new(Arc::new(egress), Arc::new(disk), settings, limits)
    });
    if let Some(backend) = &recorder {
        backend
            .tools
            .available()
            .await
            .map_err(StartError::MediaTools)?;
    }
    prepare_with(database, config, clock, media, recorder).await
}

/// Like [`prepare`], with the media server and the recorder that the caller gives.
pub async fn prepare_with(
    database: Database,
    config: Config,
    clock: Arc<dyn Clock>,
    media: Option<Arc<dyn media::MediaServer>>,
    recorder: Option<recordings::Backend>,
) -> Result<Prepared, StartError> {
    let started = bootstrap::start(&database, &config.server_name, clock.now()).await?;
    let server_id = started.server_id;
    Ok(Prepared {
        state: AppState::new(database, config, clock, server_id, media, recorder),
        owner_code: started.owner_code,
    })
}

/// The description of the event stream: a schema for every message and every event.
pub fn events_document() -> Result<String, serde_json::Error> {
    let mut document = serde_json::to_string_pretty(&events::schema::document()?)?;
    document.push('\n');
    Ok(document)
}

/// The tasks that run beside the requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tasks {
    /// The admission check of calls, the publication of speaking marks, the controller of
    /// recordings, the processing of their files and the transcription, at their intervals
    /// (ADR 0509, ADR 0513, ADR 0604, ADR 0606, ADR 0607). Tests that fix the order of
    /// things switch them off and run each step themselves.
    pub calls: bool,
}

/// Serves the API on `listener` until `shutdown` resolves, then finishes the open requests,
/// closes the connections of the event stream and waits until each has closed.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_with(listener, state, shutdown, Tasks { calls: true }).await
}

pub async fn serve_with(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
    tasks: Tasks,
) -> std::io::Result<()> {
    let telemetry = telemetry::Telemetry::start(state.config.otel_endpoint.clone()).await?;
    let watcher = tokio::spawn(events::watcher::run(state.clone()));
    let mut running = Vec::new();
    if tasks.calls && state.calls.media().is_some() {
        running.push(tokio::spawn(calls::admission::run(state.clone())));
        running.push(tokio::spawn(calls::speaking::run(state.clone())));
        running.push(tokio::spawn(recordings::control::run(state.clone())));
    }
    if tasks.calls && state.recordings.backend().is_some() {
        running.push(tokio::spawn(recordings::process::run(state.clone())));
        running.push(tokio::spawn(recordings::transcribe::run(state.clone())));
    }
    let stopping = state.clone();
    let stop = async move {
        shutdown.await;
        stopping.streams.shut_down();
    };
    let router = routes::router(state.clone());
    let router = match &telemetry {
        Some(telemetry) => telemetry.instrument(router),
        None => router,
    };
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    let served = axum::serve(listener, service)
        .with_graceful_shutdown(stop)
        .await;
    let limit = state.config.limits.stream.stop_timeout.unsigned_abs();
    state.streams.stop(limit).await;
    watcher.abort();
    for task in running {
        task.abort();
    }
    if let Some(telemetry) = telemetry {
        telemetry.shutdown().await;
    }
    served
}

mod role_order;
