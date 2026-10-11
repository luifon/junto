//! The request, response and event types of the public Junto API.
//!
//! The server and the client both build on these types. With the feature `openapi` the types
//! describe themselves for the OpenAPI document, which the server generates from them; a
//! client does not enable it.
//!
//! A reader of this crate keeps working against a newer server of the same API version: an
//! event of an unknown type, an error with an unknown code and an audit entry with an unknown
//! action are read as explicit unknown values (ADR 0206, ADR 0209). Writing is strict.

pub mod access_token;
pub mod account;
pub mod audit;
pub mod auth;
pub mod call;
pub mod discovery;
pub mod error;
pub mod events;
pub mod ids;
pub mod invite;
pub mod keys;
pub mod message;
pub mod origin;
pub mod recording;
pub mod roles;
pub mod secret;
pub mod server;
pub mod snapshot;
pub mod structure;
pub mod structure_file;
pub mod transcript;

pub use junto_authz::{ServerPermission, WorkspacePermission};

/// The version of the API that this crate describes. It is the `v1` of the route prefix.
pub const API_VERSION: u32 = 1;

/// The route prefix of this API version.
pub const API_PREFIX: &str = "/api/v1";
