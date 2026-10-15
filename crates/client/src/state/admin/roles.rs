//! The requests of the administration about roles (ADR 0301, ADR 0309): a new role, a new
//! name, the permissions of a role, its deletion (ADR 0308), its place in the order, and
//! the roles that an account or a member holds.

use gpui::Context;
use junto_client_core::api::{Changed, Receipt};
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::ids::{
    AccountId, ServerId, ServerRoleId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::roles::{
    CreateServerRole, CreateWorkspaceRole, ServerRolePosition, UpdateServerRole,
    UpdateWorkspaceRole, WorkspaceRolePosition,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::AskKind;
use crate::state::AppState;
use crate::state::receipts::Awaited;

/// The two levels of roles (ADR 0301).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleLevel {
    Server,
    Workspace(WorkspaceId),
}

/// One role of a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleRef {
    Server(ServerRoleId),
    Workspace(WorkspaceId, WorkspaceRoleId),
}

impl RoleRef {
    pub const fn level(&self) -> RoleLevel {
        match self {
            Self::Server(_) => RoleLevel::Server,
            Self::Workspace(workspace_id, _) => RoleLevel::Workspace(*workspace_id),
        }
    }

    /// A number that tells the role from every other role, for the identifier of its
    /// controls.
    pub fn number(&self) -> u64 {
        match self {
            Self::Server(role_id) => role_id.as_uuid().as_u128() as u64,
            Self::Workspace(_, role_id) => role_id.as_uuid().as_u128() as u64,
        }
    }

    /// The identifier of the role in its written form, for the name of a control in a
    /// test.
    pub fn written(&self) -> String {
        match self {
            Self::Server(role_id) => role_id.to_string(),
            Self::Workspace(_, role_id) => role_id.to_string(),
        }
    }
}

/// One permission that a role of one of the two levels can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RolePermission {
    Server(ServerPermission),
    Workspace(WorkspacePermission),
}

impl RolePermission {
    /// The name of the permission in the protocol.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Server(permission) => permission.name(),
            Self::Workspace(permission) => permission.name(),
        }
    }
}

/// The changes of single permissions of roles that the state does not show yet
/// (ADR 0311). A change names one permission, so changes of different permissions of one
/// role go at the same time and none undoes another. A permission takes no further change
/// while its change is on its way, and its switch shows the asked state until the state
/// holds the events of the change (ADR 0214).
#[derive(Debug, Default)]
pub struct RoleChanges {
    waiting: Vec<Waiting>,
    /// The number of changes that were begun. A change is named by its number, so the
    /// answer to an earlier request does not settle a later one.
    begun: u64,
}

#[derive(Debug)]
struct Waiting {
    number: u64,
    server_id: ServerId,
    role: RoleRef,
    permission: RolePermission,
    on: bool,
    /// The receipt of the answer. Absent while the request is on its way.
    answered: Option<Awaited>,
}

impl RoleChanges {
    /// The state that a change asked for, while it waits for the state.
    pub fn asked(
        &self,
        server_id: ServerId,
        role: RoleRef,
        permission: RolePermission,
    ) -> Option<bool> {
        self.waiting
            .iter()
            .find(|held| {
                held.server_id == server_id && held.role == role && held.permission == permission
            })
            .map(|held| held.on)
    }

    /// The permissions of the roles of one server whose change waits, with the asked
    /// state.
    pub fn of(&self, server_id: ServerId) -> Vec<(RoleRef, RolePermission, bool)> {
        self.waiting
            .iter()
            .filter(|held| held.server_id == server_id)
            .map(|held| (held.role, held.permission, held.on))
            .collect()
    }

    /// Notes a change of one permission of a role, and returns its number. Nothing when
    /// a change of the same permission waits already.
    pub(crate) fn begin(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        permission: RolePermission,
        on: bool,
    ) -> Option<u64> {
        if self.asked(server_id, role, permission).is_some() {
            return None;
        }
        self.begun += 1;
        self.waiting.push(Waiting {
            number: self.begun,
            server_id,
            role,
            permission,
            on,
            answered: None,
        });
        Some(self.begun)
    }

    /// The request of a change failed: the permission takes a change again.
    pub(crate) fn failed(&mut self, number: u64) {
        self.waiting.retain(|held| held.number != number);
    }

    /// The server did the request of a change, with this receipt.
    pub(crate) fn answered(&mut self, number: u64, receipt: Receipt) {
        for waiting in self.waiting.iter_mut().filter(|held| held.number == number) {
            waiting.answered = Some(Awaited::new(receipt));
        }
    }

    /// True while the change with this number waits for the state.
    pub(crate) fn waits_for(&self, number: u64) -> bool {
        self.waiting.iter().any(|held| held.number == number)
    }

