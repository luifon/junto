//! Text messages, current mentions and resumable attachment storage (ADR 0408).
pub mod direct_calls;
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
pub use place::{Place, Purpose};
pub use store::{
    REPLY_AUTHORS, VISIBLE, changed, changed_at, get, load_messages, remove, snapshot, state,
    state_query, system,
};

pub(crate) use store::erase;
