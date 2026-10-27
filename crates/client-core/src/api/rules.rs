//! The routes of the permission rules of categories and channels, of the workspace
//! permissions of a member and of the server permissions of an account (ADR 0304,
//! ADR 0306).

use junto_protocol::ids::{AccountId, CategoryId, ChannelId, WorkspaceId};
use junto_protocol::roles::{
    ChangeServerPersonPermission, ServerPersonPermissions, SetServerPersonPermissions,
};
use junto_protocol::structure::{
    CategoryRules, ChangeChannelOverride, ChannelPermissions, ChannelRules, OverrideState,
    PermissionRule, PersonPermissions, RuleSubject, SetChannelOverride,
};
use junto_protocol::{ServerPermission, WorkspacePermission};

use super::error::ApiError;
use super::session::ServerApi;
use super::transport::Method;

/// The path part of a rule of `subject`.
fn subject_path(subject: RuleSubject) -> String {
    match subject {
        RuleSubject::Role { role_id } => format!("roles/{role_id}"),
        RuleSubject::Person { account_id } => format!("people/{account_id}"),
    }
}

fn category_path(workspace_id: WorkspaceId, category_id: CategoryId) -> String {
    format!("/workspaces/{workspace_id}/categories/{category_id}/rules")
}

impl ServerApi {
    /// The rules that a channel uses, and whether it takes them from its category.
    pub async fn channel_rules(&self, channel_id: ChannelId) -> Result<ChannelRules, ApiError> {
        self.get(format!("/channels/{channel_id}/rules")).await
    }

    /// Sets both sets of the rule of `subject` in a channel. Two empty sets remove it.
    pub async fn set_channel_rule(
        &self,
        channel_id: ChannelId,
        subject: RuleSubject,
        sets: &SetChannelOverride,
    ) -> Result<PermissionRule, ApiError> {
        let path = format!("/channels/{channel_id}/rules/{}", subject_path(subject));
        self.send(Method::Put, path, sets).await
    }

    /// Changes one permission of the rule of `subject` in a channel and keeps every other
    /// permission of it. The grant rule holds for this permission only (ADR 0304).
    pub async fn change_channel_rule(
        &self,
        channel_id: ChannelId,
        subject: RuleSubject,
        permission: WorkspacePermission,
        state: OverrideState,
    ) -> Result<PermissionRule, ApiError> {
        let path = format!("/channels/{channel_id}/rules/{}", subject_path(subject));
        let change = ChangeChannelOverride { permission, state };
        self.send(Method::Patch, path, &change).await
    }

    /// Removes the own rules of a channel, which takes the rules of its category again.
    pub async fn sync_channel_rules(
        &self,
        channel_id: ChannelId,
    ) -> Result<ChannelRules, ApiError> {
        let path = format!("/channels/{channel_id}/rules/sync");
        self.send(Method::Post, path, &serde_json::json!({})).await
    }

    pub async fn category_rules(
        &self,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
    ) -> Result<CategoryRules, ApiError> {
        self.get(category_path(workspace_id, category_id)).await
    }

    pub async fn set_category_rule(
        &self,
        (workspace_id, category_id): (WorkspaceId, CategoryId),
        subject: RuleSubject,
        sets: &SetChannelOverride,
    ) -> Result<PermissionRule, ApiError> {
        let path = format!(
            "{}/{}",
            category_path(workspace_id, category_id),
            subject_path(subject)
        );
        self.send(Method::Put, path, sets).await
    }

    pub async fn change_category_rule(
        &self,
        (workspace_id, category_id): (WorkspaceId, CategoryId),
        subject: RuleSubject,
        permission: WorkspacePermission,
        state: OverrideState,
    ) -> Result<PermissionRule, ApiError> {
        let path = format!(
            "{}/{}",
            category_path(workspace_id, category_id),
            subject_path(subject)
        );
        let change = ChangeChannelOverride { permission, state };
        self.send(Method::Patch, path, &change).await
    }

    /// The workspace permissions that are allowed to or denied to one member.
    pub async fn member_permissions(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
    ) -> Result<PersonPermissions, ApiError> {
        self.get(format!(
            "/workspaces/{workspace_id}/members/{account_id}/permissions"
        ))
        .await
    }

    pub async fn set_member_permissions(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        sets: &SetChannelOverride,
    ) -> Result<PersonPermissions, ApiError> {
        let path = format!("/workspaces/{workspace_id}/members/{account_id}/permissions");
        self.send(Method::Put, path, sets).await
    }

    pub async fn change_member_permission(
        &self,
        (workspace_id, account_id): (WorkspaceId, AccountId),
        permission: WorkspacePermission,
        state: OverrideState,
    ) -> Result<PersonPermissions, ApiError> {
        let path = format!("/workspaces/{workspace_id}/members/{account_id}/permissions");
        let change = ChangeChannelOverride { permission, state };
        self.send(Method::Patch, path, &change).await
    }

    /// What a member can use in each channel of the workspace that the account sees.
    pub async fn member_access(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
    ) -> Result<Vec<ChannelPermissions>, ApiError> {
        self.get(format!(
            "/workspaces/{workspace_id}/members/{account_id}/access"
        ))
        .await
    }

    /// The server permissions that are allowed to or denied to one account.
    pub async fn server_permissions(
        &self,
        account_id: AccountId,
    ) -> Result<ServerPersonPermissions, ApiError> {
        self.get(format!("/accounts/{account_id}/server-permissions"))
            .await
    }

    pub async fn set_server_permissions(
        &self,
        account_id: AccountId,
        sets: &SetServerPersonPermissions,
    ) -> Result<ServerPersonPermissions, ApiError> {
        let path = format!("/accounts/{account_id}/server-permissions");
        self.send(Method::Put, path, sets).await
    }

    pub async fn change_server_permission(
        &self,
        account_id: AccountId,
        permission: ServerPermission,
        state: OverrideState,
    ) -> Result<ServerPersonPermissions, ApiError> {
        let path = format!("/accounts/{account_id}/server-permissions");
        let change = ChangeServerPersonPermission { permission, state };
        self.send(Method::Patch, path, &change).await
    }
}