    /// A snapshot that was read after the answer replaced the state.
    pub(crate) fn refreshed(&mut self, number: u64) {
        for waiting in self.waiting.iter_mut().filter(|held| held.number == number) {
            if let Some(answered) = &mut waiting.answered {
                answered.refreshed = true;
            }
        }
    }

    /// Frees the permissions whose change the state holds. `state` gives the state of a
    /// server for the account that asked, when it holds the role. A change waits while the
    /// server has not answered; a role or a server that the state does not hold waits for
    /// nothing.
    pub(crate) fn settle<'a>(
        &mut self,
        state: impl Fn(ServerId, RoleRef) -> Option<&'a ServerModel>,
    ) {
        self.waiting
            .retain(|waiting| match state(waiting.server_id, waiting.role) {
                None => false,
                Some(model) => waiting
                    .answered
                    .is_none_or(|answered| !answered.shown_by(model)),
            });
    }

    /// Another account uses the server, or none: no change of the account before waits.
    pub(crate) fn forget(&mut self) {
        self.waiting.clear();
    }
}

/// True when the state holds the role.
fn holds_role(model: &ServerModel, role: RoleRef) -> bool {
    match role {
        // A page that does not read the server roles tells nothing of a role: its
        // change waits for the receipt alone.
        RoleRef::Server(role_id) => model
            .admin
            .server_roles
            .loaded()
            .is_none_or(|roles| roles.iter().any(|held| held.id == role_id)),
        RoleRef::Workspace(workspace_id, role_id) => {
            model.workspace_role(workspace_id, role_id).is_some()
        }
    }
}

impl AppState {
    /// The permissions whose change the state shows now take a change again.
    pub(super) fn settle_role_changes(&mut self) {
        let snapshots = self.snapshots();
        self.admin.role_changes.settle(|server_id, role| {
            snapshots
                .iter()
                .find(|server| server.id == server_id)
                .and_then(super::view::shown)
                .filter(|model| holds_role(model, role))
        });
    }

