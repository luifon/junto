//! The requests of the administration about roles (ADR 0301, ADR 0309): a new role, a new
//! name, the permissions of a role, its deletion (ADR 0308), its place in the order, and
//! the roles that an account or a member holds.

use gpui::Context;
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

/// The changes of the permissions of roles that the state does not show yet. A change
/// states the whole list of its role as the state held it, so a second change of the
/// same role that was built on that list would undo the first. A role takes no further
/// change until the server answered and the state shows the change, until the request
/// failed, or until the role is gone.
#[derive(Debug, Default)]
pub struct RoleChanges {
    waiting: Vec<Waiting>,
    /// The number of changes that were begun. A change is named by its number, so the
    /// answer to an earlier request of a role does not settle a later one.
    begun: u64,
}

#[derive(Debug)]
struct Waiting {
    number: u64,
    server_id: ServerId,
    role: RoleRef,
    /// The permission that the change gives or takes.
    permission: RolePermission,
    on: bool,
    /// The list that the state held when the server answered. Absent until then: before
    /// the answer, another list in the state is the change of another person.
    seen_at_answer: Option<Vec<RolePermission>>,
}

/// What the state holds of a role whose change waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    /// The permissions of the role.
    List(Vec<RolePermission>),
    /// The state cannot tell now: the collection of the role is not loaded.
    Unknown,
    /// The state holds no such role, or no such server for the account.
    Gone,
}

impl RoleChanges {
    /// True while a change of the role waits for the state.
    pub fn waits(&self, server_id: ServerId, role: RoleRef) -> bool {
        self.waiting
            .iter()
            .any(|held| held.server_id == server_id && held.role == role)
    }

    /// The roles of one server whose change waits.
    pub fn of(&self, server_id: ServerId) -> Vec<RoleRef> {
        self.waiting
            .iter()
            .filter(|held| held.server_id == server_id)
            .map(|held| held.role)
            .collect()
    }

    /// Notes a change of one permission of a role, and returns its number. Nothing when
    /// a change of the role waits already: the new one is not sent.
    pub(crate) fn begin(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        permission: RolePermission,
        on: bool,
    ) -> Option<u64> {
        if self.waits(server_id, role) {
            return None;
        }
        self.begun += 1;
        self.waiting.push(Waiting {
            number: self.begun,
            server_id,
            role,
            permission,
            on,
            seen_at_answer: None,
        });
        Some(self.begun)
    }

    /// The request of a change failed: the role takes a change again.
    pub(crate) fn failed(&mut self, number: u64) {
        self.waiting.retain(|held| held.number != number);
    }

    /// The server did the request of a change. `held` is what the state holds of the
    /// role now; the role waits until the state shows the change.
    pub(crate) fn answered(&mut self, number: u64, held: &Held) {
        for waiting in &mut self.waiting {
            if waiting.number == number {
                waiting.seen_at_answer = Some(match held {
                    Held::List(list) => list.clone(),
                    Held::Unknown | Held::Gone => Vec::new(),
                });
            }
        }
    }

    /// Frees the roles whose change the state shows. A change stays while the server has
    /// not answered, and while the state can tell nothing of the role. After the answer
    /// the role is free when the state holds the permission as the change set it, and
    /// when the list of the state changed since the answer: the event of the change, or
    /// of a later one, has arrived then. A role that the state does not hold waits for
    /// nothing.
    pub(crate) fn settle(&mut self, held: impl Fn(ServerId, RoleRef) -> Held) {
        self.waiting
            .retain(|waiting| match held(waiting.server_id, waiting.role) {
                Held::Gone => false,
                Held::Unknown => true,
                Held::List(now) => match &waiting.seen_at_answer {
                    None => true,
                    Some(seen) => now.contains(&waiting.permission) != waiting.on && now == *seen,
                },
            });
    }

    /// Another account uses the server, or none: no change of the account before waits.
    pub(crate) fn forget(&mut self) {
        self.waiting.clear();
    }
}

/// The permissions of a role as a state holds them, in the order of the protocol.
fn held_permissions(model: &junto_client_core::model::ServerModel, role: RoleRef) -> Held {
    match role {
        RoleRef::Server(role_id) => match model.admin.server_roles.loaded() {
            // The collection is read while a page needs it. A page that does not need
            // it tells nothing of the role.
            None => Held::Unknown,
            Some(roles) => {
                roles
                    .iter()
                    .find(|held| held.id == role_id)
                    .map_or(Held::Gone, |held| {
                        Held::List(
                            held.permissions
                                .iter()
                                .copied()
                                .map(RolePermission::Server)
                                .collect(),
                        )
                    })
            }
        },
        RoleRef::Workspace(workspace_id, role_id) => model
            .workspace_role(workspace_id, role_id)
            .map_or(Held::Gone, |held| {
                Held::List(
                    held.permissions
                        .iter()
                        .copied()
                        .map(RolePermission::Workspace)
                        .collect(),
                )
            }),
    }
}

