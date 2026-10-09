//! The Junto server: accounts, permissions and the public application API.

pub mod access;
pub mod audit;
pub mod bootstrap;
pub mod clock;
pub mod config;
pub mod database;
pub mod db;
pub mod error;
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

/// Serves the API on `listener` until `shutdown` resolves, then finishes the open requests.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let service = routes::router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service)
        .with_graceful_shutdown(shutdown)
        .await
}
