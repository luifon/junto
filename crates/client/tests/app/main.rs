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
// A test waits for its files by design; the refusals of the file system in `clippy.toml`
// are for the code of the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/limit.rs"]
mod limit;
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/recording_server.rs"]
mod recording_server;
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

mod account;
mod admin;
mod admin_pages;
mod calls;
mod contacts;
mod data_directory;
mod departures;
mod flows;
mod glossary;
// The rate of growth of the long use of `tests/endurance/`, tested here because that
// target runs without the test runner.
#[path = "../rehearsal/growth.rs"]
mod growth;
mod growth_tests;
mod harness;
mod layout;
mod layout_forms;
mod linking;
mod live;
mod logo;
mod logout;
mod messages;
mod notices;
mod people;
mod permissions;
mod recordings;
mod recovery;
// The steps of a recording need the recorder of a deployment: only the native rehearsal
// (`tests/rehearsal/`) runs them.
#[allow(dead_code)]
mod rehearsal;
mod remaining;
mod roles;
mod search;
mod servers;
mod states;
mod threads;
mod transcripts;

mod updates;
