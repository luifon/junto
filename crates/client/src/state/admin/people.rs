//! The requests of the administration about the people of a server: removal, ban and
//! unban of an account (ADR 0113), a new member of a workspace (ADR 0115), the removal of
//! a member from one workspace (ADR 0106), invites and join requests (ADR 0111).

use gpui::Context;
use junto_client_core::api::Statement;
use junto_client_core::protocol::account::RecoverAccount;
use junto_client_core::protocol::ids::{AccountId, InviteId, JoinRequestId, ServerId, WorkspaceId};
use junto_client_core::protocol::invite::{ApproveJoinRequest, CreateInvite};

use super::{AskKind, Made};
use crate::state::AppState;

impl AppState {
    /// Removes an account from the server. Its devices, sessions, memberships and roles
    /// end. The reason stays in the audit entry of the removal (ADR 0106); the message is
    /// the notice that the person reads (ADR 0120).
    pub fn remove_account(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        statement: Statement,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RemoveAccount(account_id),
            |api| async move { api.remove_account_stating(account_id, &statement).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Bans an account. Its keys cannot log in or join again, and its invites are
    /// cancelled. The reason and the message are stated as for a removal.
    pub fn ban_account(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        statement: Statement,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::BanAccount(account_id),
            |api| async move { api.ban_account_stating(account_id, &statement).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Removes a member from one workspace. Its roles of that workspace go with the
    /// membership; its account, its other workspaces and its server roles stay
    /// (ADR 0106).
    pub fn remove_workspace_member(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        statement: Statement,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RemoveMember(account_id),
            |api| async move {
                api.remove_workspace_member(workspace_id, account_id, &statement)
                    .await
            },
            |_, (), _| {},
            cx,
        );
    }

    /// Gives a new device key access to an account (ADR 0104, ADR 0112). Only the server
    /// owner can do it. The server ends every session of the account and tells each of its
    /// devices; the answer states when the recovery was done, which the row of the account
    /// shows until the person puts it away.
    pub fn recover_account(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        recovery: RecoverAccount,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RecoverAccount(account_id),
            |api| async move { api.recover_account(account_id, &recovery).await },
            move |this, notice, _| {
                this.admin.made = Some(Made::Recovered { account_id, notice });
            },
            cx,
        );
    }

    /// Lifts the ban of an account. The account has its memberships and roles again.
    pub fn unban_account(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::UnbanAccount(account_id),
            |api| async move { api.unban_account(account_id).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Adds an active account of the server to a workspace (ADR 0115).
    pub fn add_workspace_member(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::AddMember(account_id),
            |api| async move { api.add_workspace_member(workspace_id, account_id).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Creates an invite. The answer is the only one that carries its code, so the code
    /// is kept until the person puts it away (ADR 0111).
    pub fn create_invite(
        &mut self,
        server_id: ServerId,
        invite: CreateInvite,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::CreateInvite,
            |api| async move { api.create_invite(&invite).await },
            |this, created, _| {
                this.admin.made = Some(Made::Invite {
                    invite_id: created.invite.id,
                    code: created.code,
                });
            },
            cx,
        );
    }

    /// Revokes an invite. Its code works no longer.
    pub fn revoke_invite(
        &mut self,
        server_id: ServerId,
        invite_id: InviteId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RevokeInvite(invite_id),
            |api| async move { api.revoke_invite(invite_id).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Approves a join request: the server creates the account in the workspaces. No
    /// workspace: the default workspace.
    pub fn approve_join_request(
        &mut self,
        server_id: ServerId,
        join_request_id: JoinRequestId,
        workspace_ids: Vec<WorkspaceId>,
        cx: &mut Context<Self>,
    ) {
        let approval = ApproveJoinRequest { workspace_ids };
        self.admin_ask(
            server_id,
            AskKind::ApproveRequest(join_request_id),
            |api| async move { api.approve_join_request(join_request_id, &approval).await },
            |_, _, _| {},
            cx,
        );
    }

    pub fn deny_join_request(
        &mut self,
        server_id: ServerId,
        join_request_id: JoinRequestId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::DenyRequest(join_request_id),
            |api| async move { api.deny_join_request(join_request_id).await },
            |_, _, _| {},
            cx,
        );
    }
}
