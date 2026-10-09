//! Every operation that needs a session, written once. An authority adds the session
//! token to each request: a fixed token, or the session that a saved server keeps and
//! renews.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use junto_protocol::account::{
    AccountSummary, ApproveDeviceLink, Device, LookUpDeviceLink, OwnAccount, PendingDeviceLink,
    RecoverAccount, RecoveryNotice, RenameDevice,
};
use junto_protocol::audit::{AuditPage, AuditQuery};
use junto_protocol::ids::{
    AccountId, CategoryId, ChannelId, DeviceId, InviteId, JoinRequestId, ServerRoleId, WorkspaceId,
    WorkspaceRoleId,
};
use junto_protocol::invite::{
    ApproveJoinRequest, CreateInvite, Invite, InviteCreated, JoinRequest, JoinRequestStatus,
    RedeemInvite, Redemption,
};
use junto_protocol::keys::PublicKey;
use junto_protocol::roles::{
    CreateServerRole, CreateWorkspaceRole, ServerRole, UpdateServerRole, UpdateWorkspaceRole,
    WorkspaceRole,
};
use junto_protocol::secret::{InviteCode, LinkCode};
use junto_protocol::structure::{
    Category, Channel, ChannelList, ChannelOverride, CreateCategory, CreateChannel,
    CreateWorkspace, SetChannelOverride, UpdateCategory, UpdateChannel, UpdateWorkspace, Workspace,
    WorkspaceMember,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::client::{ApiClient, encode, read_json};
use super::error::ApiError;
use super::retry::Repeat;
use super::transport::{HttpResponse, Method};
use crate::secret::{SecretBytes, SecretToken};

/// A request of a session without its token and without the version prefix.
pub(crate) struct Outline {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) query: Vec<(&'static str, String)>,
    pub(crate) body: Option<Arc<SecretBytes>>,
}

pub(crate) type AuthorityFuture =
    Pin<Box<dyn Future<Output = Result<HttpResponse, ApiError>> + Send + 'static>>;

pub(crate) trait Authority: Send + Sync + 'static {
    fn send(&self, outline: Outline) -> AuthorityFuture;
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
}

pub(crate) async fn send_with_token(
    api: &ApiClient,
    token: &SecretToken,
    outline: Outline,
) -> Result<HttpResponse, ApiError> {
    api.send(
        outline.method,
        &outline.path,
        &outline.query,
        Some(token),
        outline.body,
        Repeat::WhenUnchanged,
    )
    .await
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

/// The operations of a logged-in account on one server.
#[derive(Clone)]
pub struct ServerApi {
    authority: Arc<dyn Authority>,
}

impl ServerApi {
    pub(crate) fn new(authority: Arc<dyn Authority>) -> Self {
        Self { authority }
    }

    async fn call(
        &self,
        method: Method,
        path: String,
        query: Vec<(&'static str, String)>,
        body: Option<Arc<SecretBytes>>,
    ) -> Result<HttpResponse, ApiError> {
        self.authority
            .send(Outline {
                method,
                path,
                query,
                body,
            })
            .await
    }

    async fn get<R: DeserializeOwned>(&self, path: String) -> Result<R, ApiError> {
        read_json(&self.call(Method::Get, path, Vec::new(), None).await?)
    }

    async fn send<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        body: &B,
    ) -> Result<R, ApiError> {
        let body = Some(encode(body)?);
        read_json(&self.call(method, path, Vec::new(), body).await?)
    }

    async fn act(&self, method: Method, path: String) -> Result<(), ApiError> {
        self.call(method, path, Vec::new(), None).await.map(|_| ())
    }

    // The account of the caller and its devices.

    pub async fn own_account(&self) -> Result<OwnAccount, ApiError> {
        self.get("/account".to_owned()).await
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

    pub async fn recoveries(&self) -> Result<Vec<RecoveryNotice>, ApiError> {
        self.get("/account/recoveries".to_owned()).await
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
        self.get("/accounts".to_owned()).await
    }

    pub async fn remove_account(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.act(Method::Post, format!("/accounts/{account_id}/removal"))
            .await
    }

    pub async fn ban_account(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.act(Method::Put, format!("/accounts/{account_id}/ban"))
            .await
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

    // The audit log.

    pub async fn server_audit_log(&self, query: &AuditQuery) -> Result<AuditPage, ApiError> {
        let response = self
            .call(Method::Get, "/audit-log".to_owned(), page(query), None)
            .await?;
        read_json(&response)
    }

    pub async fn workspace_audit_log(
        &self,
        workspace_id: WorkspaceId,
        query: &AuditQuery,
    ) -> Result<AuditPage, ApiError> {
        let path = format!("/workspaces/{workspace_id}/audit-log");
        read_json(&self.call(Method::Get, path, page(query), None).await?)
    }
}

fn page(query: &AuditQuery) -> Vec<(&'static str, String)> {
    let mut pairs = Vec::new();
    if let Some(before) = query.before {
        pairs.push(("before", before.to_string()));
    }
    if let Some(limit) = query.limit {
        pairs.push(("limit", limit.to_string()));
    }
    pairs
}
