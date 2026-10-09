//! The Junto server: accounts, permissions and the public application API.

pub mod access;
pub mod audit;
pub mod bootstrap;
pub mod clock;
pub mod config;
pub mod database;
pub mod db;
pub mod error;
pub mod events;
pub mod extract;
pub mod maintenance;
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
/// happened, and builds the state of the application.
pub async fn prepare(
    database: Database,
    config: Config,
    clock: Arc<dyn Clock>,
) -> Result<Prepared, StartError> {
    let started = bootstrap::start(&database, &config.server_name, clock.now()).await?;
    Ok(Prepared {
        state: AppState::new(database, config, clock, started.server_id),
        owner_code: started.owner_code,
    })
}

/// The description of the event stream: a schema for every message and every event.
pub fn events_document() -> Result<String, serde_json::Error> {
    let mut document = serde_json::to_string_pretty(&events::schema::document()?)?;
    document.push('\n');
    Ok(document)
}

/// Serves the API on `listener` until `shutdown` resolves, then finishes the open requests,
/// closes the connections of the event stream and waits until each has closed.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let watcher = tokio::spawn(events::watcher::run(state.clone()));
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
    served
}
