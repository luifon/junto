//! Every operation that needs a session, written once. An authority adds the session
//! token to each request: a fixed token, or the session that a saved server keeps and
//! renews.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use junto_protocol::account::{
    AccountProfile, AccountSummary, ApproveDeviceLink, Avatar, Device, ImageType, ListedAccount,
    LookUpDeviceLink, ModerationNoticePage, OwnAccount, PendingDeviceLink, RecoverAccount,
    RecoveryNotice, RenameDevice, UpdateProfile,
};
use junto_protocol::ids::{
    AccountId, CategoryId, ChannelId, DeviceId, InviteId, JoinRequestId, RecoveryId, ServerRoleId,
    WorkspaceId, WorkspaceRoleId,
};
use junto_protocol::invite::{
    ApproveJoinRequest, CreateInvite, Invite, InviteCreated, JoinRequest, JoinRequestStatus,
    RedeemInvite, Redemption,
};
use junto_protocol::keys::PublicKey;
use junto_protocol::roles::{
    CreateServerRole, CreateWorkspaceRole, ListedServerRole, ListedWorkspaceRole, ServerRole,
    UpdateServerRole, UpdateWorkspaceRole, WorkspaceRole,
};
use junto_protocol::secret::{InviteCode, LinkCode};
use junto_protocol::structure::{
    Category, Channel, ChannelList, ChannelOverride, CreateCategory, CreateChannel,
    CreateWorkspace, MoveChannels, SetChannelOverride, UpdateCategory, UpdateChannel,
    UpdateWorkspace, Workspace, WorkspaceMember,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::client::{ApiClient, encode, read_json};
use super::error::ApiError;
use super::retry::Repeat;
use super::transport::{HttpResponse, Method, OpenedResponse, Receipt, RequestBody};
use crate::profile::{self, InvalidAvatar};
use crate::secret::{SecretBytes, SecretToken};

/// A request of a session without its token and without the version prefix.
#[derive(Clone)]
pub(crate) struct Outline {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) query: Vec<(&'static str, String)>,
    pub(crate) body: Option<RequestBody>,
    /// The body is one part of a file: the request gets the time of a slow connection.
    pub(crate) part: bool,
}

pub(crate) type AuthorityFuture =
    Pin<Box<dyn Future<Output = Result<HttpResponse, ApiError>> + Send + 'static>>;

pub(crate) type OpenAuthorityFuture =
    Pin<Box<dyn Future<Output = Result<OpenedResponse, ApiError>> + Send + 'static>>;

pub(crate) trait Authority: Send + Sync + 'static {
    fn send(&self, outline: Outline) -> AuthorityFuture;

    /// Asks for a file from the byte `from` on. The answer has a status below 400.
    fn open(&self, outline: Outline, from: u64) -> OpenAuthorityFuture;
}

pub(crate) struct TokenAuthority {
    api: ApiClient,
    token: SecretToken,
}

impl TokenAuthority {
    pub(crate) fn new(api: ApiClient, token: SecretToken) -> Self {
        Self { api, token }
    }
}

impl Authority for TokenAuthority {
    fn send(&self, outline: Outline) -> AuthorityFuture {
        let (api, token) = (self.api.clone(), self.token.clone());
        Box::pin(async move { send_with_token(&api, &token, outline).await })
    }

    fn open(&self, outline: Outline, from: u64) -> OpenAuthorityFuture {
        let (api, token) = (self.api.clone(), self.token.clone());
        Box::pin(async move { open_with_token(&api, &token, outline, from).await })
    }
}

pub(crate) async fn send_with_token(
    api: &ApiClient,
    token: &SecretToken,
    outline: Outline,
) -> Result<HttpResponse, ApiError> {
    let repeat = if outline.part {
        Repeat::Part
    } else {
        Repeat::WhenUnchanged
    };
    api.send(
        outline.method,
        &outline.path,
        &outline.query,
        Some(token),
        outline.body,
        repeat,
    )
    .await
}

pub(crate) async fn open_with_token(
    api: &ApiClient,
    token: &SecretToken,
    outline: Outline,
    from: u64,
) -> Result<OpenedResponse, ApiError> {
    api.open(&outline.path, &outline.query, token, from).await
}

impl ApiClient {
    /// Ends the session of `token`.
    pub async fn log_out(&self, token: &SecretToken) -> Result<(), ApiError> {
        self.send(
            Method::Delete,
            "/auth/session",
            &[],
            Some(token),
            None,
            Repeat::WhenUnchanged,
        )
        .await
        .map(|_| ())
    }
}

