//! The flows that a user interface drives (ADR 0712).
//!
//! Every flow has the same contract. It is in one state at a time, and each state is one
//! that the interface can show and that lasts until an action. A state accepts a fixed set
//! of actions. An action runs to its end and then leaves the flow in a state again: the
//! next one when it succeeded, the one before when it failed, with the reason. While an
//! action runs the flow accepts no other action. No flow waits for the interface.

use std::time::Duration;

use junto_protocol::ids::AccountId;

use crate::address::ServerAddress;
use crate::api::{ApiError, ChallengeMismatch, ErrorCategory, RejectionCode, TransportError};
use crate::keys::KeyStoreError;
use crate::storage::StorageError;
use crate::version::Incompatibility;

pub mod access;
pub mod add_server;
pub mod approve_link;
pub mod link_all;
pub(crate) mod machine;

/// What an interface reads from a flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowView<S, K> {
    pub state: S,
    /// The action that runs now.
    pub in_progress: Option<K>,
    /// The actions that the flow accepts now. Empty while an action runs.
    pub actions: Vec<K>,
    /// Why the last action failed. The next action clears it.
    pub error: Option<FlowError>,
}

/// Why an action of a flow did not succeed. The value holds no secret and can be shown.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    #[error("the action is not available in this state")]
    NotAvailable,
    #[error("another action is in progress")]
    Busy,
    #[error("{0}")]
    InvalidInput(String),
    /// No answer arrived: the address, the network or the certificate of the server.
    #[error(transparent)]
    Unreachable(TransportError),
    #[error("the server failed or its answer was lost")]
    Unavailable,
    #[error("the server limits attempts; try again later")]
    Limited { retry_after: Option<Duration> },
    /// The server refused. The code states why.
    #[error("the server refused with the error {code}")]
    Refused {
        code: RejectionCode,
        category: ErrorCategory,
    },
    #[error("the address answers as another server than the saved one")]
    OtherServer,
    #[error("the server {server_id} is already saved")]
    AlreadySaved {
        server_id: junto_protocol::ids::ServerId,
    },
    #[error(transparent)]
    Incompatible(Incompatibility),
    #[error("the server sent a challenge that does not match the request: {0}")]
    Challenge(ChallengeMismatch),
    #[error("the server answered in a form that this client cannot read")]
    InvalidResponse,
    #[error("this device is not logged in to the server")]
    NotLoggedIn,
    #[error("the account lost its access to the server")]
    AccessLost,
    /// The session of this device was renewed into another account. What was confirmed
    /// for the account before was not done.
    #[error("this device now belongs to another account, and the action was not done")]
    AccountChanged { was: AccountId, now: AccountId },
    /// A key of this server is stored for another address. It is not used here.
    #[error("the stored device key of this server belongs to the address {stored}")]
    KeyOfOtherAddress { stored: ServerAddress },
    #[error("the server is not saved on this device")]
    UnknownServer,
    #[error("the code expired")]
    Expired,
    #[error("the text is not a valid {0}")]
    InvalidBundle(&'static str),
    /// An action over several servers ended, and some of them failed. Each server has its
    /// own result.
    #[error("{failed} of {total} servers did not succeed")]
    Incomplete { failed: usize, total: usize },
    #[error("the server API has no operation for this yet")]
    NotOffered,
    #[error("a local file could not be used: {0}")]
    Storage(String),
    #[error("the client is shutting down")]
    Stopped,
}

