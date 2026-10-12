//! The client of the public API of one server (ADR 0201, ADR 0205).

mod calls;
mod client;
pub(crate) mod error;
mod messages;
mod notices;
pub(crate) mod proof;
mod recordings;
mod retry;
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
pub use notices::{SearchQuery, SettingsScope};
pub use proof::{ChallengeMismatch, ExpectedAccount};
pub use retry::RetryPolicy;
pub use session::{AvatarImage, ServerApi};
pub use transport::{
    BodyStream, HttpRequest, HttpResponse, MAX_RESPONSE_BYTES, Method, OpenFuture, OpenedResponse,
    RequestBody, ReqwestTransport, Transport, TransportError, TransportErrorKind, TransportFuture,
};