    /// Sends the change of one permission of a role. A request that failed frees the
    /// permission at once; one that the server did frees it when the state holds it.
    fn send_role_permission<T, F>(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        number: u64,
        request: impl FnOnce(junto_client_core::api::ServerApi) -> F + 'static,
        cx: &mut Context<Self>,
    ) where
        T: 'static,
        F: std::future::Future<Output = Result<Changed<T>, junto_client_core::api::ApiError>>
            + 'static,
    {
        let (told, outcome) = futures_channel::oneshot::channel();
        self.admin_ask(
            server_id,
            AskKind::UpdateRole(role),
            move |api| {
                let asked = request(api);
                async move {
                    let answer = asked.await;
                    let _ = told.send(answer.as_ref().ok().map(|changed| changed.receipt));
                    answer
                }
            },
            |_, _, _| {},
            cx,
        );
        // The page tells the refusal of its newest request only. The permission is freed
        // for every failed request, also for one that the page tells nothing about.
        cx.spawn(async move |this, cx| {
            let receipt = outcome.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                match receipt {
                    Some(receipt) => {
                        this.admin.role_changes.answered(number, receipt);
                        let model = this
                            .client
                            .server(server_id)
                            .and_then(|server| server.live().view().model);
                        if let Some(model) = model {
                            this.refresh_for(
                                server_id,
                                &model,
                                Awaited::new(receipt),
                                move |this| this.admin.role_changes.waits_for(number),
                                move |this| this.admin.role_changes.refreshed(number),
                                cx,
                            );
                        }
                        this.settle_role_changes();
                    }
                    None => this.admin.role_changes.failed(number),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Creates a role without a permission. The server puts it at the bottom of the
    /// order, above the "everyone" role (ADR 0309).
    pub fn create_role(
        &mut self,
        server_id: ServerId,
        level: RoleLevel,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let what = AskKind::CreateRole(level);
        match level {
            RoleLevel::Server => {
                let role = CreateServerRole {
                    name,
                    permissions: Vec::new(),
                };
                self.admin_ask(
                    server_id,
                    what,
                    |api| async move { api.create_server_role(&role).await },
                    |_, _, _| {},
                    cx,
                );
            }
            RoleLevel::Workspace(workspace_id) => {
                let role = CreateWorkspaceRole {
                    name,
                    permissions: Vec::new(),
                };
                self.admin_ask(
                    server_id,
                    what,
                    |api| async move { api.create_workspace_role(workspace_id, &role).await },
                    |_, _, _| {},
                    cx,
                );
            }
        }
    }

    pub fn rename_role(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let what = AskKind::UpdateRole(role);
        match role {
            RoleRef::Server(role_id) => {
                let change = UpdateServerRole {
                    name: Some(name),
                    permissions: None,
                };
                self.admin_ask(
                    server_id,
                    what,
                    |api| async move { api.update_server_role(role_id, &change).await },
                    |_, _, _| {},
                    cx,
                );
            }
            RoleRef::Workspace(workspace_id, role_id) => {
                let change = UpdateWorkspaceRole {
                    name: Some(name),
                    permissions: None,
                };
                self.admin_ask(
                    server_id,
                    what,
                    |api| async move {
                        api.update_workspace_role(workspace_id, role_id, &change)
                            .await
                    },
                    |_, _, _| {},
                    cx,
                );
            }
        }
    }

    /// Gives a role one permission or takes it, with one request that names that
    /// permission (ADR 0311). Nothing is sent while a change of the same permission waits
    /// for the state, or when the role holds the permission as asked.
    pub fn set_role_permission(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        permission: RolePermission,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let asked = permission;
        match (role, permission) {
            (RoleRef::Server(role_id), RolePermission::Server(permission)) => {
                let Some(stored) = model.server_role(role_id) else {
                    return;
                };
                if stored.permissions.contains(&permission) == on {
                    return;
                }
                let Some(number) = self.admin.role_changes.begin(server_id, role, asked, on) else {
                    return;
                };
                self.send_role_permission(
                    server_id,
                    role,
                    number,
                    move |api| async move {
                        api.change_server_role_permission(role_id, permission, on)
                            .await
                    },
                    cx,
                );
            }
            (RoleRef::Workspace(workspace_id, role_id), RolePermission::Workspace(permission)) => {
                let Some(stored) = model.workspace_role(workspace_id, role_id) else {
                    return;
                };
                if stored.permissions.contains(&permission) == on {
                    return;
                }
                let Some(number) = self.admin.role_changes.begin(server_id, role, asked, on) else {
                    return;
                };
                self.send_role_permission(
                    server_id,
                    role,
                    number,
                    move |api| async move {
                        api.change_workspace_role_permission(workspace_id, role_id, permission, on)
                            .await
                    },
                    cx,
                );
            }
            // A permission of the other level is no permission of this role.
            _ => {}
        }
    }

    /// Deletes a role with its assignments, and a workspace role with its overrides
    /// (ADR 0308).
    pub fn delete_role(&mut self, server_id: ServerId, role: RoleRef, cx: &mut Context<Self>) {
        let what = AskKind::DeleteRole(role);
        match role {
            RoleRef::Server(role_id) => self.admin_ask(
                server_id,
                what,
                |api| async move { api.delete_server_role(role_id).await },
                |_, (), _| {},
                cx,
            ),
            RoleRef::Workspace(workspace_id, role_id) => self.admin_ask(
                server_id,
                what,
                |api| async move { api.delete_workspace_role(workspace_id, role_id).await },
                |_, (), _| {},
                cx,
            ),
        }
    }

    /// Moves a role to the place `to` among the roles of its level, counted from the
    /// highest role. One request gives every role between the two places its new position
    /// (ADR 0309). Nothing is sent for a place that the page does not offer.
    pub fn move_role(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.roles_page(server_id, role.level()) else {
            return;
        };
        let changes = page.moved(role, to);
        if changes.is_empty() {
            return;
        }
        let what = AskKind::MoveRole(role);
        match role.level() {
            RoleLevel::Server => {
                let positions: Vec<ServerRolePosition> = changes
                    .into_iter()
                    .filter_map(|(role, position)| match role {
                        RoleRef::Server(role_id) => Some(ServerRolePosition { role_id, position }),
                        RoleRef::Workspace(..) => None,
                    })
                    .collect();
                self.admin_ask(
                    server_id,
                    what,
                    |api| async move { api.reorder_server_roles(positions).await },
                    |_, (), _| {},
                    cx,
                );
            }
            RoleLevel::Workspace(workspace_id) => {
                let positions: Vec<WorkspaceRolePosition> = changes
                    .into_iter()
                    .filter_map(|(role, position)| match role {
                        RoleRef::Workspace(_, role_id) => {
                            Some(WorkspaceRolePosition { role_id, position })
                        }
                        RoleRef::Server(_) => None,
                    })
                    .collect();
                self.admin_ask(
                    server_id,
                    what,
                    |api| async move { api.reorder_workspace_roles(workspace_id, positions).await },
                    |_, (), _| {},
                    cx,
                );
            }
        }
    }

    /// Gives a role to an account, or takes it.
    pub fn assign_role(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        account_id: AccountId,
        give: bool,
        cx: &mut Context<Self>,
    ) {
        let what = AskKind::AssignRole(role, account_id);
        match role {
            RoleRef::Server(role_id) => self.admin_ask(
                server_id,
                what,
                move |api| async move {
                    if give {
                        api.assign_server_role(account_id, role_id).await
                    } else {
                        api.unassign_server_role(account_id, role_id).await
                    }
                },
                |_, (), _| {},
                cx,
            ),
            RoleRef::Workspace(workspace_id, role_id) => self.admin_ask(
                server_id,
                what,
                move |api| async move {
                    if give {
                        api.assign_workspace_role(workspace_id, account_id, role_id)
                            .await
                    } else {
                        api.unassign_workspace_role(workspace_id, account_id, role_id)
                            .await
                    }
                },
                |_, (), _| {},
                cx,
            ),
        }
    }

    /// The roles of one level as the page of the roles shows them now.
    fn roles_page(
        &self,
        server_id: ServerId,
        level: RoleLevel,
    ) -> Option<super::view::roles::RolesPage> {
        let model = self.client.server(server_id)?.live().view().model?;
        super::view::roles::derive(&model, level)
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::api::Receipt;
    use junto_client_core::model::ServerModel;
    use junto_client_core::protocol::events::Cursor;

    use super::*;

    #[test]
    fn a_permission_takes_one_change_at_a_time_until_the_state_holds_its_event() {
        use RolePermission::Workspace as P;
        use WorkspacePermission::{ManageChannel, ManageMessages};
        let model = crate::state::fixtures::model_at(40);
        let server_id = ServerId::generate();
        let workspace_id = WorkspaceId::generate();
        let (first, second) = (
            RoleRef::Workspace(workspace_id, WorkspaceRoleId::generate()),
            RoleRef::Workspace(workspace_id, WorkspaceRoleId::generate()),
        );
        let at = |position| {
            Receipt::At(Cursor {
                epoch: model.cursor.epoch,
                position,
            })
        };
        fn state<'a>(
            server_id: ServerId,
            model: &'a ServerModel,
            held: Vec<RoleRef>,
        ) -> impl Fn(ServerId, RoleRef) -> Option<&'a ServerModel> {
            move |of: ServerId, role: RoleRef| {
                (of == server_id && held.contains(&role)).then_some(model)
            }
        }
        let mut changes = RoleChanges::default();
        // Two permissions of one role and a permission of another role go at the same
        // time; a second change of a waiting permission is not sent.
        let one = changes
            .begin(server_id, first, P(ManageMessages), true)
            .unwrap();
        let two = changes
            .begin(server_id, first, P(ManageChannel), false)
            .unwrap();
        let three = changes
            .begin(server_id, second, P(ManageChannel), true)
            .unwrap();
        assert_eq!(
            changes.begin(server_id, first, P(ManageMessages), false),
            None
        );
        assert_eq!(
            changes.asked(server_id, first, P(ManageMessages)),
            Some(true)
        );
        assert_eq!(
            changes.asked(server_id, first, P(ManageChannel)),
            Some(false)
        );

        // Nothing is answered: every change waits.
        changes.settle(state(server_id, &model, vec![first, second]));
        assert_eq!(changes.of(server_id).len(), 3);

        // An answer at a position that the state has not reached waits; one at or before
        // the position of the state is held by the state.
        changes.answered(one, at(41));
        changes.answered(two, at(40));
        changes.settle(state(server_id, &model, vec![first, second]));
        assert_eq!(
            changes.asked(server_id, first, P(ManageMessages)),
            Some(true)
        );
        assert_eq!(changes.asked(server_id, first, P(ManageChannel)), None);
        let later = crate::state::fixtures::model_at(41);
        assert_eq!(later.cursor.epoch, model.cursor.epoch);
        changes.settle(state(server_id, &later, vec![first, second]));
        assert_eq!(changes.asked(server_id, first, P(ManageMessages)), None);

        // A change without an event is held at once. An answer that states no position
        // waits for a snapshot read after it.
        changes.answered(three, Receipt::NotStated);
        changes.settle(state(server_id, &later, vec![first, second]));
        assert!(changes.asked(server_id, second, P(ManageChannel)).is_some());
        changes.refreshed(three);
        changes.settle(state(server_id, &later, vec![first, second]));
        assert!(changes.of(server_id).is_empty());
        let four = changes
            .begin(server_id, second, P(ManageChannel), true)
            .unwrap();
        changes.answered(four, Receipt::Nothing);
        changes.settle(state(server_id, &later, vec![first, second]));
        assert!(changes.of(server_id).is_empty());

        // A request that failed frees its permission; a role that the state does not
        // hold, and another account, end every wait.
        let five = changes
            .begin(server_id, first, P(ManageChannel), true)
            .unwrap();
        changes.failed(five);
        assert!(changes.of(server_id).is_empty());
        changes
            .begin(server_id, first, P(ManageChannel), true)
            .unwrap();
        changes.settle(state(server_id, &later, vec![second]));
        assert!(changes.of(server_id).is_empty());
        changes
            .begin(server_id, second, P(ManageChannel), true)
            .unwrap();
        changes.forget();
        assert!(changes.of(server_id).is_empty());
    }
}
