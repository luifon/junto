use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::rate_limit::client_address;
use crate::secrets::{SESSION_TOKEN_PREFIX, hash_session_token};
use crate::state::AppState;

const FORWARDED_FOR: &str = "x-forwarded-for";

pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::invalid(rejection.body_text())),
        }
    }
}

pub struct ApiPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::invalid(rejection.body_text())),
        }
    }
}

pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::invalid(rejection.body_text())),
        }
    }
}

/// The hash of the session token of a request. The session behind it is verified inside the
/// transaction of the request, by `Access::open`.
#[derive(Debug, Clone, Copy)]
pub struct Bearer(pub [u8; 32]);

impl<S: Send + Sync> FromRequestParts<S> for Bearer {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|token| token.starts_with(SESSION_TOKEN_PREFIX))
            .map(|token| Self(hash_session_token(token)))
            .ok_or_else(ApiError::unauthenticated)
    }
}

/// The address that the limits on attempts count. See [`client_address`].
#[derive(Debug, Clone, Copy)]
pub struct ClientAddress(pub IpAddr);

impl FromRequestParts<AppState> for ClientAddress {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip())
            .ok_or_else(|| ApiError::internal(&"the connection has no peer address"))?;
        let forwarded: Vec<&str> = parts
            .headers
            .get_all(FORWARDED_FOR)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        Ok(Self(client_address(
            peer,
            &forwarded,
            &app.config.trusted_proxies,
        )))
    }
}
