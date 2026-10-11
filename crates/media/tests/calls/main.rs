//! The media engine and the calls of a client against the real server of this repository
//! and a real LiveKit server (ADR 0902). No test opens a real microphone or plays a sound:
//! the microphone of each client is a tone, and its output is a sink that measures.
//!
//! The tests are ignored in the plain run and run with:
//!
//! ```sh
//! tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
//!     cargo test -p junto-media --test calls -- --ignored
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

mod devices;
mod scene;
mod servers;
mod sound;
