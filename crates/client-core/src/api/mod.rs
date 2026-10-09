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
pub use error::{AccessLoss, ApiError, ErrorCategory, Rejection, RejectionCode, category};
pub use proof::{ChallengeMismatch, ExpectedAccount};
pub use retry::RetryPolicy;
pub use session::ServerApi;
pub use transport::{
    HttpRequest, HttpResponse, MAX_RESPONSE_BYTES, Method, ReqwestTransport, Transport,
    TransportError, TransportErrorKind, TransportFuture,
};
