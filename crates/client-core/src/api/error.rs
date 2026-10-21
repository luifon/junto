//! The errors of the API client and what each server error code means for the client.

use std::fmt;
use std::time::Duration;

use junto_protocol::call::LeaveReason;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, ServerId};

use super::proof::ChallengeMismatch;
use super::transport::{HttpResponse, TransportError};
use crate::keys::KeyStoreError;
use crate::profile::{InvalidAvatar, InvalidName};
use crate::storage::StorageError;
use crate::version::Incompatibility;

const LONGEST_MESSAGE: usize = 200;

/// What the client does about a refusal. Every code of the server belongs to one group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// `busy`, `timeout`: nothing was changed and the same request is sent again.
    Unchanged,
    /// A limit on attempts or on connections. The request can be sent later.
    Limited,
    /// `unauthenticated`: the session does not exist any more. The device logs in again.
    SessionEnded,
    /// The proof of the device key was not accepted: the challenge, its signature, or the
    /// origin that the device signed.
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

/// The group of a code. The match has no wildcard: a code that the protocol crate adds
/// does not compile until it is placed here. A code that this build does not know
/// (ADR 0209) is grouped by the status of its response; it is never sent again by itself
/// and never ends a session.
pub const fn category(code: ErrorCode, status: u16) -> ErrorCategory {
    match code {
        ErrorCode::Busy | ErrorCode::Timeout => ErrorCategory::Unchanged,
        ErrorCode::RateLimited
        | ErrorCode::TooManyAttempts
        | ErrorCode::TooManyConnections
        | ErrorCode::ChallengesFull
        | ErrorCode::DeviceLinksFull
        | ErrorCode::JoinRequestsFull => ErrorCategory::Limited,
        ErrorCode::Unauthenticated => ErrorCategory::SessionEnded,
        ErrorCode::ChallengeInvalid
        | ErrorCode::ChallengeExpired
        | ErrorCode::InvalidSignature
        | ErrorCode::OriginRejected => ErrorCategory::Proof,
        ErrorCode::UnknownDevice => ErrorCategory::UnknownDevice,
        ErrorCode::AccountBanned => ErrorCategory::Banned,
        ErrorCode::Forbidden
        | ErrorCode::OwnerOnly
        | ErrorCode::OwnerProtected
        | ErrorCode::OwnAccountTarget
        | ErrorCode::ExceedsOwnPermissions => ErrorCategory::NotAllowed,
        ErrorCode::NotFound => ErrorCategory::NotFound,
        ErrorCode::Conflict
        | ErrorCode::ChannelArchived
        | ErrorCode::LastDevice
        | ErrorCode::LimitReached
        | ErrorCode::KeyAlreadyRegistered
        | ErrorCode::OwnerAlreadyClaimed
        | ErrorCode::JoinRequestPending
        | ErrorCode::PublicKeyMismatch
        | ErrorCode::CallLocked
        | ErrorCode::CallFull
        | ErrorCode::NotInCall
        | ErrorCode::SeatEnded
        | ErrorCode::StorageLow
        | ErrorCode::RecordingsFull => ErrorCategory::Conflict,
        ErrorCode::OwnerCodeInvalid
        | ErrorCode::InviteNotFound
        | ErrorCode::InviteExpired
        | ErrorCode::InviteExhausted
        | ErrorCode::InviteRevoked
        | ErrorCode::LinkCodeInvalid => ErrorCategory::CodeRefused,
        ErrorCode::InvalidRequest
        | ErrorCode::MethodNotAllowed
        | ErrorCode::ImageTooLarge
        | ErrorCode::ImageUnsupported
        | ErrorCode::RangeNotSatisfiable => ErrorCategory::ClientFault,
        ErrorCode::Unavailable
        | ErrorCode::MediaUnavailable
        | ErrorCode::RecorderUnavailable
        | ErrorCode::Internal => ErrorCategory::ServerFault,
        ErrorCode::Unknown => match status {
            401 | 403 => ErrorCategory::NotAllowed,
            404 => ErrorCategory::NotFound,
            409 | 410 => ErrorCategory::Conflict,
            429 => ErrorCategory::Limited,
            500..=599 => ErrorCategory::ServerFault,
            _ => ErrorCategory::ClientFault,
        },
    }
}

