//! The Junto client without a user interface: where it stores its files, the device keys,
//! the client of the server API, the join and login flows, and the manager that holds
//! several servers at once. A user interface and the command-line tool both build on it.

#![cfg_attr(not(windows), forbid(unsafe_code))]

pub mod address;
pub mod api;
pub mod book;
pub mod events;
pub mod flow;
pub mod keys;
pub mod manager;
pub mod paths;
pub mod profile;
pub mod runtime;
pub mod secret;
pub mod storage;
pub mod version;

pub use junto_protocol as protocol;
pub use manager::{Client, ClientOptions, OpenError, Reach, ServerHandle};
