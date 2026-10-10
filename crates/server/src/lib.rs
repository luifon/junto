//! The Junto server: accounts, permissions and the public application API.

pub mod access;
pub mod audit;
pub mod bodies;
pub mod bootstrap;
pub mod calls;
pub mod clock;
pub mod config;
pub mod database;
pub mod db;
pub mod error;
pub mod events;
pub mod extract;
pub mod image;
pub mod maintenance;
pub mod media;
pub mod owner;
pub mod proof;
pub mod rate_limit;
pub mod routes;
pub mod secrets;
pub mod state;
pub mod store;
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
/// server of the configuration, when one is configured.
pub async fn prepare(
    database: Database,
    config: Config,
    clock: Arc<dyn Clock>,
) -> Result<Prepared, StartError> {
    let media = match &config.media {
        Some(settings) => {
            let client = media::livekit::LiveKit::new(settings.clone(), clock.clone())
                .map_err(|error| StartError::MediaServer(error.to_string()))?;
            Some(Arc::new(client) as Arc<dyn media::MediaServer>)
        }
        None => None,
    };
    prepare_with(database, config, clock, media).await
}

/// Like [`prepare`], with the media server that the caller gives.
pub async fn prepare_with(
    database: Database,
    config: Config,
    clock: Arc<dyn Clock>,
    media: Option<Arc<dyn media::MediaServer>>,
) -> Result<Prepared, StartError> {
    let started = bootstrap::start(&database, &config.server_name, clock.now()).await?;
    Ok(Prepared {
        state: AppState::new(database, config, clock, started.server_id, media),
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
    /// The admission check of calls and the publication of speaking marks, at their
    /// intervals (ADR 0509, ADR 0513). Tests that fix the order of things switch them off
    /// and run each check themselves.
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
    let watcher = tokio::spawn(events::watcher::run(state.clone()));
    let calls = (tasks.calls && state.calls.media().is_some()).then(|| {
        (
            tokio::spawn(calls::admission::run(state.clone())),
            tokio::spawn(calls::speaking::run(state.clone())),
        )
    });
    let stopping = state.clone();
    let stop = async move {
        shutdown.await;
        stopping.streams.shut_down();
    };
    let service = routes::router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let served = axum::serve(listener, service)
        .with_graceful_shutdown(stop)
        .await;
    let limit = state.config.limits.stream.stop_timeout.unsigned_abs();
    state.streams.stop(limit).await;
    watcher.abort();
    if let Some((admission, speaking)) = calls {
        admission.abort();
        speaking.abort();
    }
    served
}