/// The image of an avatar as the client read it. The server checked the header of the
/// image and not its data (ADR 0119): the image is a file of an unknown sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarImage {
    /// The format that the first bytes state. The `Content-Type` of the response is not read.
    pub media_type: ImageType,
    /// The SHA-256 hash of the bytes in lower-case hexadecimal, computed by the client. It
    /// equals [`Avatar::hash`] of the profile while the avatar is the one that the profile
    /// describes.
    pub hash: String,
    pub bytes: Vec<u8>,
}

/// The answer to a change, with what the answer states about the position of the events of
/// the change (ADR 0214). A caller that shows the change until the state holds it compares
/// the receipt with the state (`ServerModel::followed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed<T> {
    pub value: T,
    pub receipt: Receipt,
}

impl<T> Changed<T> {
    pub fn map<U>(self, change: impl FnOnce(T) -> U) -> Changed<U> {
        Changed {
            value: change(self.value),
            receipt: self.receipt,
        }
    }
}

/// The operations of a logged-in account on one server.
#[derive(Clone)]
pub struct ServerApi {
    authority: Arc<dyn Authority>,
}

impl ServerApi {
    pub(crate) fn new(authority: Arc<dyn Authority>) -> Self {
        Self { authority }
    }

    pub(super) async fn call(
        &self,
        method: Method,
        path: String,
        query: Vec<(&'static str, String)>,
        body: Option<RequestBody>,
    ) -> Result<HttpResponse, ApiError> {
        self.authority
            .send(Outline {
                method,
                path,
                query,
                body,
                part: false,
            })
            .await
    }

    /// Sends one part of a file.
    pub(super) async fn call_part(
        &self,
        path: String,
        body: RequestBody,
    ) -> Result<HttpResponse, ApiError> {
        self.authority
            .send(Outline {
                method: Method::Put,
                path,
                query: Vec::new(),
                body: Some(body),
                part: true,
            })
            .await
    }

    pub(super) async fn open(&self, path: String, from: u64) -> Result<OpenedResponse, ApiError> {
        let outline = Outline {
            method: Method::Get,
            path,
            query: Vec::new(),
            body: None,
            part: false,
        };
        self.authority.open(outline, from).await
    }

    pub(super) async fn get<R: DeserializeOwned>(&self, path: String) -> Result<R, ApiError> {
        read_json(&self.call(Method::Get, path, Vec::new(), None).await?)
    }

    pub(super) async fn send<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        body: &B,
    ) -> Result<R, ApiError> {
        let body = Some(encode(body)?);
        read_json(&self.call(method, path, Vec::new(), body).await?)
    }

    pub(super) async fn act(&self, method: Method, path: String) -> Result<(), ApiError> {
        self.call(method, path, Vec::new(), None).await.map(|_| ())
    }

