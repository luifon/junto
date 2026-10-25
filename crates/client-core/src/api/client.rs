//! The requests that need no session: the server information, and every operation that a
//! device proves with its key (ADR 0108).

use std::sync::Arc;

use junto_protocol::account::{DeviceLinkCreated, RequestDeviceLink};
use junto_protocol::account::{ModerationNoticePage, ReadModerationNotices, RecoveryNotice};
use junto_protocol::auth::{
    Challenge, ChallengeOperation, ClaimOwner, CreateChallenge, Login, Proof,
};
use junto_protocol::ids::ServerId;
use junto_protocol::ids::{AccountId, DeviceId};
use junto_protocol::invite::{
    Join, JoinRequest, JoinRequestStatus, JoinRequestStatusQuery, RequestToJoin,
    WithdrawJoinRequest,
};
use junto_protocol::secret::{InviteCode, OwnerCode};
use junto_protocol::server::ServerInfo;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::error::ApiError;
use super::proof::{self, Expected, ExpectedAccount};
use super::retry::{self, Repeat, RetryPolicy};
use super::session::{ServerApi, TokenAuthority};
use super::transport::{
    HttpRequest, HttpResponse, Method, OpenedResponse, RequestBody, Transport, TransportError,
    TransportErrorKind,
};
use crate::address::ServerAddress;
use crate::keys::DeviceKey;
use crate::secret::{SecretBytes, SecretToken};
use crate::version::ApiVersion;

/// The largest error body that the client reads in place of a file.
const LONGEST_ERROR_BODY: usize = 64 * 1024;

/// Asks the server at `address` for its identifier, name, logo and API versions. The route
/// has no version and needs no account.
pub async fn server_info(
    transport: &dyn Transport,
    address: &ServerAddress,
    policy: &RetryPolicy,
) -> Result<ServerInfo, ApiError> {
    let request = HttpRequest {
        method: Method::Get,
        url: address.endpoint("/api/server"),
        bearer: None,
        body: None,
    };
    let response = retry::send(transport, &request, Repeat::WhenUnchanged, policy).await?;
    read_json(&response)
}

pub(crate) fn read_json<R: DeserializeOwned>(response: &HttpResponse) -> Result<R, ApiError> {
    serde_json::from_slice(&response.body).map_err(|_| ApiError::InvalidResponse {
        status: response.status,
        reason: "unexpected body",
    })
}

/// Writes a request body into a buffer that is overwritten when it is dropped.
pub(crate) fn encode<B: Serialize>(body: &B) -> Result<RequestBody, ApiError> {
    let mut bytes = SecretBytes::with_capacity(512);
    serde_json::to_writer(&mut bytes, body)
        .map_err(|error| ApiError::Request(error.to_string()))?;
    Ok(RequestBody::json(bytes))
}

/// A session as the server created it. The token goes from the response into a buffer of
/// its own that is overwritten when the session is dropped.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Session {
    pub token: SecretToken,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    /// The recoveries of the account that this device has not acknowledged (ADR 0118).
    pub recoveries: Vec<RecoveryNotice>,
}

/// The API of one saved server in one API version.
#[derive(Clone)]
pub struct ApiClient {
    transport: Arc<dyn Transport>,
    address: ServerAddress,
    server_id: ServerId,
    version: ApiVersion,
    policy: RetryPolicy,
}

impl ApiClient {
    pub fn new(
        transport: Arc<dyn Transport>,
        address: ServerAddress,
        server_id: ServerId,
        version: ApiVersion,
        policy: RetryPolicy,
    ) -> Self {
        Self {
            transport,
            address,
            server_id,
            version,
            policy,
        }
    }

    pub fn server_id(&self) -> ServerId {
        self.server_id
    }

    pub fn version(&self) -> ApiVersion {
        self.version
    }

    pub fn address(&self) -> &ServerAddress {
        &self.address
    }

    /// The operations of a session, with a token that the caller holds. An ended session
    /// is reported and not renewed; a saved server renews it (`ServerHandle::api`).
    pub fn with_token(&self, token: SecretToken) -> ServerApi {
        ServerApi::new(Arc::new(TokenAuthority::new(self.clone(), token)))
    }

