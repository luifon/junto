//! Text messages, current mentions and resumable attachment storage (ADR 0408).
pub mod document;
mod mentions;
pub mod notifications;
mod place;
pub mod retention;
mod store;
pub mod threads;
pub mod transfers;
pub mod uploads;

pub use mentions::{mentioned_among, mentions};
pub use place::Place;
pub use store::{
    VISIBLE, changed, get, load_messages, remove, snapshot, state, state_query, system,
};

pub(crate) use store::erase;
