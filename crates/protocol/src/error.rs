use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The stable machine-readable code of an error. A client decides on the code, never on the
/// message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthenticated,
    ChallengeInvalid,
    ChallengeExpired,
    InvalidSignature,
    UnknownDevice,
    TooManyAttempts,
    RateLimited,
    AccountBanned,
    Forbidden,
    OwnerOnly,
    OwnerProtected,
    OwnAccountTarget,
    ExceedsOwnPermissions,
    NotFound,
    Conflict,
    KeyAlreadyRegistered,
    OwnerAlreadyClaimed,
    OwnerCodeInvalid,
    InviteNotFound,
    InviteExpired,
    InviteExhausted,
    InviteRevoked,
    JoinRequestPending,
    JoinRequestsFull,
    ChallengesFull,
    DeviceLinksFull,
    LinkCodeInvalid,
    PublicKeyMismatch,
    MethodNotAllowed,
    Timeout,
    Busy,
    Internal,
}

impl ErrorCode {
    pub const ALL: &'static [Self] = &[
        Self::InvalidRequest,
        Self::Unauthenticated,
        Self::ChallengeInvalid,
        Self::ChallengeExpired,
        Self::InvalidSignature,
        Self::UnknownDevice,
        Self::TooManyAttempts,
        Self::RateLimited,
        Self::AccountBanned,
        Self::Forbidden,
        Self::OwnerOnly,
        Self::OwnerProtected,
        Self::OwnAccountTarget,
        Self::ExceedsOwnPermissions,
        Self::NotFound,
        Self::Conflict,
        Self::KeyAlreadyRegistered,
        Self::OwnerAlreadyClaimed,
        Self::OwnerCodeInvalid,
        Self::InviteNotFound,
        Self::InviteExpired,
        Self::InviteExhausted,
        Self::InviteRevoked,
        Self::JoinRequestPending,
        Self::JoinRequestsFull,
        Self::ChallengesFull,
        Self::DeviceLinksFull,
        Self::LinkCodeInvalid,
        Self::PublicKeyMismatch,
        Self::MethodNotAllowed,
        Self::Timeout,
        Self::Busy,
        Self::Internal,
    ];

    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequest => 400,
            Self::Unauthenticated
            | Self::ChallengeInvalid
            | Self::ChallengeExpired
            | Self::InvalidSignature
            | Self::UnknownDevice => 401,
            Self::AccountBanned
            | Self::Forbidden
            | Self::OwnerOnly
            | Self::OwnerProtected
            | Self::OwnAccountTarget
            | Self::ExceedsOwnPermissions
            | Self::OwnerCodeInvalid => 403,
            Self::NotFound | Self::InviteNotFound | Self::LinkCodeInvalid => 404,
            Self::Conflict
            | Self::KeyAlreadyRegistered
            | Self::OwnerAlreadyClaimed
            | Self::JoinRequestPending
            | Self::PublicKeyMismatch => 409,
            Self::InviteExpired | Self::InviteExhausted | Self::InviteRevoked => 410,
            Self::TooManyAttempts
            | Self::RateLimited
            | Self::JoinRequestsFull
            | Self::ChallengesFull
            | Self::DeviceLinksFull => 429,
            Self::MethodNotAllowed => 405,
            Self::Timeout | Self::Busy => 503,
            Self::Internal => 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    /// A sentence for a person who reads a log. Its text is not stable.
    pub message: String,
}

/// The body of every response with a status of 400 or above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_snake_case_and_unique_on_the_wire() {
        let names: Vec<String> = ErrorCode::ALL
            .iter()
            .map(|code| {
                serde_json::to_value(code)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        for name in &names {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name}"
            );
        }
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
        assert_eq!(names[0], "invalid_request");
        assert_eq!(names[names.len() - 1], "internal");
    }

    #[test]
    fn every_code_maps_to_an_error_status() {
        for code in ErrorCode::ALL {
            assert!((400..=503).contains(&code.http_status()), "{code:?}");
        }
    }
}
