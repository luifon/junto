//! The client of the public API of one server (ADR 0201, ADR 0205).

mod admin;
mod calls;
mod client;
pub(crate) mod error;
mod messages;
mod moderation;
mod notices;
pub(crate) mod proof;
mod recordings;
mod retry;
mod roles;
mod rules;
pub(crate) mod session;
mod threads;
pub(crate) mod transport;

#[cfg(test)]
pub(crate) mod testing;

pub use client::{ApiClient, Session, server_info};
pub use error::{
    AccessLoss, ApiError, ErrorCategory, Rejection, category, code_name, unknown_refusal,
};
pub use messages::{CHUNK_BYTES, FileBody, PageQuery};
pub use moderation::{LONGEST_REASON, Statement, logo_at, stated_reason};
pub use notices::{SearchQuery, SettingsScope};
pub use proof::{ChallengeMismatch, ExpectedAccount};
pub use retry::RetryPolicy;
pub use session::{AvatarImage, Changed, ServerApi};
pub use transport::{
    BodyStream, HttpRequest, HttpResponse, MAX_RESPONSE_BYTES, Method, OpenFuture, OpenedResponse,
    Receipt, RequestBody, ReqwestTransport, Transport, TransportError, TransportErrorKind,
    TransportFuture,
};