    pub(crate) async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&'static str, String)],
        bearer: Option<&SecretToken>,
        body: Option<RequestBody>,
        repeat: Repeat,
    ) -> Result<HttpResponse, ApiError> {
        let mut url = self
            .address
            .endpoint(&format!("{}{path}", self.version.prefix()));
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(name, value)| (*name, value.as_str())));
        }
        let request = HttpRequest {
            method,
            url,
            bearer: bearer.cloned(),
            body,
        };
        retry::send(self.transport.as_ref(), &request, repeat, &self.policy).await
    }

    /// Asks for a file of the API from the byte `from` on. A status of 400 or above is
    /// read as the error of the API and returned as one.
    pub(crate) async fn open(
        &self,
        path: &str,
        query: &[(&'static str, String)],
        bearer: &SecretToken,
        from: u64,
    ) -> Result<OpenedResponse, ApiError> {
        use futures_util::StreamExt as _;
        let mut url = self
            .address
            .endpoint(&format!("{}{path}", self.version.prefix()));
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(name, value)| (*name, value.as_str())));
        }
        let request = HttpRequest {
            method: Method::Get,
            url,
            bearer: Some(bearer.clone()),
            body: None,
        };
        let mut opened = self
            .transport
            .open(request, from)
            .await
            .map_err(ApiError::Unreachable)?;
        if opened.status < 300 {
            return Ok(opened);
        }
        if opened.status < 400 {
            return Err(ApiError::InvalidResponse {
                status: opened.status,
                reason: "unexpected status",
            });
        }
        let mut body = SecretBytes::with_capacity(512);
        while let Some(chunk) = opened.body.next().await {
            let chunk = chunk.map_err(ApiError::Unreachable)?;
            if body.len() + chunk.len() > LONGEST_ERROR_BODY {
                return Err(ApiError::Unreachable(TransportError::new(
                    TransportErrorKind::TooLarge,
                    "the error of a download",
                )));
            }
            body.extend(&chunk);
        }
        Err(super::error::read_rejection(&HttpResponse {
            status: opened.status,
            retry_after: opened.retry_after,
            body,
            receipt: crate::api::Receipt::NotStated,
        }))
    }

    /// Gets a challenge for `operation`, compares it with what was asked for, signs it and
    /// sends the request that `build` makes from the proof. The request is built once. The
    /// proof names the origin of the address of this client, which is where both requests
    /// go (ADR 0117).
    async fn prove<B, R>(
        &self,
        key: &DeviceKey,
        operation: ChallengeOperation,
        account: ExpectedAccount,
        path: &str,
        build: impl FnOnce(Proof) -> B,
    ) -> Result<(R, proof::VerifiedChallenge), ApiError>
    where
        B: Serialize,
        R: DeserializeOwned,
    {
        let (response, verified) = self
            .prove_answer(key, operation, account, path, build)
            .await?;
        // The server answered that it did the work. An answer that cannot be read is a
        // lost answer: the caller finds out what the server did.
        let answer = read_json(&response).map_err(|_| ApiError::OutcomeUnknown)?;
        Ok((answer, verified))
    }

    async fn prove_answer<B: Serialize>(
        &self,
        key: &DeviceKey,
        operation: ChallengeOperation,
        account: ExpectedAccount,
        path: &str,
        build: impl FnOnce(Proof) -> B,
    ) -> Result<(HttpResponse, proof::VerifiedChallenge), ApiError> {
        let public_key = key.public_key();
        let ask = encode(&CreateChallenge {
            public_key,
            operation,
        })?;
        let response = self
            .send(
                Method::Post,
                "/auth/challenges",
                &[],
                None,
                Some(ask),
                Repeat::WhenUnchanged,
            )
            .await?;
        let challenge: Challenge = read_json(&response)?;
        let verified = proof::verify(
            challenge,
            Expected {
                origin: self.address.origin().clone(),
                server_id: self.server_id,
                operation,
                public_key,
                account,
            },
        )?;
        let body = encode(&build(verified.sign(key)?))?;
        let response = self
            .send(Method::Post, path, &[], None, Some(body), Repeat::Signed)
            .await?;
        Ok((response, verified))
    }

    /// Starts a session. `account` is the account that the client stored for this device.
    pub async fn log_in(
        &self,
        key: &DeviceKey,
        account: ExpectedAccount,
    ) -> Result<Session, ApiError> {
        let (session, challenge): (Session, _) = self
            .prove(
                key,
                ChallengeOperation::Login,
                account,
                "/auth/sessions",
                |proof| Login { proof },
            )
            .await?;
        if challenge.account_id() != Some(session.account_id) {
            return Err(ApiError::InvalidResponse {
                status: 201,
                reason: "the session is for another account than the challenge",
            });
        }
        Ok(session)
    }

    pub async fn claim_owner(
        &self,
        key: &DeviceKey,
        owner_code: OwnerCode,
        name: String,
        device_name: String,
    ) -> Result<Session, ApiError> {
        let (session, _) = self
            .prove(
                key,
                ChallengeOperation::ClaimOwner,
                ExpectedAccount::None,
                "/owner/claim",
                |proof| ClaimOwner {
                    proof,
                    owner_code,
                    name,
                    device_name,
                },
            )
            .await?;
        Ok(session)
    }

    pub async fn join(
        &self,
        key: &DeviceKey,
        invite_code: InviteCode,
        name: String,
        device_name: String,
    ) -> Result<Session, ApiError> {
        let (session, _) = self
            .prove(
                key,
                ChallengeOperation::Join,
                ExpectedAccount::None,
                "/join",
                |proof| Join {
                    proof,
                    invite_code,
                    name,
                    device_name,
                },
            )
            .await?;
        Ok(session)
    }

    pub async fn request_to_join(
        &self,
        key: &DeviceKey,
        name: String,
    ) -> Result<JoinRequest, ApiError> {
        let (request, _) = self
            .prove(
                key,
                ChallengeOperation::RequestToJoin,
                ExpectedAccount::None,
                "/join-requests",
                |proof| RequestToJoin { proof, name },
            )
            .await?;
        Ok(request)
    }

    /// Withdraws the waiting join request of this device key. The server answers the same
    /// when no request of the key waits.
    pub async fn withdraw_join_request(&self, key: &DeviceKey) -> Result<(), ApiError> {
        self.prove_answer(
            key,
            ChallengeOperation::WithdrawJoinRequest,
            ExpectedAccount::None,
            "/join-requests/withdrawal",
            |proof| WithdrawJoinRequest { proof },
        )
        .await?;
        Ok(())
    }

    /// The state of the newest join request of this device key that was not withdrawn.
    pub async fn join_request_status(
        &self,
        key: &DeviceKey,
    ) -> Result<JoinRequestStatus, ApiError> {
        let (status, _) = self
            .prove(
                key,
                ChallengeOperation::JoinRequestStatus,
                ExpectedAccount::None,
                "/join-requests/status",
                |proof| JoinRequestStatusQuery { proof },
            )
            .await?;
        Ok(status)
    }

    /// The private moderation notices that are addressed to this device key (ADR 0120).
    /// The device proves its key without a session: a removed or banned account has none.
    /// A key without notices gets an empty page.
    pub async fn moderation_notices(
        &self,
        key: &DeviceKey,
        before: Option<uuid::Uuid>,
    ) -> Result<ModerationNoticePage, ApiError> {
        let (page, _) = self
            .prove(
                key,
                ChallengeOperation::ModerationNotices,
                ExpectedAccount::None,
                "/auth/moderation-notices",
                |proof| ReadModerationNotices { proof, before },
            )
            .await?;
        Ok(page)
    }

    /// Asks for the code that a logged-in device of the account approves.
    pub async fn request_device_link(
        &self,
        key: &DeviceKey,
        device_name: String,
    ) -> Result<DeviceLinkCreated, ApiError> {
        let (link, _) = self
            .prove(
                key,
                ChallengeOperation::LinkDevice,
                ExpectedAccount::None,
                "/device-links",
                |proof| RequestDeviceLink { proof, device_name },
            )
            .await?;
        Ok(link)
    }
}
