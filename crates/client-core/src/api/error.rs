//! The errors of the API client and what each server error code means for the client.

use std::fmt;
use std::time::Duration;

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, ServerId};
use serde::Deserialize;

use super::proof::ChallengeMismatch;
use super::transport::{HttpResponse, TransportError};
use crate::keys::KeyStoreError;
use crate::storage::StorageError;
use crate::version::Incompatibility;

const LONGEST_MESSAGE: usize = 200;
const LONGEST_CODE: usize = 64;

/// What the client does about a refusal. Every code of the server belongs to one group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// `busy`, `timeout`: nothing was changed and the same request is sent again.
    Unchanged,
    /// A limit on attempts or a ceiling of the server. The request can be sent later.
    Limited,
    /// `unauthenticated`: the session does not exist any more. The device logs in again.
    SessionEnded,
    /// The challenge or its signature was not accepted. A new challenge is needed.
    Proof,
    /// `unknown_device`: the device key belongs to no account on the server.
    UnknownDevice,
    /// `account_banned`.
    Banned,
    /// The account may not do this.
    NotAllowed,
    /// The object does not exist or the account cannot see it.
    NotFound,
    /// The request does not fit the present state of the server.
    Conflict,
    /// An owner, invite or link code was not accepted.
    CodeRefused,
    /// The request itself is wrong. Sending it again does not help.
    ClientFault,
    /// The server failed.
    ServerFault,
}

/// The group of each code. The match has no wildcard: a code that the protocol crate adds
/// does not compile until it is placed here.
pub const fn category(code: ErrorCode) -> ErrorCategory {
    match code {
        ErrorCode::Busy | ErrorCode::Timeout => ErrorCategory::Unchanged,
        ErrorCode::RateLimited
        | ErrorCode::TooManyAttempts
        | ErrorCode::ChallengesFull
        | ErrorCode::DeviceLinksFull
        | ErrorCode::JoinRequestsFull => ErrorCategory::Limited,
        ErrorCode::Unauthenticated => ErrorCategory::SessionEnded,
        ErrorCode::ChallengeInvalid | ErrorCode::ChallengeExpired | ErrorCode::InvalidSignature => {
            ErrorCategory::Proof
        }
        ErrorCode::UnknownDevice => ErrorCategory::UnknownDevice,
        ErrorCode::AccountBanned => ErrorCategory::Banned,
        ErrorCode::Forbidden
        | ErrorCode::OwnerOnly
        | ErrorCode::OwnerProtected
        | ErrorCode::OwnAccountTarget
        | ErrorCode::ExceedsOwnPermissions => ErrorCategory::NotAllowed,
        ErrorCode::NotFound => ErrorCategory::NotFound,
        ErrorCode::Conflict
        | ErrorCode::KeyAlreadyRegistered
        | ErrorCode::OwnerAlreadyClaimed
        | ErrorCode::JoinRequestPending
        | ErrorCode::PublicKeyMismatch => ErrorCategory::Conflict,
        ErrorCode::OwnerCodeInvalid
        | ErrorCode::InviteNotFound
        | ErrorCode::InviteExpired
        | ErrorCode::InviteExhausted
        | ErrorCode::InviteRevoked
        | ErrorCode::LinkCodeInvalid => ErrorCategory::CodeRefused,
        ErrorCode::InvalidRequest | ErrorCode::MethodNotAllowed => ErrorCategory::ClientFault,
        ErrorCode::Internal => ErrorCategory::ServerFault,
    }
}

/// The group of a code that this client does not know, from its HTTP status. Such a code
/// is never sent again by itself and never ends a session.
const fn category_of_status(status: u16) -> ErrorCategory {
    match status {
        403 | 401 => ErrorCategory::NotAllowed,
        404 => ErrorCategory::NotFound,
        409 | 410 => ErrorCategory::Conflict,
        429 => ErrorCategory::Limited,
        500..=599 => ErrorCategory::ServerFault,
        _ => ErrorCategory::ClientFault,
    }
}

