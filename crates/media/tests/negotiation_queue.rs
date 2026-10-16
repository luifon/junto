//! The publisher negotiation queue of the vendored call library (ADR 0733). A remote track
//! that is published while this device is in a room reaches it only through a negotiation of
//! this queue; a lost request leaves the person unheard for the rest of the connection.
//!
//! The queue is private to the call library, so this test compiles the same source file and
//! runs its tests here. The file uses the standard library only.

#[path = "../../../vendor/livekit-0.9.4/src/rtc_engine/negotiation_queue.rs"]
mod negotiation_queue;
