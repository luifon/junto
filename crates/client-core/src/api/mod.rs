//! The client of the public API of one server (ADR 0201, ADR 0205).

mod client;
mod error;
pub(crate) mod proof;
mod retry;
pub(crate) mod session;
mod transport;

#[cfg(test)]
pub(crate) mod testing;

pub use client::{ApiClient, Session, server_info};
pub use error::{
    AccessLoss, ApiError, ErrorCategory, Rejection, category, code_name, unknown_refusal,
};
pub use proof::{ChallengeMismatch, ExpectedAccount};
pub use retry::RetryPolicy;
pub use session::{AvatarImage, ServerApi};
pub use transport::{
    HttpRequest, HttpResponse, MAX_RESPONSE_BYTES, Method, RequestBody, ReqwestTransport,
    Transport, TransportError, TransportErrorKind, TransportFuture,
};