/// The code of a refusal: one that this client knows, or the text of one that a newer
/// server added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionCode {
    Known(ErrorCode),
    Unknown(String),
}

impl fmt::Display for RejectionCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(code) => match serde_json::to_value(code) {
                Ok(serde_json::Value::String(name)) => formatter.write_str(&name),
                _ => write!(formatter, "{code:?}"),
            },
            Self::Unknown(name) => formatter.write_str(name),
        }
    }
}

/// An error response of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: RejectionCode,
    pub status: u16,
    pub retry_after: Option<Duration>,
    /// The sentence of the server for a person. Its text is not stable and comes from the
    /// server, so the client shows it as a quotation and decides nothing on it.
    pub message: String,
}

impl Rejection {
    pub fn known(&self) -> Option<ErrorCode> {
        match &self.code {
            RejectionCode::Known(code) => Some(*code),
            RejectionCode::Unknown(_) => None,
        }
    }

    pub fn category(&self) -> ErrorCategory {
        match &self.code {
            RejectionCode::Known(code) => category(*code),
            RejectionCode::Unknown(_) => category_of_status(self.status),
        }
    }
}

/// Why the account of this device cannot use the server any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessLoss {
    Banned,
    /// The account or this device was removed: the server does not know the key.
    Removed,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Unreachable(TransportError),
    #[error("the server refused the request with the error {}", .0.code)]
    Rejected(Rejection),
    #[error(
        "the server answered with status {status} in a form that this client cannot read ({reason})"
    )]
    InvalidResponse { status: u16, reason: &'static str },
    #[error("the challenge of the server does not match the request: {0}")]
    Challenge(#[from] ChallengeMismatch),
    /// A signed request was sent and its answer was lost. The request may have taken effect.
    #[error("the answer of the server was lost, and the request may have taken effect")]
    OutcomeUnknown,
    #[error("this device is not logged in to the server")]
    NotLoggedIn,
    #[error("the account lost its access to the server")]
    AccessLost(AccessLoss),
    #[error("the address answers as server {reported}, and the saved server is {saved}")]
    OtherServer { saved: ServerId, reported: ServerId },
    #[error(transparent)]
    Incompatible(#[from] Incompatibility),
    #[error(transparent)]
    Key(#[from] KeyStoreError),
    #[error("the request could not be built: {0}")]
    Request(String),
    /// The session of this device was renewed and now belongs to another account than the
    /// one that started the request. The request was not sent again.
    #[error("this device now belongs to the account {now}, and the request was made for {was}")]
    AccountChanged { was: AccountId, now: AccountId },
    #[error("the server API has no operation to {operation}")]
    NotOffered { operation: &'static str },
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("the client is shutting down")]
    Stopped,
}

impl ApiError {
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Rejected(rejection) => Some(rejection),
            _ => None,
        }
    }

    /// The known code of a refusal.
    pub fn code(&self) -> Option<ErrorCode> {
        self.rejection().and_then(Rejection::known)
    }

    pub fn category(&self) -> Option<ErrorCategory> {
        self.rejection().map(Rejection::category)
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.rejection().and_then(|rejection| rejection.retry_after)
    }

    /// True when the server did not answer, or answered that it failed. The server may be
    /// usable later and the state of the account is not known.
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Unreachable(_) | Self::OutcomeUnknown => true,
            Self::InvalidResponse { status, .. } => *status >= 500,
            Self::Rejected(rejection) => matches!(
                rejection.category(),
                ErrorCategory::Unchanged | ErrorCategory::ServerFault
            ),
            _ => false,
        }
    }
}

#[derive(Deserialize)]
struct WireError {
    error: WireDetail,
}

#[derive(Deserialize)]
struct WireDetail {
    code: String,
    #[serde(default)]
    message: String,
}