impl From<&ApiError> for FlowError {
    fn from(error: &ApiError) -> Self {
        match error {
            ApiError::Unreachable(error) => Self::Unreachable(error.clone()),
            ApiError::OutcomeUnknown => Self::Unavailable,
            ApiError::Rejected(rejection) => match rejection.category() {
                ErrorCategory::Unchanged | ErrorCategory::ServerFault => Self::Unavailable,
                ErrorCategory::Limited => Self::Limited {
                    retry_after: rejection.retry_after,
                },
                category => Self::Refused {
                    code: rejection.code.clone(),
                    category,
                },
            },
            ApiError::InvalidResponse { status, .. } if *status >= 500 => Self::Unavailable,
            ApiError::InvalidResponse { .. } => Self::InvalidResponse,
            ApiError::Challenge(mismatch) => Self::Challenge(*mismatch),
            ApiError::NotLoggedIn => Self::NotLoggedIn,
            ApiError::AccessLost(_) => Self::AccessLost,
            ApiError::OtherServer { .. } => Self::OtherServer,
            ApiError::Incompatible(incompatibility) => Self::Incompatible(*incompatibility),
            ApiError::Key(error) => Self::from(error),
            ApiError::AccountChanged { was, now } => Self::AccountChanged {
                was: *was,
                now: *now,
            },
            ApiError::NotOffered { .. } => Self::NotOffered,
            ApiError::Storage(error) => Self::Storage(error.to_string()),
            ApiError::Request(reason) => Self::InvalidInput(reason.clone()),
            ApiError::Stopped => Self::Stopped,
        }
    }
}

impl From<ApiError> for FlowError {
    fn from(error: ApiError) -> Self {
        Self::from(&error)
    }
}

impl From<&KeyStoreError> for FlowError {
    fn from(error: &KeyStoreError) -> Self {
        match error {
            KeyStoreError::OtherOrigin { stored, .. } => Self::KeyOfOtherAddress {
                stored: stored.clone(),
            },
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<KeyStoreError> for FlowError {
    fn from(error: KeyStoreError) -> Self {
        Self::from(&error)
    }
}

impl From<StorageError> for FlowError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<crate::book::BookError> for FlowError {
    fn from(error: crate::book::BookError) -> Self {
        use crate::book::BookError;
        match error {
            BookError::AlreadySaved { server_id, .. }
            | BookError::AddressTaken { server_id, .. } => Self::AlreadySaved { server_id },
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<crate::runtime::Stopped> for FlowError {
    fn from(_: crate::runtime::Stopped) -> Self {
        Self::Stopped
    }
}

impl From<crate::profile::InvalidName> for FlowError {
    fn from(error: crate::profile::InvalidName) -> Self {
        Self::InvalidInput(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Rejection, TransportError, TransportErrorKind};
    use junto_protocol::error::ErrorCode;

    fn rejected(code: ErrorCode, retry_after: Option<u64>) -> ApiError {
        ApiError::Rejected(Rejection {
            code: RejectionCode::Known(code),
            status: code.http_status(),
            retry_after: retry_after.map(Duration::from_secs),
            message: "a sentence of the server".to_owned(),
        })
    }

    #[test]
    fn every_code_of_the_server_becomes_a_flow_error_without_the_text_of_the_server() {
        for code in ErrorCode::ALL {
            let error = FlowError::from(rejected(*code, Some(30)));
            let expected = match crate::api::category(*code) {
                ErrorCategory::Unchanged | ErrorCategory::ServerFault => FlowError::Unavailable,
                ErrorCategory::Limited => FlowError::Limited {
                    retry_after: Some(Duration::from_secs(30)),
                },
                category => FlowError::Refused {
                    code: RejectionCode::Known(*code),
                    category,
                },
            };
            assert_eq!(error, expected, "{code:?}");
            assert!(!error.to_string().contains("sentence"));
        }
    }

    #[test]
    fn failures_without_an_answer_keep_their_cause() {
        let refused = TransportError::new(TransportErrorKind::Connect, "connection refused");
        let error = FlowError::from(ApiError::Unreachable(refused.clone()));
        assert_eq!(error, FlowError::Unreachable(refused));
        assert_eq!(
            error.to_string(),
            "the server cannot be reached (connection refused)"
        );
        assert_eq!(
            FlowError::from(ApiError::OutcomeUnknown),
            FlowError::Unavailable
        );
        let bad_gateway = ApiError::InvalidResponse {
            status: 502,
            reason: "no error body",
        };
        assert_eq!(FlowError::from(bad_gateway), FlowError::Unavailable);
        let unreadable = ApiError::InvalidResponse {
            status: 200,
            reason: "unexpected body",
        };
        assert_eq!(FlowError::from(unreadable), FlowError::InvalidResponse);
    }
}
