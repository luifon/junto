//! Text messages, current mentions and resumable attachment storage (ADR 0408).
pub mod compat;
pub mod direct_calls;
pub mod document;
pub mod forwarding;
pub mod groups;
mod mentions;
pub mod notifications;
pub mod page;
mod place;
pub mod retention;
mod store;
pub mod threads;
pub mod transfers;
pub mod uploads;
pub mod visibility;

pub use mentions::{mentioned_among, mentions};
pub use place::{Place, Purpose};
pub use store::{
    JOINED_LINES_WITHIN, REPLY_AUTHORS, VISIBLE, changed, created, get, load_messages, remove,
    rename_line, snapshot, state, state_query, system, system_notifying,
};

pub(crate) use store::erase;