    /// Sends a change with a JSON body and returns its answer with its receipt.
    pub(super) async fn change<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        body: Option<&B>,
    ) -> Result<Changed<R>, ApiError> {
        let body = body.map(encode).transpose()?;
        let response = self.call(method, path, Vec::new(), body).await?;
        Ok(Changed {
            value: read_json(&response)?,
            receipt: response.receipt,
        })
    }

    /// Sends a change whose answer has no body that the client reads, and returns its
    /// receipt.
    pub(super) async fn change_without_body(
        &self,
        method: Method,
        path: String,
    ) -> Result<Receipt, ApiError> {
        Ok(self.call(method, path, Vec::new(), None).await?.receipt)
    }

    // The account of the caller and its devices.

    pub async fn own_account(&self) -> Result<OwnAccount, ApiError> {
        self.get("/account".to_owned()).await
    }

    /// Sets the name that the account shows (ADR 0119). The name is checked by the rule of
    /// the server before the request.
    pub async fn update_profile(&self, name: &str) -> Result<OwnAccount, ApiError> {
        let name = profile::account_name(name)?;
        let path = "/account".to_owned();
        self.send(Method::Patch, path, &UpdateProfile { name })
            .await
    }

    /// Stores the avatar of the account. The body of the request is the image itself. An
    /// image that no server accepts by its size or its first bytes is not sent.
    pub async fn set_avatar(&self, image: Vec<u8>) -> Result<Avatar, ApiError> {
        let kind = profile::check_avatar(&image)?;
        let body = RequestBody::image(kind, SecretBytes::from_vec(image));
        let path = "/account/avatar".to_owned();
        read_json(&self.call(Method::Put, path, Vec::new(), Some(body)).await?)
    }

    pub async fn remove_avatar(&self) -> Result<(), ApiError> {
        self.act(Method::Delete, "/account/avatar".to_owned()).await
    }

    /// The name and the description of the avatar of an account that this account may see.
    pub async fn profile(&self, account_id: AccountId) -> Result<AccountProfile, ApiError> {
        self.get(format!("/accounts/{account_id}/profile")).await
    }

    /// The avatar of an account that this account may see. An answer that is larger than an
    /// avatar can be, or that is no PNG, JPEG or WebP image by its first bytes, is refused.
    pub async fn avatar(&self, account_id: AccountId) -> Result<AvatarImage, ApiError> {
        let path = format!("/accounts/{account_id}/avatar");
        let response = self.call(Method::Get, path, Vec::new(), None).await?;
        let media_type =
            profile::check_avatar(&response.body).map_err(|invalid| ApiError::InvalidResponse {
                status: response.status,
                reason: match invalid {
                    InvalidAvatar::TooLarge => "the avatar is too large",
                    InvalidAvatar::Unsupported => "the avatar is not an image",
                },
            })?;
        Ok(AvatarImage {
            media_type,
            hash: profile::avatar_hash(&response.body),
            bytes: response.body.to_vec(),
        })
    }

    pub async fn devices(&self) -> Result<Vec<Device>, ApiError> {
        self.get("/account/devices".to_owned()).await
    }

    pub async fn rename_device(
        &self,
        device_id: DeviceId,
        name: String,
    ) -> Result<Device, ApiError> {
        let path = format!("/account/devices/{device_id}");
        self.send(Method::Patch, path, &RenameDevice { name }).await
    }

    /// Removes a device of the account and ends its sessions. The last device of an
    /// account cannot be removed.
    pub async fn remove_device(&self, device_id: DeviceId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/account/devices/{device_id}"))
            .await
    }

    /// Every recovery of the account.
    pub async fn recoveries(&self) -> Result<Vec<RecoveryNotice>, ApiError> {
        self.get("/account/recoveries".to_owned()).await
    }

    /// The recoveries that this device has not acknowledged, oldest first (ADR 0118).
    pub async fn recovery_notices(&self) -> Result<Vec<RecoveryNotice>, ApiError> {
        self.get("/account/recovery-notices".to_owned()).await
    }

    /// States that this device has shown a recovery to its user. The notice of the other
    /// devices of the account stays. A saved server acknowledges through
    /// `ServerHandle::acknowledge_recovery`, which also updates the state of its flow.
    pub async fn acknowledge_recovery(&self, recovery_id: RecoveryId) -> Result<(), ApiError> {
        let path = format!("/account/recovery-notices/{recovery_id}");
        self.act(Method::Delete, path).await
    }

    /// The private moderation notices of the account, the newest first, before the notice
    /// `before` (ADR 0120).
    pub async fn moderation_notices(
        &self,
        before: Option<uuid::Uuid>,
    ) -> Result<ModerationNoticePage, ApiError> {
        let query = before
            .map(|before| vec![("before", before.to_string())])
            .unwrap_or_default();
        let path = "/account/moderation-notices".to_owned();
        read_json(&self.call(Method::Get, path, query, None).await?)
    }

    /// The public key and the device name behind a link code, for the user to compare.
    pub async fn look_up_device_link(&self, code: LinkCode) -> Result<PendingDeviceLink, ApiError> {
        let path = "/account/device-links/lookup".to_owned();
        self.send(Method::Post, path, &LookUpDeviceLink { code })
            .await
    }

    /// Adds the device with `public_key` to the account. The server refuses when the key
    /// behind the code is another one.
    pub async fn approve_device_link(
        &self,
        code: LinkCode,
        public_key: PublicKey,
    ) -> Result<Device, ApiError> {
        let path = "/account/device-links/approve".to_owned();
        self.send(Method::Post, path, &ApproveDeviceLink { code, public_key })
            .await
    }

    // Invites and join requests.

    pub async fn create_invite(&self, invite: &CreateInvite) -> Result<InviteCreated, ApiError> {
        self.send(Method::Post, "/invites".to_owned(), invite).await
    }

    pub async fn invites(&self) -> Result<Vec<Invite>, ApiError> {
        self.get("/invites".to_owned()).await
    }

    pub async fn revoke_invite(&self, invite_id: InviteId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/invites/{invite_id}"))
            .await
    }

    /// Enters the workspaces of an invite with the account that is logged in.
    pub async fn redeem_invite(&self, invite_code: InviteCode) -> Result<Redemption, ApiError> {
        let path = "/invites/redemption".to_owned();
        self.send(Method::Post, path, &RedeemInvite { invite_code })
            .await
    }

    pub async fn join_requests(&self) -> Result<Vec<JoinRequest>, ApiError> {
        self.get("/join-requests".to_owned()).await
    }

    pub async fn approve_join_request(
        &self,
        join_request_id: JoinRequestId,
        approval: &ApproveJoinRequest,
    ) -> Result<JoinRequestStatus, ApiError> {
        let path = format!("/join-requests/{join_request_id}/approval");
        self.send(Method::Post, path, approval).await
    }

    pub async fn deny_join_request(
        &self,
        join_request_id: JoinRequestId,
    ) -> Result<JoinRequestStatus, ApiError> {
        let path = format!("/join-requests/{join_request_id}/denial");
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    // Moderation.

    pub async fn accounts(&self) -> Result<Vec<AccountSummary>, ApiError> {
        let listed = self.listed_accounts().await?;
        Ok(listed.into_iter().map(|listed| listed.account).collect())
    }

    /// Every account, each with whether the caller may unban it (ADR 0311).
    pub async fn listed_accounts(&self) -> Result<Vec<ListedAccount>, ApiError> {
        self.get("/accounts".to_owned()).await
    }

    pub async fn unban_account(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/accounts/{account_id}/ban"))
            .await
    }

    /// Gives a new device access to an account. Only the server owner can do this.
    pub async fn recover_account(
        &self,
        account_id: AccountId,
        recovery: &RecoverAccount,
    ) -> Result<RecoveryNotice, ApiError> {
        let path = format!("/accounts/{account_id}/recovery");
        self.send(Method::Post, path, recovery).await
    }

    // Server roles.

    pub async fn server_roles(&self) -> Result<Vec<ServerRole>, ApiError> {
        let listed = self.listed_server_roles().await?;
        Ok(listed.into_iter().map(|listed| listed.role).collect())
    }

    /// The server roles, each with what the caller may do with it (ADR 0311).
    pub async fn listed_server_roles(&self) -> Result<Vec<ListedServerRole>, ApiError> {
        self.get("/server-roles".to_owned()).await
    }

    pub async fn create_server_role(
        &self,
        role: &CreateServerRole,
    ) -> Result<ServerRole, ApiError> {
        self.send(Method::Post, "/server-roles".to_owned(), role)
            .await
    }

    pub async fn update_server_role(
        &self,
        role_id: ServerRoleId,
        change: &UpdateServerRole,
    ) -> Result<ServerRole, ApiError> {
        self.send(Method::Patch, format!("/server-roles/{role_id}"), change)
            .await
    }

    pub async fn assign_server_role(
        &self,
        account_id: AccountId,
        role_id: ServerRoleId,
    ) -> Result<(), ApiError> {
        let path = format!("/accounts/{account_id}/server-roles/{role_id}");
        self.act(Method::Put, path).await
    }

    pub async fn unassign_server_role(
        &self,
        account_id: AccountId,
        role_id: ServerRoleId,
    ) -> Result<(), ApiError> {
        let path = format!("/accounts/{account_id}/server-roles/{role_id}");
        self.act(Method::Delete, path).await
    }

    // Workspaces and their roles.

    pub async fn workspaces(&self) -> Result<Vec<Workspace>, ApiError> {
        self.get("/workspaces".to_owned()).await
    }

    pub async fn create_workspace(&self, name: String) -> Result<Workspace, ApiError> {
        let path = "/workspaces".to_owned();
        self.send(Method::Post, path, &CreateWorkspace { name })
            .await
    }

    pub async fn rename_workspace(
        &self,
        workspace_id: WorkspaceId,
        name: String,
    ) -> Result<Workspace, ApiError> {
        let path = format!("/workspaces/{workspace_id}");
        self.send(Method::Patch, path, &UpdateWorkspace { name })
            .await
    }

    pub async fn workspace_members(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<WorkspaceMember>, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/members"))
            .await
    }

    /// Adds an existing account to a workspace (ADR 0115).
    pub async fn add_workspace_member(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        let path = format!("/workspaces/{workspace_id}/members/{account_id}");
        self.act(Method::Put, path).await
    }

    pub async fn workspace_roles(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<WorkspaceRole>, ApiError> {
        let listed = self.listed_workspace_roles(workspace_id).await?;
        Ok(listed.into_iter().map(|listed| listed.role).collect())
    }

    /// The roles of a workspace, each with what the caller may do with it (ADR 0311).
    pub async fn listed_workspace_roles(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<ListedWorkspaceRole>, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/roles")).await
    }

    pub async fn create_workspace_role(
        &self,
        workspace_id: WorkspaceId,
        role: &CreateWorkspaceRole,
    ) -> Result<WorkspaceRole, ApiError> {
        let path = format!("/workspaces/{workspace_id}/roles");
        self.send(Method::Post, path, role).await
    }

    pub async fn update_workspace_role(
        &self,
        workspace_id: WorkspaceId,
        role_id: WorkspaceRoleId,
        change: &UpdateWorkspaceRole,
    ) -> Result<WorkspaceRole, ApiError> {
        let path = format!("/workspaces/{workspace_id}/roles/{role_id}");
        self.send(Method::Patch, path, change).await
    }

    pub async fn assign_workspace_role(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        role_id: WorkspaceRoleId,
    ) -> Result<(), ApiError> {
        let path = format!("/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}");
        self.act(Method::Put, path).await
    }

    pub async fn unassign_workspace_role(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        role_id: WorkspaceRoleId,
    ) -> Result<(), ApiError> {
        let path = format!("/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}");
        self.act(Method::Delete, path).await
    }

    // Categories, channels and channel overrides.

    pub async fn create_category(
        &self,
        workspace_id: WorkspaceId,
        category: &CreateCategory,
    ) -> Result<Category, ApiError> {
        let path = format!("/workspaces/{workspace_id}/categories");
        self.send(Method::Post, path, category).await
    }

    pub async fn update_category(
        &self,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
        change: &UpdateCategory,
    ) -> Result<Category, ApiError> {
        let path = format!("/workspaces/{workspace_id}/categories/{category_id}");
        self.send(Method::Patch, path, change).await
    }

    /// The channels and categories of a workspace that the account can see.
    pub async fn channels(&self, workspace_id: WorkspaceId) -> Result<ChannelList, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/channels"))
            .await
    }

    pub async fn create_channel(
        &self,
        workspace_id: WorkspaceId,
        channel: &CreateChannel,
    ) -> Result<Channel, ApiError> {
        let path = format!("/workspaces/{workspace_id}/channels");
        self.send(Method::Post, path, channel).await
    }

    pub async fn update_channel(
        &self,
        channel_id: ChannelId,
        change: &UpdateChannel,
    ) -> Result<Channel, ApiError> {
        self.send(Method::Patch, format!("/channels/{channel_id}"), change)
            .await
    }

    /// Changes the category and the position of channels of one workspace, all or none.
    /// The server refuses the whole request with `conflict` when a channel is no longer
    /// where `from` states.
    pub async fn move_channels(
        &self,
        workspace_id: WorkspaceId,
        moves: &MoveChannels,
    ) -> Result<Receipt, ApiError> {
        let body = Some(super::client::encode(moves)?);
        let path = format!("/workspaces/{workspace_id}/channels/positions");
        Ok(self
            .call(Method::Patch, path, Vec::new(), body)
            .await?
            .receipt)
    }

    pub async fn delete_channel(&self, channel_id: ChannelId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/channels/{channel_id}"))
            .await
    }

    pub async fn channel_overrides(
        &self,
        channel_id: ChannelId,
    ) -> Result<Vec<ChannelOverride>, ApiError> {
        self.get(format!("/channels/{channel_id}/overrides")).await
    }

    /// Sets the override of one role in one channel. Two empty sets remove it.
    pub async fn set_channel_override(
        &self,
        channel_id: ChannelId,
        role_id: WorkspaceRoleId,
        sets: &SetChannelOverride,
    ) -> Result<ChannelOverride, ApiError> {
        let path = format!("/channels/{channel_id}/overrides/{role_id}");
        self.send(Method::Put, path, sets).await
    }
}
