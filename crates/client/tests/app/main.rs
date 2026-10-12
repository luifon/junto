//! The application window against the real server of this repository (ADR 0902): the first
//! start, joining, several servers, the lists that follow the event stream, and the states
//! of a server that a person must see.
//!
//! A test opens the real window code on gpui's test platform, with a client on a data
//! directory of its own, and acts as a person does: it puts text into a field and clicks
//! what is drawn. A second client, without a window, is the administrator of the server.
//!
//! ```sh
//! tools/with-test-postgres.sh tools/serial.sh cargo test -p junto-client --test app
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/recording_server.rs"]
mod recording_server;
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

mod admin;
mod admin_pages;
mod calls;
mod flows;
mod harness;
mod live;
mod messages;
mod notices;
mod recordings;
mod search;
mod states;
mod threads;
