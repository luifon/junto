use serde::{Deserialize, Serialize};

/// The stable machine-readable code of an error. A client decides on the code, never on the
/// message.
///
/// A server can add codes within one API version (ADR 0209). A code that this build does not
/// know is read as [`ErrorCode::Unknown`]; the status of the response then tells the kind of
/// the error. `Unknown` is never written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthenticated,
    ChallengeInvalid,
    ChallengeExpired,
    InvalidSignature,
    /// The signature names an origin that is not one of the public origins of this server.
    OriginRejected,
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
    LimitReached,
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
    TooManyConnections,
    LinkCodeInvalid,
    PublicKeyMismatch,
    /// The image has more bytes than the server stores.
    ImageTooLarge,
    /// The bytes are not a PNG, JPEG or WebP image of an accepted size.
    ImageUnsupported,
    /// The voice channel is locked and the caller has no invitation.
    CallLocked,
    /// The voice channel holds as many people as its user limit allows.
    CallFull,
    /// The caller is not in the call that the request is about.
    NotInCall,
    /// The server does not reach its media server. No call can be joined.
    MediaUnavailable,
    /// The server has no recorder, or no recorder took the request. No recording starts.
    RecorderUnavailable,
    /// The free disk space of the server is below what a new recording needs (ADR 0006).
    StorageLow,
    /// As many recordings run as the server allows at one time (ADR 0006).
    RecordingsFull,
    /// The byte range of the request lies outside the file.
    RangeNotSatisfiable,
    MethodNotAllowed,
    Timeout,
    Busy,
    Unavailable,
    Internal,
    /// A code that this build does not know. It is read and never written.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl ErrorCode {
    /// Every code that this build knows, without [`ErrorCode::Unknown`].
    pub const ALL: &'static [Self] = &[
        Self::InvalidRequest,
        Self::Unauthenticated,
        Self::ChallengeInvalid,
        Self::ChallengeExpired,
        Self::InvalidSignature,
        Self::OriginRejected,
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
        Self::LimitReached,
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
        Self::TooManyConnections,
        Self::LinkCodeInvalid,
        Self::PublicKeyMismatch,
        Self::ImageTooLarge,
        Self::ImageUnsupported,
        Self::CallLocked,
        Self::CallFull,
        Self::NotInCall,
        Self::MediaUnavailable,
        Self::RecorderUnavailable,
        Self::StorageLow,
        Self::RecordingsFull,
        Self::RangeNotSatisfiable,
        Self::MethodNotAllowed,
        Self::Timeout,
        Self::Busy,
        Self::Unavailable,
        Self::Internal,
    ];

    /// The one status of the code. For [`ErrorCode::Unknown`] it is 500; a client takes the
    /// status of such an error from the response.
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
            | Self::OwnerCodeInvalid
            | Self::OriginRejected => 403,
            Self::NotFound | Self::InviteNotFound | Self::LinkCodeInvalid => 404,
            Self::Conflict
            | Self::LimitReached
            | Self::KeyAlreadyRegistered
            | Self::OwnerAlreadyClaimed
            | Self::JoinRequestPending
            | Self::PublicKeyMismatch
            | Self::CallLocked
            | Self::CallFull
            | Self::NotInCall => 409,
            Self::InviteExpired | Self::InviteExhausted | Self::InviteRevoked => 410,
            Self::TooManyAttempts
            | Self::RateLimited
            | Self::JoinRequestsFull
            | Self::ChallengesFull
            | Self::DeviceLinksFull
            | Self::TooManyConnections
            | Self::RecordingsFull => 429,
            Self::MethodNotAllowed => 405,
            Self::ImageTooLarge => 413,
            Self::ImageUnsupported => 415,
            Self::RangeNotSatisfiable => 416,
            Self::Timeout
            | Self::Busy
            | Self::Unavailable
            | Self::MediaUnavailable
            | Self::RecorderUnavailable
            | Self::StorageLow => 503,
            Self::Internal | Self::Unknown => 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ErrorDetail {
    pub code: ErrorCode,
    /// A sentence for a person who reads a log. Its text is not stable.
    pub message: String,
}

/// The body of every response with a status of 400 or above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    fn a_code_of_a_newer_server_is_read_as_unknown_and_never_written() {
        let body: ErrorBody = serde_json::from_str(
            r#"{"error":{"code":"quota_exceeded","message":"later","new_field":1}}"#,
        )
        .unwrap();
        assert_eq!(body.error.code, ErrorCode::Unknown);
        assert_eq!(body.error.message, "later");
        assert!(serde_json::to_string(&ErrorCode::Unknown).is_err());
        assert!(serde_json::to_string(&body).is_err());
        assert!(!ErrorCode::ALL.contains(&ErrorCode::Unknown));
        assert_eq!(
            serde_json::from_str::<ErrorCode>("\"unknown\"").unwrap(),
            ErrorCode::Unknown
        );
        assert!(serde_json::from_str::<ErrorCode>("7").is_err());
    }

    #[test]
    fn every_code_maps_to_an_error_status() {
        for code in ErrorCode::ALL {
            assert!((400..=503).contains(&code.http_status()), "{code:?}");
        }
    }
}
