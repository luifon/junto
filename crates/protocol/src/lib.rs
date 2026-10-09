//! The request, response and event types of the public Junto API.
//!
//! The server and the client both build on these types. The OpenAPI document is generated
//! from them.

pub mod account;
pub mod audit;
pub mod auth;
pub mod error;
pub mod ids;
pub mod invite;
pub mod keys;
pub mod roles;
pub mod secret;
pub mod server;
pub mod structure;

pub use junto_authz::{ServerPermission, WorkspacePermission};

/// The version of the API that this crate describes. It is the `v1` of the route prefix.
pub const API_VERSION: u32 = 1;

/// The route prefix of this API version.
pub const API_PREFIX: &str = "/api/v1";