/// Reads a response with a status of 400 or above (ADR 0205).
pub(crate) fn read_rejection(response: &HttpResponse) -> ApiError {
    let Ok(wire) = serde_json::from_slice::<WireError>(&response.body) else {
        return ApiError::InvalidResponse {
            status: response.status,
            reason: "no error body",
        };
    };
    let name_is_plain = !wire.error.code.is_empty()
        && wire.error.code.len() <= LONGEST_CODE
        && wire
            .error
            .code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '_');
    if !name_is_plain {
        return ApiError::InvalidResponse {
            status: response.status,
            reason: "no error code",
        };
    }
    let code = match serde_json::from_value::<ErrorCode>(wire.error.code.as_str().into()) {
        Ok(code) => RejectionCode::Known(code),
        Err(_) => RejectionCode::Unknown(wire.error.code),
    };
    let message: String = wire
        .error
        .message
        .chars()
        .filter(|c| !c.is_control())
        .take(LONGEST_MESSAGE)
        .collect();
    ApiError::Rejected(Rejection {
        code,
        status: response.status,
        retry_after: response.retry_after,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str, retry_after: Option<u64>) -> HttpResponse {
        HttpResponse {
            status,
            retry_after: retry_after.map(Duration::from_secs),
            body: body.as_bytes().to_vec().into(),
        }
    }

    fn wire_name(code: ErrorCode) -> String {
        serde_json::to_value(code)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn every_code_of_the_server_is_read_and_has_a_group() {
        for code in ErrorCode::ALL {
            let name = wire_name(*code);
            let body = format!(r#"{{"error":{{"code":"{name}","message":"text"}}}}"#);
            let error = read_rejection(&response(code.http_status(), &body, Some(3)));
            assert_eq!(error.code(), Some(*code), "{name}");
            assert_eq!(error.category(), Some(category(*code)));
            assert_eq!(error.retry_after(), Some(Duration::from_secs(3)));
            assert!(error.to_string().ends_with(&name), "{error}");
            assert!(!error.to_string().contains("text"));
        }
    }

    #[test]
    fn the_groups_follow_the_decisions_of_the_records() {
        use ErrorCategory as C;
        let cases = [
            (ErrorCode::Busy, C::Unchanged),
            (ErrorCode::Timeout, C::Unchanged),
            (ErrorCode::RateLimited, C::Limited),
            (ErrorCode::TooManyAttempts, C::Limited),
            (ErrorCode::ChallengesFull, C::Limited),
            (ErrorCode::DeviceLinksFull, C::Limited),
            (ErrorCode::JoinRequestsFull, C::Limited),
            (ErrorCode::Unauthenticated, C::SessionEnded),
            (ErrorCode::ChallengeInvalid, C::Proof),
            (ErrorCode::ChallengeExpired, C::Proof),
            (ErrorCode::InvalidSignature, C::Proof),
            (ErrorCode::UnknownDevice, C::UnknownDevice),
            (ErrorCode::AccountBanned, C::Banned),
            (ErrorCode::Forbidden, C::NotAllowed),
            (ErrorCode::OwnerOnly, C::NotAllowed),
            (ErrorCode::OwnerProtected, C::NotAllowed),
            (ErrorCode::OwnAccountTarget, C::NotAllowed),
            (ErrorCode::ExceedsOwnPermissions, C::NotAllowed),
            (ErrorCode::NotFound, C::NotFound),
            (ErrorCode::Conflict, C::Conflict),
            (ErrorCode::KeyAlreadyRegistered, C::Conflict),
            (ErrorCode::OwnerAlreadyClaimed, C::Conflict),
            (ErrorCode::JoinRequestPending, C::Conflict),
            (ErrorCode::PublicKeyMismatch, C::Conflict),
            (ErrorCode::OwnerCodeInvalid, C::CodeRefused),
            (ErrorCode::InviteNotFound, C::CodeRefused),
            (ErrorCode::InviteExpired, C::CodeRefused),
            (ErrorCode::InviteExhausted, C::CodeRefused),
            (ErrorCode::InviteRevoked, C::CodeRefused),
            (ErrorCode::LinkCodeInvalid, C::CodeRefused),
            (ErrorCode::InvalidRequest, C::ClientFault),
            (ErrorCode::MethodNotAllowed, C::ClientFault),
            (ErrorCode::Internal, C::ServerFault),
        ];
        assert_eq!(cases.len(), ErrorCode::ALL.len());
        for (code, expected) in cases {
            assert_eq!(category(code), expected, "{code:?}");
        }
        let unchanged: Vec<_> = ErrorCode::ALL
            .iter()
            .filter(|code| category(**code) == C::Unchanged)
            .collect();
        assert_eq!(unchanged, [&ErrorCode::Timeout, &ErrorCode::Busy]);
    }

    #[test]
    fn a_code_of_a_newer_server_is_kept_and_grouped_by_its_status() {
        let cases = [
            (400, ErrorCategory::ClientFault),
            (401, ErrorCategory::NotAllowed),
            (403, ErrorCategory::NotAllowed),
            (404, ErrorCategory::NotFound),
            (409, ErrorCategory::Conflict),
            (410, ErrorCategory::Conflict),
            (429, ErrorCategory::Limited),
            (500, ErrorCategory::ServerFault),
            (503, ErrorCategory::ServerFault),
        ];
        for (status, expected) in cases {
            let body = r#"{"error":{"code":"quota_exceeded","message":"later"}}"#;
            let error = read_rejection(&response(status, body, None));
            assert_eq!(error.code(), None);
            assert_eq!(error.category(), Some(expected), "{status}");
            let rejection = error.rejection().unwrap();
            assert_eq!(
                rejection.code,
                RejectionCode::Unknown("quota_exceeded".to_owned())
            );
            assert_eq!(rejection.code.to_string(), "quota_exceeded");
        }
    }

    #[test]
    fn a_body_in_another_form_is_an_invalid_response() {
        let bodies = [
            "",
            "<html>Bad Gateway</html>",
            "{}",
            r#"{"error":"busy"}"#,
            r#"{"error":{"message":"no code"}}"#,
            r#"{"error":{"code":"","message":""}}"#,
            r#"{"error":{"code":"Not A Code","message":""}}"#,
            r#"{"error":{"code":"busy\n","message":""}}"#,
        ];
        for body in bodies {
            let error = read_rejection(&response(502, body, None));
            assert!(
                matches!(error, ApiError::InvalidResponse { status: 502, .. }),
                "{body}: {error}"
            );
            assert!(error.is_unavailable());
        }
        let error = read_rejection(&response(400, "{}", None));
        assert!(!error.is_unavailable());
    }

    #[test]
    fn the_message_of_the_server_is_bounded_and_has_no_control_characters() {
        let long = "m".repeat(5000);
        let body = format!(r#"{{"error":{{"code":"forbidden","message":"a\u001b[31m{long}"}}}}"#);
        let error = read_rejection(&response(403, &body, None));
        let message = &error.rejection().unwrap().message;
        assert_eq!(message.chars().count(), 200);
        assert!(message.starts_with("a[31m"));
        let without: HttpResponse = response(403, r#"{"error":{"code":"forbidden"}}"#, None);
        assert_eq!(read_rejection(&without).rejection().unwrap().message, "");
    }

    #[test]
    fn unavailable_means_that_the_state_of_the_account_is_not_known() {
        let rejected = |code: ErrorCode| {
            let body = format!(
                r#"{{"error":{{"code":"{}","message":""}}}}"#,
                wire_name(code)
            );
            read_rejection(&response(code.http_status(), &body, None))
        };
        assert!(rejected(ErrorCode::Busy).is_unavailable());
        assert!(rejected(ErrorCode::Timeout).is_unavailable());
        assert!(rejected(ErrorCode::Internal).is_unavailable());
        assert!(ApiError::OutcomeUnknown.is_unavailable());
        for code in [
            ErrorCode::AccountBanned,
            ErrorCode::UnknownDevice,
            ErrorCode::Unauthenticated,
            ErrorCode::RateLimited,
            ErrorCode::NotFound,
        ] {
            assert!(!rejected(code).is_unavailable(), "{code:?}");
        }
    }
}
