//! The Junto client without a user interface: where it stores its files, the device keys,
//! the client of the server API, the join and login flows, and the manager that holds
//! several servers at once. A user interface and the command-line tool both build on it.

#![cfg_attr(not(any(windows, target_os = "macos")), forbid(unsafe_code))]

pub mod address;
pub mod api;
pub mod app_badge;
pub mod app_identity;
pub mod book;
pub mod console;
pub mod events;
pub mod flow;
pub mod keys;
pub mod link;
pub mod live;
pub mod manager;
#[cfg(target_os = "macos")]
pub mod menu_bar;
pub mod model;
#[cfg(windows)]
pub mod notification_area;
pub mod paths;
pub mod profile;
pub mod release_configuration;
pub mod releases;
pub mod runtime;
pub mod secret;
pub mod settings;
pub mod storage;
pub mod stream;
#[cfg(windows)]
pub mod taskbar;
pub mod transfers;
pub mod updates;
pub mod version;
pub mod voice;

pub use book::{EntryKey, Owner};
pub use junto_protocol as protocol;
pub use manager::{Client, ClientOptions, OpenError, Reach, ServerHandle};

pub mod call_window;
pub mod emoji_fonts;
