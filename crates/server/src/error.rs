use std::borrow::Cow;
use std::collections::BTreeMap;

use axum::Json;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use junto_authz::{Denial, WorkspacePermission};
use junto_protocol::error::{ErrorBody, ErrorCode, ErrorDetail};
use time::Duration;
use utoipa::openapi::{ContentBuilder, Ref, RefOr, ResponseBuilder, ResponsesBuilder};

use crate::database::TransactionError;
use crate::secrets::RandomnessUnavailable;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
pub struct ApiError {
    code: ErrorCode,
    message: Cow<'static, str>,
    retry_after: Option<Duration>,
    retryable: bool,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after: None,
            retryable: false,
        }
    }

    pub fn invalid(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    /// One response for every absent object and for every object that the caller may not
    /// see, so that the two cannot be told apart.
    pub fn not_found() -> Self {
        Self::new(ErrorCode::NotFound, "not found")
    }

    pub fn conflict(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            ErrorCode::Unauthenticated,
            "a valid session token is required",
        )
    }

    /// The request reached a time limit and changed nothing.
    pub fn timeout() -> Self {
        Self::new(
            ErrorCode::Timeout,
            "the request took too long and changed nothing; send the same request again",
        )
        .retry_after(Duration::seconds(1))
    }

    pub fn retry_after(mut self, wait: Duration) -> Self {
        self.retry_after = Some(wait);
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Logs the cause and returns an error that tells the caller nothing about it.
    pub fn internal(cause: &dyn std::fmt::Display) -> Self {
        tracing::error!(%cause, "request failed with an internal error");
        Self::new(ErrorCode::Internal, "internal error")
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if crate::database::is_conflict(&error) {
            let mut busy = <Self as TransactionError>::exhausted();
            busy.retryable = true;
            busy
        } else if crate::database::is_timeout(&error) {
            Self::timeout()
        } else {
            Self::internal(&error)
        }
    }
}

impl TransactionError for ApiError {
    fn is_conflict(&self) -> bool {
        self.retryable
    }

    fn exhausted() -> Self {
        Self::new(
            ErrorCode::Busy,
            "the server is busy and changed nothing; send the same request again",
        )
        .retry_after(Duration::seconds(1))
    }
}

impl From<RandomnessUnavailable> for ApiError {
    fn from(error: RandomnessUnavailable) -> Self {
        Self::internal(&error)
    }
}

impl From<Denial> for ApiError {
    fn from(denial: Denial) -> Self {
        match denial {
            Denial::NotParticipant
            | Denial::NotMember
            | Denial::MissingWorkspacePermission(WorkspacePermission::ViewChannel) => {
                Self::not_found()
            }
            Denial::MissingWorkspacePermission(permission) => Self::new(
                ErrorCode::Forbidden,
                format!("the workspace permission {} is required", permission.name()),
            ),
            Denial::MissingServerPermission(permission) => Self::new(
                ErrorCode::Forbidden,
                format!("the server permission {} is required", permission.name()),
            ),
            Denial::PermanentMessage => {
                Self::new(ErrorCode::Forbidden, "a permanent message cannot be edited")
            }
            Denial::NotAuthor => Self::new(ErrorCode::Forbidden, "only the author can do this"),
            Denial::OwnerOnly => {
                Self::new(ErrorCode::OwnerOnly, "only the server owner can do this")
            }
            Denial::OwnerProtected => Self::new(
                ErrorCode::OwnerProtected,
                "this cannot be done to the server owner",
            ),
            Denial::OwnAccountTarget => Self::new(
                ErrorCode::OwnAccountTarget,
                "this cannot be done to the caller's own account",
            ),
            Denial::ExceedsOwnPermissions => Self::new(
                ErrorCode::ExceedsOwnPermissions,
                "the change grants a permission that the caller does not hold",
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.code.http_status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: self.message.into_owned(),
            },
        };
        let mut response = (status, Json(body)).into_response();
        if let Some(wait) = self.retry_after {
            let seconds = wait.whole_seconds().max(1);
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

impl utoipa::IntoResponses for ApiError {
    fn responses() -> BTreeMap<String, RefOr<utoipa::openapi::Response>> {
        ResponsesBuilder::new()
            .response(
                "default",
                ResponseBuilder::new()
                    .description("An error. The field `error.code` states which one.")
                    .content(
                        "application/json",
                        ContentBuilder::new()
                            .schema(Some(Ref::from_schema_name("ErrorBody")))
                            .build(),
                    ),
            )
            .build()
            .into()
    }
}

pub fn violates(error: &sqlx::Error, constraint: &str) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.constraint())
        == Some(constraint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_channel_and_a_foreign_workspace_look_like_missing_resources() {
        for denial in [
            Denial::NotMember,
            Denial::NotParticipant,
            Denial::MissingWorkspacePermission(WorkspacePermission::ViewChannel),
        ] {
            assert_eq!(ApiError::from(denial).code(), ErrorCode::NotFound);
        }
    }

    #[test]
    fn denials_keep_their_own_codes() {
        let cases = [
            (Denial::OwnerOnly, ErrorCode::OwnerOnly),
            (Denial::OwnerProtected, ErrorCode::OwnerProtected),
            (Denial::OwnAccountTarget, ErrorCode::OwnAccountTarget),
            (
                Denial::ExceedsOwnPermissions,
                ErrorCode::ExceedsOwnPermissions,
            ),
            (
                Denial::MissingWorkspacePermission(WorkspacePermission::ManageRoles),
                ErrorCode::Forbidden,
            ),
            (
                Denial::MissingServerPermission(junto_authz::ServerPermission::BanMembers),
                ErrorCode::Forbidden,
            ),
        ];
        for (denial, code) in cases {
            assert_eq!(ApiError::from(denial).code(), code);
        }
    }

    #[test]
    fn retry_after_is_sent_in_whole_seconds() {
        let response = ApiError::new(ErrorCode::RateLimited, "slow down")
            .retry_after(Duration::milliseconds(2_500))
            .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[RETRY_AFTER], "2");
    }
}