/// The name of a code as the server writes it. A code that this build does not know has
/// no name here; its status tells the kind of the error (ADR 0209).
pub fn code_name(code: ErrorCode) -> String {
    match serde_json::to_value(code) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_owned(),
    }
}

/// An error response of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    /// [`ErrorCode::Unknown`] for a code that a newer server added.
    pub code: ErrorCode,
    pub status: u16,
    pub retry_after: Option<Duration>,
    /// The sentence of the server for a person, cut to 200 characters without control
    /// characters. Its text is not stable and comes from the server, so the client shows
    /// it as a quotation and decides nothing on it. The text of an error of this crate
    /// contains it only for a code that this client does not know.
    pub message: String,
    /// With `seat_ended`: why the seat that a request for a token renews cannot be used
    /// (ADR 0520). Absent when the server does not state it, or states a reason that this
    /// client does not know.
    pub left: Option<LeaveReason>,
}

impl Rejection {
    pub const fn category(&self) -> ErrorCategory {
        category(self.code, self.status)
    }
}

/// How a refusal with a code that this client does not know is shown: by its status and
/// by the sentence of the server, as a quotation (ADR 0209).
pub fn unknown_refusal(status: u16, message: &str) -> String {
    let code = format!("an error code that this client does not know (status {status})");
    if message.is_empty() {
        code
    } else {
        format!("{code}; the server says: \"{message}\"")
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            ErrorCode::Unknown => formatter.write_str(&unknown_refusal(self.status, &self.message)),
            code => write!(formatter, "the error {}", code_name(code)),
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
    #[error("the server refused the request with {0}")]
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
    /// The name is not one that a server accepts. It was not sent.
    #[error(transparent)]
    Name(#[from] InvalidName),
    /// The image is not one that a server stores as an avatar. It was not sent.
    #[error(transparent)]
    Avatar(#[from] InvalidAvatar),
    /// The session of this device was renewed and now belongs to another account than the
    /// one that started the request. The request was not sent again.
    #[error("this device now belongs to the account {now}, and the request was made for {was}")]
    AccountChanged { was: AccountId, now: AccountId },
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

    /// The code of a refusal: [`ErrorCode::Unknown`] for one that this build does not know.
    pub fn code(&self) -> Option<ErrorCode> {
        self.rejection().map(|rejection| rejection.code)
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

/// The error body as the client reads it: a value of `left` that this client does not
/// know does not make the whole body unreadable.
#[derive(serde::Deserialize)]
struct ReadBody {
    error: ReadDetail,
}

#[derive(serde::Deserialize)]
struct ReadDetail {
    code: ErrorCode,
    message: String,
    #[serde(default)]
    left: Option<serde_json::Value>,
}

/// Reads a response with a status of 400 or above (ADR 0205).
pub(crate) fn read_rejection(response: &HttpResponse) -> ApiError {
    let Ok(body) = serde_json::from_slice::<ReadBody>(&response.body) else {
        return ApiError::InvalidResponse {
            status: response.status,
            reason: "no error body",
        };
    };
    let message: String = body
        .error
        .message
        .chars()
        .filter(|c| !c.is_control())
        .take(LONGEST_MESSAGE)
        .collect();
    ApiError::Rejected(Rejection {
        code: body.error.code,
        status: response.status,
        retry_after: response.retry_after,
        message,
        left: body
            .error
            .left
            .and_then(|left| serde_json::from_value(left).ok()),
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
            receipt: crate::api::Receipt::NotStated,
        }
    }

    #[test]
    fn every_code_of_the_server_is_read_and_has_a_group() {
        for code in ErrorCode::ALL {
            let name = code_name(*code);
            let body = format!(r#"{{"error":{{"code":"{name}","message":"text"}}}}"#);
            let error = read_rejection(&response(code.http_status(), &body, Some(3)));
            assert_eq!(error.code(), Some(*code), "{name}");
            assert_eq!(error.category(), Some(category(*code, code.http_status())));
            assert_eq!(error.retry_after(), Some(Duration::from_secs(3)));
            assert!(error.to_string().ends_with(&name), "{error}");
            assert!(!error.to_string().contains("text"));
        }
    }

    #[test]
    fn a_refusal_of_a_renewal_states_why_the_seat_ended() {
        let left = |body: &str| match read_rejection(&response(409, body, None)) {
            ApiError::Rejected(rejection) => {
                assert_eq!(rejection.code, ErrorCode::SeatEnded);
                rejection.left
            }
            other => panic!("read {other:?}"),
        };
        assert_eq!(
            left(r#"{"error":{"code":"seat_ended","message":"","left":"connection_lost"}}"#),
            Some(LeaveReason::ConnectionLost)
        );
        assert_eq!(
            left(r#"{"error":{"code":"seat_ended","message":""}}"#),
            None
        );
        // A reason of a newer server is not known; the refusal is read without it.
        assert_eq!(
            left(r#"{"error":{"code":"seat_ended","message":"","left":"expired"}}"#),
            None
        );
    }

    #[test]
    fn the_groups_follow_the_decisions_of_the_records() {
        use ErrorCategory as C;
        let cases = [
            (ErrorCode::Busy, C::Unchanged),
            (ErrorCode::Timeout, C::Unchanged),
            (ErrorCode::RateLimited, C::Limited),
            (ErrorCode::TooManyAttempts, C::Limited),
            (ErrorCode::TooManyConnections, C::Limited),
            (ErrorCode::ChallengesFull, C::Limited),
            (ErrorCode::DeviceLinksFull, C::Limited),
            (ErrorCode::JoinRequestsFull, C::Limited),
            (ErrorCode::Unauthenticated, C::SessionEnded),
            (ErrorCode::ChallengeInvalid, C::Proof),
            (ErrorCode::ChallengeExpired, C::Proof),
            (ErrorCode::InvalidSignature, C::Proof),
            (ErrorCode::OriginRejected, C::Proof),
            (ErrorCode::UnknownDevice, C::UnknownDevice),
            (ErrorCode::AccountBanned, C::Banned),
            (ErrorCode::Forbidden, C::NotAllowed),
            (ErrorCode::OwnerOnly, C::NotAllowed),
            (ErrorCode::OwnerProtected, C::NotAllowed),
            (ErrorCode::OwnAccountTarget, C::NotAllowed),
            (ErrorCode::ExceedsOwnPermissions, C::NotAllowed),
            (ErrorCode::NotFound, C::NotFound),
            (ErrorCode::Conflict, C::Conflict),
            (ErrorCode::ChannelArchived, C::Conflict),
            (ErrorCode::LastDevice, C::Conflict),
            (ErrorCode::LimitReached, C::Conflict),
            (ErrorCode::KeyAlreadyRegistered, C::Conflict),
            (ErrorCode::OwnerAlreadyClaimed, C::Conflict),
            (ErrorCode::JoinRequestPending, C::Conflict),
            (ErrorCode::PublicKeyMismatch, C::Conflict),
            (ErrorCode::CallLocked, C::Conflict),
            (ErrorCode::CallFull, C::Conflict),
            (ErrorCode::NotInCall, C::Conflict),
            (ErrorCode::SeatEnded, C::Conflict),
            (ErrorCode::StorageLow, C::Conflict),
            (ErrorCode::RecordingsFull, C::Conflict),
            (ErrorCode::OwnerCodeInvalid, C::CodeRefused),
            (ErrorCode::InviteNotFound, C::CodeRefused),
            (ErrorCode::InviteExpired, C::CodeRefused),
            (ErrorCode::InviteExhausted, C::CodeRefused),
            (ErrorCode::InviteRevoked, C::CodeRefused),
            (ErrorCode::LinkCodeInvalid, C::CodeRefused),
            (ErrorCode::InvalidRequest, C::ClientFault),
            (ErrorCode::MethodNotAllowed, C::ClientFault),
            (ErrorCode::ImageTooLarge, C::ClientFault),
            (ErrorCode::ImageUnsupported, C::ClientFault),
            (ErrorCode::RangeNotSatisfiable, C::ClientFault),
            (ErrorCode::Unavailable, C::ServerFault),
            (ErrorCode::MediaUnavailable, C::ServerFault),
            (ErrorCode::RecorderUnavailable, C::ServerFault),
            (ErrorCode::Internal, C::ServerFault),
        ];
        assert_eq!(cases.len(), ErrorCode::ALL.len());
        for (code, expected) in cases {
            // The status of the response does not move a code that this build knows.
            for status in [code.http_status(), 400, 500] {
                assert_eq!(category(code, status), expected, "{code:?}");
            }
        }
        let unchanged: Vec<_> = ErrorCode::ALL
            .iter()
            .filter(|code| category(**code, code.http_status()) == C::Unchanged)
            .collect();
        assert_eq!(unchanged, [&ErrorCode::Timeout, &ErrorCode::Busy]);
    }

    #[test]
    fn a_code_of_a_newer_server_is_read_as_unknown_and_grouped_by_its_status() {
        let cases = [
            (400, ErrorCategory::ClientFault),
            (401, ErrorCategory::NotAllowed),
            (403, ErrorCategory::NotAllowed),
            (404, ErrorCategory::NotFound),
            (409, ErrorCategory::Conflict),
            (410, ErrorCategory::Conflict),
            (413, ErrorCategory::ClientFault),
            (429, ErrorCategory::Limited),
            (500, ErrorCategory::ServerFault),
            (503, ErrorCategory::ServerFault),
        ];
        for (status, expected) in cases {
            let body = r#"{"error":{"code":"quota_exceeded","message":"later","detail":7}}"#;
            let error = read_rejection(&response(status, body, Some(2)));
            assert_eq!(error.code(), Some(ErrorCode::Unknown));
            assert_eq!(error.category(), Some(expected), "{status}");
            assert_ne!(error.category(), Some(ErrorCategory::Unchanged));
            assert_ne!(error.category(), Some(ErrorCategory::SessionEnded));
            let rejection = error.rejection().unwrap();
            assert_eq!(rejection.status, status);
            assert_eq!(rejection.message, "later");
            assert_eq!(rejection.retry_after, Some(Duration::from_secs(2)));
            assert_eq!(
                error.to_string(),
                format!(
                    "the server refused the request with an error code that this client does \
                     not know (status {status}); the server says: \"later\""
                )
            );
        }
        assert_eq!(code_name(ErrorCode::Unknown), "unknown");
        assert_eq!(code_name(ErrorCode::OriginRejected), "origin_rejected");
    }

    #[test]
    fn a_body_in_another_form_is_an_invalid_response() {
        let bodies = [
            "",
            "<html>Bad Gateway</html>",
            "{}",
            r#"{"error":"busy"}"#,
            r#"{"error":{"message":"no code"}}"#,
            r#"{"error":{"code":"busy"}}"#,
            r#"{"error":{"code":7,"message":""}}"#,
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
    }

    #[test]
    fn unavailable_means_that_the_state_of_the_account_is_not_known() {
        let rejected = |code: ErrorCode| {
            let body = format!(
                r#"{{"error":{{"code":"{}","message":""}}}}"#,
                code_name(code)
            );
            read_rejection(&response(code.http_status(), &body, None))
        };
        assert!(rejected(ErrorCode::Busy).is_unavailable());
        assert!(rejected(ErrorCode::Timeout).is_unavailable());
        assert!(rejected(ErrorCode::Internal).is_unavailable());
        assert!(rejected(ErrorCode::Unavailable).is_unavailable());
        assert!(ApiError::OutcomeUnknown.is_unavailable());
        for code in [
            ErrorCode::AccountBanned,
            ErrorCode::UnknownDevice,
            ErrorCode::Unauthenticated,
            ErrorCode::RateLimited,
            ErrorCode::NotFound,
            ErrorCode::OriginRejected,
        ] {
            assert!(!rejected(code).is_unavailable(), "{code:?}");
        }
        let later = r#"{"error":{"code":"made_up_later","message":""}}"#;
        assert!(read_rejection(&response(503, later, None)).is_unavailable());
        assert!(!read_rejection(&response(409, later, None)).is_unavailable());
    }
}