impl AppState {
    /// What the state of a server holds of a role now, for the account that it shows.
    fn held_of_role(&self, server_id: ServerId, role: RoleRef) -> Held {
        let snapshots = self.snapshots();
        snapshots
            .iter()
            .find(|server| server.id == server_id)
            .and_then(super::view::shown)
            .map_or(Held::Gone, |model| held_permissions(model, role))
    }

    /// The roles whose change of permissions the state shows now take a change again.
    pub(super) fn settle_role_changes(&mut self) {
        let snapshots = self.snapshots();
        self.admin.role_changes.settle(|server_id, role| {
            snapshots
                .iter()
                .find(|server| server.id == server_id)
                .and_then(super::view::shown)
                .map_or(Held::Gone, |model| held_permissions(model, role))
        });
    }

    /// Sends the new list of a role, and frees the role for a further change when the
    /// request failed. A request that the server did frees the role when the state shows
    /// the change.
    fn send_role_permissions<T, F>(
        &mut self,
        server_id: ServerId,
        role: RoleRef,
        number: u64,
        request: impl FnOnce(junto_client_core::api::ServerApi) -> F + 'static,
        cx: &mut Context<Self>,
    ) where
        T: 'static,
        F: std::future::Future<Output = Result<T, junto_client_core::api::ApiError>> + 'static,
    {
        let (told, outcome) = futures_channel::oneshot::channel();
        self.admin_ask(
            server_id,
            AskKind::UpdateRole(role),
            move |api| {
                let asked = request(api);
                async move {
                    let answer = asked.await;
                    let _ = told.send(answer.is_ok());
                    answer
                }
            },
            |_, _, _| {},
            cx,
        );
        // The page tells the refusal of its newest request only. The role is freed for
        // every failed request, also for one that the page tells nothing about.
        cx.spawn(async move |this, cx| {
            let done = outcome.await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                if done {
                    let held = this.held_of_role(server_id, role);
                    this.admin.role_changes.answered(number, &held);
                    this.settle_role_changes();
                } else {
                    this.admin.role_changes.failed(number);
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

    /// Gives a role one permission or takes it. The request states the whole list of the
    /// role as the state holds it now with this one change. Nothing is sent while an
    /// earlier change of the role waits for the state (ADR 0791).
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
                let next = changed(ServerPermission::ALL, &stored.permissions, permission, on);
                // The role holds the permission as asked already: nothing is sent.
                if next == stored.permissions {
                    return;
                }
                let change = UpdateServerRole {
                    name: None,
                    permissions: Some(next),
                };
                let Some(number) = self.admin.role_changes.begin(server_id, role, asked, on) else {
                    return;
                };
                self.send_role_permissions(
                    server_id,
                    role,
                    number,
                    move |api| async move { api.update_server_role(role_id, &change).await },
                    cx,
                );
            }
            (RoleRef::Workspace(workspace_id, role_id), RolePermission::Workspace(permission)) => {
                let Some(stored) = model.workspace_role(workspace_id, role_id) else {
                    return;
                };
                let next = changed(
                    WorkspacePermission::ALL,
                    &stored.permissions,
                    permission,
                    on,
                );
                if next == stored.permissions {
                    return;
                }
                let change = UpdateWorkspaceRole {
                    name: None,
                    permissions: Some(next),
                };
                let Some(number) = self.admin.role_changes.begin(server_id, role, asked, on) else {
                    return;
                };
                self.send_role_permissions(
                    server_id,
                    role,
                    number,
                    move |api| async move {
                        api.update_workspace_role(workspace_id, role_id, &change)
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

/// The list of a role after one permission was given or taken, in the order of the
/// protocol.
fn changed<P: Copy + PartialEq>(all: &[P], held: &[P], permission: P, on: bool) -> Vec<P> {
    all.iter()
        .copied()
        .filter(|of| {
            if *of == permission {
                on
            } else {
                held.contains(of)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_takes_one_change_of_its_permissions_at_a_time_until_the_state_shows_it() {
        use RolePermission::Workspace as P;
        use WorkspacePermission::{ManageChannel, ManageMessages, ViewChannel};
        let (server_id, other_server) = (ServerId::generate(), ServerId::generate());
        let workspace_id = WorkspaceId::generate();
        let (first, second) = (
            RoleRef::Workspace(workspace_id, WorkspaceRoleId::generate()),
            RoleRef::Workspace(workspace_id, WorkspaceRoleId::generate()),
        );
        // What the state holds of the two roles of the first server.
        let state = |lists: Vec<(RoleRef, Held)>| {
            move |server: ServerId, role: RoleRef| {
                if server != server_id {
                    return Held::Gone;
                }
                lists
                    .iter()
                    .find(|(of, _)| *of == role)
                    .map_or(Held::Gone, |(_, held)| held.clone())
            }
        };
        let list = |permissions: &[WorkspacePermission]| {
            Held::List(permissions.iter().copied().map(P).collect())
        };
        let mut changes = RoleChanges::default();
        // "Manage messages" is given to the first role, which holds "view channel".
        let one = changes
            .begin(server_id, first, P(ManageMessages), true)
            .unwrap();
        // A second change of the same role would be built on the list of before the
        // first one: it is not sent. Another role, and the same role of another server,
        // are free.
        assert_eq!(
            changes.begin(server_id, first, P(ManageChannel), true),
            None
        );
        let two = changes
            .begin(server_id, second, P(ViewChannel), true)
            .unwrap();
        assert!(
            changes
                .begin(other_server, first, P(ViewChannel), true)
                .is_some()
        );
        assert_eq!(changes.of(server_id), [first, second]);

        // Nothing is answered: both wait, also when the state shows another list, which
        // is the change of another person then. The server of the third change is gone.
        let before = vec![(first, list(&[ViewChannel])), (second, list(&[]))];
        changes.settle(state(before.clone()));
        let by_another = vec![
            (first, list(&[ViewChannel, ManageChannel])),
            (second, list(&[])),
        ];
        changes.settle(state(by_another.clone()));
        assert_eq!(changes.of(server_id), [first, second]);
        assert!(!changes.waits(other_server, first));

        // The server answered the first change while the state holds the list of the
        // other person, without the own change: the role still waits.
        changes.answered(one, &by_another[0].1);
        changes.settle(state(by_another));
        assert!(changes.waits(server_id, first));
        // The own event arrives: the state holds the permission as the change set it.
        changes.settle(state(vec![
            (first, list(&[ViewChannel, ManageChannel, ManageMessages])),
            (second, list(&[])),
        ]));
        assert_eq!(changes.of(server_id), [second]);

        // The second change: answered while the state shows the list of before. The role
        // waits for its event, and a state that tells nothing of the role frees nothing.
        changes.answered(two, &list(&[]));
        changes.settle(state(vec![(second, list(&[]))]));
        changes.settle(state(vec![(second, Held::Unknown)]));
        assert_eq!(changes.of(server_id), [second]);
        changes.settle(state(vec![(second, list(&[ViewChannel]))]));
        assert!(changes.of(server_id).is_empty());

        // The answer to an earlier request of a role does not settle a later one.
        let three = changes
            .begin(server_id, first, P(ViewChannel), false)
            .unwrap();
        changes.failed(one);
        changes.answered(two, &list(&[]));
        assert!(changes.waits(server_id, first));
        // A request that failed frees its role at once.
        changes.failed(three);
        assert!(!changes.waits(server_id, first));

        // A change whose event another person undid before the answer: the state does
        // not hold the permission as the change set it. The next change of the list
        // frees the role, so that it does not wait for ever.
        let four = changes
            .begin(server_id, first, P(ManageChannel), true)
            .unwrap();
        changes.answered(four, &list(&[ViewChannel]));
        changes.settle(state(vec![(first, list(&[ViewChannel]))]));
        assert!(changes.waits(server_id, first));
        changes.settle(state(vec![(first, list(&[]))]));
        assert!(!changes.waits(server_id, first));

        // A role that the state does not hold any more waits for nothing, and neither
        // does anything after another account took the place.
        changes
            .begin(server_id, first, P(ViewChannel), true)
            .unwrap();
        changes.settle(state(Vec::new()));
        assert!(!changes.waits(server_id, first));
        changes
            .begin(server_id, second, P(ViewChannel), true)
            .unwrap();
        changes.forget();
        assert!(!changes.waits(server_id, second));
    }

    #[test]
    fn one_permission_changes_and_the_others_stay_in_the_order_of_the_protocol() {
        use WorkspacePermission::{ManageChannel, ManageRoles, ViewChannel};
        let all = WorkspacePermission::ALL;
        assert_eq!(
            changed(all, &[ManageChannel, ViewChannel], ManageRoles, true),
            [ViewChannel, ManageRoles, ManageChannel]
        );
        assert_eq!(
            changed(all, &[ManageChannel, ViewChannel], ViewChannel, false),
            [ManageChannel]
        );
        // A permission that the role holds already, and one that it does not hold.
        assert_eq!(
            changed(all, &[ViewChannel], ViewChannel, true),
            [ViewChannel]
        );
        assert_eq!(
            changed(all, &[ViewChannel], ManageRoles, false),
            [ViewChannel]
        );
    }
}
