//! The pages of the roles (ADR 0301, ADR 0309): the roles of the server or of the shown
//! workspace in their order, where the person stands among them, and what the person can
//! do with each one. Role order is decided by the function that the server uses
//! (`junto-authz`); every refusal that it would give is a state of a row before the
//! person tries. The server decides every request.

use junto_client_core::model::{
    AdminPart, CollectionState, RoleRank, ServerModel, WorkspaceModel, outranks,
};
use junto_client_core::protocol::ids::{ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::roles::RoleActions;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::Listed;
use crate::state::admin::roles::{RoleLevel, RolePermission, RoleRef};

/// The most roles of one level: the roles of a workspace with its "everyone" role, and
/// the roles of a server (ADR 0306, ADR 0309).
pub const MOST_ROLES: usize = 250;
/// The highest position that a reorder gives a role (ADR 0309).
pub const MOST_POSITION: i32 = 1_000_000;
/// The most roles that one account holds at one level, without the "everyone" role
/// (ADR 0306).
pub const MOST_HELD_ROLES: usize = 64;

/// Where the person stands among the roles of a level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// The server owner stands above every role.
    Owner,
    /// An administrator in a workspace: above every role of the workspace.
    Administrator,
    /// The highest role of the person at the level. The roles below it can be changed.
    Role(String),
    /// The person holds no role at the level, so no role is below the person.
    NoRole,
}

/// Why the person cannot change, move, delete, give or take a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locked {
    /// The role is the highest role of the person.
    OwnHighest,
    /// The role stands above the highest role of the person, or at its place.
    Above,
}

/// Why a role cannot be moved one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixed {
    Locked(Locked),
    /// Every member holds the "everyone" role, and it stays the lowest one.
    Everyone,
    /// The role is the first one, or the role above it is none that the person can move.
    NoPlaceAbove,
    /// The role is the last one above the "everyone" role.
    NoPlaceBelow,
    /// The positions of the roles that the move names are not in order: a reorder of
    /// another person is arriving, one role at a time.
    Unsettled,
    /// A role that the move names has a position outside the range that a reorder
    /// gives (ADR 0309). The server takes no exchange of such a position.
    OutOfRange,
}

/// What the server states that the person may do with a role: give it, take it, delete
/// it (ADR 0311). The statement takes the overrides of the channels into account, which
/// the client does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stated {
    Known(RoleActions),
    /// The statement is on its way: the list of the roles is read, or read again after a
    /// change of the order or of an override.
    Reading,
    /// The read failed: the server decides each request.
    Unknown,
}

impl Stated {
    /// True when the statement allows the action, or when no statement can be had.
    fn allows(&self, action: impl Fn(&RoleActions) -> bool) -> bool {
        match self {
            Self::Known(actions) => action(actions),
            Self::Reading => false,
            Self::Unknown => true,
        }
    }
}

/// What the server states of one role, from the collection that holds the statements.
/// A collection that no reader has open, and one whose read failed, state nothing: the
/// client decides by role order and the server decides the request.
fn stated(state: CollectionState, actions: Option<&RoleActions>) -> Stated {
    match (actions, state) {
        (Some(actions), _) => Stated::Known(actions.clone()),
        (None, CollectionState::Failed(_) | CollectionState::NotLoaded) => Stated::Unknown,
        // The first read is on its way, or the list was read before an event that added
        // the role or changed the order: the read after that event is on its way.
        (None, CollectionState::Loading | CollectionState::Loaded) => Stated::Reading,
    }
}

/// What the server states of each role of a workspace.
fn stated_of_workspace(
    model: &ServerModel,
    workspace_id: WorkspaceId,
) -> impl Fn(WorkspaceRoleId) -> Stated + '_ {
    let state = model.admin.state(AdminPart::RoleActions(workspace_id));
    move |role_id| {
        stated(
            state,
            model.admin.workspace_role_actions(workspace_id, role_id),
        )
    }
}

/// One permission of a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionSwitch {
    pub permission: RolePermission,
    pub on: bool,
    /// The person does not hold the permission and so cannot give it to the role: the
    /// grant rule of ADR 0304. Taking it is always possible.
    pub beyond_own: bool,
}

/// One role in the order of its level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRow {
    pub role: RoleRef,
    pub name: String,
    pub position: i32,
    /// The role that every member of the workspace holds.
    pub is_everyone: bool,
    /// The person holds the role.
    pub held: bool,
    /// Why the person cannot act on the role. Absent: the role is below the person.
    pub locked: Option<Locked>,
    /// The place that "move up" gives the role, counted from the highest role.
    pub up: Result<usize, Fixed>,
    pub down: Result<usize, Fixed>,
    /// The name can be changed, and the role can be deleted, given and taken.
    pub named: bool,
    /// In the order of the protocol.
    pub permissions: Vec<PermissionSwitch>,
    /// What the server states that the person may do with the role.
    pub stated: Stated,
}

/// Why a role cannot be deleted, beside role order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoDeletion {
    Locked(Locked),
    /// The server states that the person may not delete the role: an override of the
    /// role in a channel gives a permission that the person does not hold (ADR 0311).
    NotAllowed,
    /// The statement of the server is on its way.
    Reading,
}

impl RoleRow {
    /// Whether the person can delete the role, and why not. A role without a name cannot
    /// be deleted at all.
    pub fn deletion(&self) -> Result<(), NoDeletion> {
        if let Some(locked) = self.locked {
            return Err(NoDeletion::Locked(locked));
        }
        match &self.stated {
            Stated::Reading => Err(NoDeletion::Reading),
            stated if !stated.allows(|actions| actions.may_delete) => Err(NoDeletion::NotAllowed),
            _ => Ok(()),
        }
    }

    /// The number of permissions that the role holds.
    pub fn granted(&self) -> usize {
        self.permissions.iter().filter(|held| held.on).count()
    }
}

/// Why a new role cannot be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCreation {
    /// The person holds no role: a new role would not be below the person.
    NoRole,
    /// The level has the most roles that the server allows.
    Full,
    /// Where the person stands is not read yet.
    NotRead,
}

/// The roles of one level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolesPage {
    pub level: RoleLevel,
    /// The name of the workspace, for the roles of a workspace.
    pub workspace_name: Option<String>,
    /// Absent while the server roles that decide it are not read.
    pub standing: Option<Standing>,
    /// From the highest position to the lowest; the "everyone" role is the last one.
    pub roles: Listed<Vec<RoleRow>>,
    pub create: Result<(), NoCreation>,
}

impl RolesPage {
    pub fn row(&self, role: RoleRef) -> Option<&RoleRow> {
        self.roles.loaded()?.iter().find(|row| row.role == role)
    }

    /// True when the role can be put at the place `to`: both places and every place
    /// between them hold roles below the person, and none is the "everyone" role.
    pub fn takes(&self, role: RoleRef, to: usize) -> bool {
        !self.moved(role, to).is_empty()
    }

    /// The new positions that put the role at the place `to`, counted from the highest
    /// role. The roles between the two places move one place towards the old place of
    /// the role, and each role takes a position that one of them held, so the request
    /// collides with no role outside it (ADR 0309). Empty when the page does not offer
    /// the move.
    pub fn moved(&self, role: RoleRef, to: usize) -> Vec<(RoleRef, i32)> {
        let Some(rows) = self.roles.loaded() else {
            return Vec::new();
        };
        let Some(from) = rows.iter().position(|row| row.role == role) else {
            return Vec::new();
        };
        if from == to || to >= rows.len() {
            return Vec::new();
        }
        let (first, last) = (from.min(to), from.max(to));
        let span = &rows[first..=last];
        if span
            .iter()
            .any(|row| row.locked.is_some() || row.is_everyone)
            || !settled(rows, first, last)
        {
            return Vec::new();
        }
        let mut order: Vec<RoleRef> = span.iter().map(|row| row.role).collect();
        let moved = order.remove(from - first);
        order.insert(to - first, moved);
        order
            .into_iter()
            .zip(span.iter().map(|row| row.position))
            .filter(|(role, position)| {
                span.iter()
                    .any(|row| row.role == *role && row.position != *position)
            })
            .collect()
    }
}

/// A role in the list of the roles that an account holds or can be given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleChoice {
    pub role: RoleRef,
    pub name: String,
    /// The account holds the role.
    pub held: bool,
    /// Why the person cannot give the role, or take it when the account holds it.
    pub blocked: Option<NoAssignment>,
}

/// Why a role cannot be given to an account or taken from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoAssignment {
    Locked(Locked),
    /// The role holds a permission that the person does not hold: giving it is refused by
    /// the grant rule (ADR 0304). Taking it is possible.
    BeyondOwn,
    /// The account holds the most roles of the level (ADR 0306).
    Full,
    /// The server states that the person may not give the role, or not take it
    /// (ADR 0311).
    NotAllowed,
    /// The statement of the server is on its way.
    Reading,
}

/// The most roles that one reorder names (ADR 0309).
const MOST_MOVED: usize = 250;

/// True when the server takes a reorder of the roles from the place `first` to the place
/// `last`, as far as the state can tell: their positions fall from one role to the next,
/// each lies in the range of a reorder, no role outside them holds one of these
/// positions, and they are not more than one request names. The positions are not in
/// order while a reorder of another person arrives, one event for each role.
fn settled(rows: &[RoleRow], first: usize, last: usize) -> bool {
    let Some(span) = rows.get(first..=last) else {
        return false;
    };
    span.len() <= MOST_MOVED
        && in_range(span)
        && span
            .windows(2)
            .all(|pair| pair[0].position > pair[1].position)
        && rows
            .iter()
            .enumerate()
            .filter(|(index, _)| !(first..=last).contains(index))
            .all(|(_, outside)| span.iter().all(|row| row.position != outside.position))
}

/// True when every position is one that a reorder gives a role.
fn in_range(span: &[RoleRow]) -> bool {
    span.iter()
        .all(|row| (1..=MOST_POSITION).contains(&row.position))
}

fn locked(own: RoleRank, role: RoleRank, held: bool, highest: bool) -> Option<Locked> {
    if outranks(own, role) {
        None
    } else if held && highest {
        Some(Locked::OwnHighest)
    } else {
        Some(Locked::Above)
    }
}

/// The places that one move up and one move down give each row.
fn set_moves(rows: &mut [RoleRow]) {
    let free = |row: &RoleRow| row.locked.is_none() && !row.is_everyone;
    let state: Vec<(bool, Option<Locked>, bool)> = rows
        .iter()
        .map(|row| (free(row), row.locked, row.is_everyone))
        .collect();
    let orderly: Vec<bool> = (0..rows.len())
        .map(|index| index + 1 < rows.len() && settled(rows, index, index + 1))
        .collect();
    // Why the exchange of the role at a place with the one below it is not offered.
    let unsettled: Vec<Fixed> = (0..rows.len())
        .map(|index| match rows.get(index..=index + 1) {
            Some(pair) if !in_range(pair) => Fixed::OutOfRange,
            _ => Fixed::Unsettled,
        })
        .collect();
    for (index, row) in rows.iter_mut().enumerate() {
        let own = match state[index] {
            (true, _, _) => Ok(()),
            (_, _, true) => Err(Fixed::Everyone),
            (_, Some(locked), _) => Err(Fixed::Locked(locked)),
            // A row is free, the "everyone" role, or locked.
            (false, None, false) => Err(Fixed::NoPlaceAbove),
        };
        row.up = own.and_then(|()| {
            let above = index
                .checked_sub(1)
                .filter(|above| state[*above].0)
                .ok_or(Fixed::NoPlaceAbove)?;
            orderly[above].then_some(above).ok_or(unsettled[above])
        });
        row.down = own.and_then(|()| {
            let below = Some(index + 1)
                .filter(|below| state.get(*below).is_some_and(|below| below.0))
                .ok_or(Fixed::NoPlaceBelow)?;
            orderly[index].then_some(below).ok_or(unsettled[index])
        });
    }
}

fn workspace_rows(
    workspace: &WorkspaceModel,
    own: RoleRank,
    stated: impl Fn(WorkspaceRoleId) -> Stated,
) -> Vec<RoleRow> {
    let workspace_id = workspace.workspace.id;
    let highest = workspace
        .roles
        .iter()
        .filter(|role| !role.is_everyone && workspace.own_role_ids.contains(&role.id))
        .map(|role| role.position)
        .max();
    // The state holds the "everyone" role first; the page shows it last, at its place in
    // the order.
    let others = workspace.roles.iter().filter(|role| !role.is_everyone);
    let everyone = workspace.roles.iter().filter(|role| role.is_everyone);
    let mut rows: Vec<RoleRow> = others
        .chain(everyone)
        .map(|role| {
            let held = role.is_everyone || workspace.own_role_ids.contains(&role.id);
            RoleRow {
                role: RoleRef::Workspace(workspace_id, role.id),
                name: role.name.clone(),
                position: role.position,
                is_everyone: role.is_everyone,
                held,
                locked: locked(
                    own,
                    RoleRank::Workspace(role.position),
                    held,
                    highest == Some(role.position) || (role.is_everyone && highest.is_none()),
                ),
                up: Err(Fixed::NoPlaceAbove),
                down: Err(Fixed::NoPlaceBelow),
                named: !role.is_everyone,
                permissions: WorkspacePermission::ALL
                    .iter()
                    .copied()
                    .map(|permission| PermissionSwitch {
                        permission: RolePermission::Workspace(permission),
                        on: role.permissions.contains(&permission),
                        beyond_own: !workspace.holds(permission),
                    })
                    .collect(),
                stated: stated(role.id),
            }
        })
        .collect();
    set_moves(&mut rows);
    rows
}

fn server_rows(model: &ServerModel, own: RoleRank) -> Option<Vec<RoleRow>> {
    let roles = model.admin.server_roles.loaded()?;
    let held = |role_id: ServerRoleId| model.account.server_role_ids.contains(&role_id);
    let highest = roles
        .iter()
        .filter(|role| held(role.id))
        .map(|role| role.position)
        .max();
    let state = model.admin.state(AdminPart::ServerRoles);
    let mut rows: Vec<RoleRow> = roles
        .iter()
        .map(|role| RoleRow {
            stated: stated(state, model.admin.server_role_actions(role.id)),
            role: RoleRef::Server(role.id),
            name: role.name.clone(),
            position: role.position,
            is_everyone: false,
            held: held(role.id),
            locked: locked(
                own,
                RoleRank::Server(role.position),
                held(role.id),
                highest == Some(role.position),
            ),
            up: Err(Fixed::NoPlaceAbove),
            down: Err(Fixed::NoPlaceBelow),
            named: true,
            permissions: ServerPermission::ALL
                .iter()
                .copied()
                .map(|permission| PermissionSwitch {
                    permission: RolePermission::Server(permission),
                    on: role.permissions.contains(&permission),
                    // A change of a server role needs the administrator permission,
                    // which passes every check of a server permission (ADR 0304).
                    beyond_own: false,
                })
                .collect(),
        })
        .collect();
    set_moves(&mut rows);
    Some(rows)
}

fn standing(own: RoleRank, highest: Option<String>) -> Standing {
    match (own, highest) {
        (RoleRank::Owner, _) => Standing::Owner,
        // In a workspace a place among the server roles is the place of an
        // administrator (ADR 0309).
        (RoleRank::Server(_), None) => Standing::NoRole,
        (_, Some(name)) => Standing::Role(name),
        (RoleRank::Workspace(_), None) => Standing::NoRole,
    }
}

/// The page of the roles of one level. Absent for a workspace that the state does not
/// hold.
pub fn derive(model: &ServerModel, level: RoleLevel) -> Option<RolesPage> {
    match level {
        RoleLevel::Server => {
            let own = model.server_rank();
            let highest = model.admin.server_roles.loaded().and_then(|roles| {
                roles
                    .iter()
                    .filter(|role| model.account.server_role_ids.contains(&role.id))
                    .max_by_key(|role| role.position)
                    .map(|role| role.name.clone())
            });
            let roles = match own {
                Some(own) => Listed::of(&model.admin.server_roles, |_| {
                    server_rows(model, own).unwrap_or_default()
                }),
                // Where the person stands is not known: the list is on its way, or it
                // does not name a role that the account holds yet. No row is shown as
                // one that the person can change.
                None => match &model.admin.server_roles {
                    junto_client_core::model::Collection::Failed(failure) => {
                        Listed::Failed(*failure)
                    }
                    _ => Listed::Loading,
                },
            };
            let count = roles.loaded().map_or(0, Vec::len);
            Some(RolesPage {
                level,
                workspace_name: None,
                standing: own.map(|own| standing(own, highest)),
                create: creation(own, RoleRank::Server(0), count),
                roles,
            })
        }
        RoleLevel::Workspace(workspace_id) => {
            let workspace = model.workspace(workspace_id)?;
            let own = model.workspace_rank(workspace_id);
            let highest = workspace
                .roles
                .iter()
                .filter(|role| !role.is_everyone && workspace.own_role_ids.contains(&role.id))
                .max_by_key(|role| role.position)
                .map(|role| role.name.clone());
            let roles = match own {
                Some(own) => Listed::Loaded(workspace_rows(
                    workspace,
                    own,
                    stated_of_workspace(model, workspace_id),
                )),
                // An administrator stands at a server role whose position is not read
                // yet (ADR 0309).
                None => match &model.admin.server_roles {
                    junto_client_core::model::Collection::Failed(failure) => {
                        Listed::Failed(*failure)
                    }
                    _ => Listed::Loading,
                },
            };
            let standing = own.map(|own| match own {
                RoleRank::Server(_) => Standing::Administrator,
                own => standing(own, highest),
            });
            Some(RolesPage {
                level,
                workspace_name: Some(workspace.workspace.name.clone()),
                standing,
                create: creation(own, RoleRank::Workspace(0), workspace.roles.len()),
                roles,
            })
        }
    }
}

fn creation(own: Option<RoleRank>, lowest: RoleRank, count: usize) -> Result<(), NoCreation> {
    // A new role enters at the bottom, and the person must stand above that place.
    let Some(own) = own else {
        return Err(NoCreation::NotRead);
    };
    if !outranks(own, lowest) {
        Err(NoCreation::NoRole)
    } else if count >= MOST_ROLES {
        Err(NoCreation::Full)
    } else {
        Ok(())
    }
}

/// Why the person cannot give a role or take it, by the statement of the server when it
/// has one (ADR 0311): role order and the grant rule then only explain a refusal of the
/// server. Without a statement the client decides by role order and the grant rule.
fn assignment(row: &RoleRow, holds: bool, full: bool, beyond: bool) -> Option<NoAssignment> {
    let stated_refusal = match &row.stated {
        Stated::Reading => return Some(NoAssignment::Reading),
        Stated::Known(actions) => {
            let allowed = if holds {
                actions.may_remove
            } else {
                actions.may_assign
            };
            if allowed {
                return (full && !holds).then_some(NoAssignment::Full);
            }
            Some(NoAssignment::NotAllowed)
        }
        Stated::Unknown => None,
    };
    row.locked
        .map(NoAssignment::Locked)
        .or((beyond && !holds).then_some(NoAssignment::BeyondOwn))
        .or(stated_refusal)
        .or((full && !holds).then_some(NoAssignment::Full))
}

/// The server roles that an account holds or can be given, from the highest to the
/// lowest. Absent while the server roles or the place of the person are not read.
pub fn server_choices(model: &ServerModel, held: &[ServerRoleId]) -> Option<Vec<RoleChoice>> {
    let own = model.server_rank()?;
    let rows = server_rows(model, own)?;
    let full = held.len() >= MOST_HELD_ROLES;
    Some(
        rows.into_iter()
            .map(|row| {
                let holds = matches!(row.role, RoleRef::Server(role_id) if held.contains(&role_id));
                RoleChoice {
                    blocked: assignment(&row, holds, full, false),
                    role: row.role,
                    name: row.name,
                    held: holds,
                }
            })
            .collect(),
    )
}

/// The roles of a workspace that a member holds or can be given, from the highest to the
/// lowest, without the "everyone" role. Absent while the place of the person is not read.
pub fn workspace_choices(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    held: &[WorkspaceRoleId],
) -> Option<Vec<RoleChoice>> {
    let own = model.workspace_rank(workspace.workspace.id)?;
    let full = held.len() >= MOST_HELD_ROLES;
    let stated = stated_of_workspace(model, workspace.workspace.id);
    Some(
        workspace_rows(workspace, own, stated)
            .into_iter()
            .filter(|row| !row.is_everyone)
            .map(|row| {
                let holds =
                    matches!(row.role, RoleRef::Workspace(_, role_id) if held.contains(&role_id));
                // Giving a role adds its permissions to the member, so the person must
                // hold each of them (ADR 0304).
                let beyond = row
                    .permissions
                    .iter()
                    .any(|switch| switch.on && switch.beyond_own);
                RoleChoice {
                    blocked: assignment(&row, holds, full, beyond),
                    role: row.role,
                    name: row.name,
                    held: holds,
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use junto_client_core::model::{AdminModel, Collection, LoadFailure};
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{AccountId, StreamEpoch, WorkspaceId};
    use junto_client_core::protocol::roles::{ServerRole, WorkspaceRole};
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::Workspace;

    use super::*;

    struct Scene {
        studio: WorkspaceId,
        everyone: WorkspaceRoleId,
        guests: WorkspaceRoleId,
        helpers: WorkspaceRoleId,
        leads: WorkspaceRoleId,
        snapshot: Snapshot,
    }

    /// The workspace "Studio" with the roles "leads" (3), "helpers" (2) and "guests" (1)
    /// above the "everyone" role. The account "Luca" holds "helpers" and the permissions
    /// "view channel" and "manage roles".
    fn scene() -> Scene {
        let studio = WorkspaceId::generate();
        let role = |name: &str, position, permissions: &[WorkspacePermission]| WorkspaceRole {
            position,
            id: WorkspaceRoleId::generate(),
            workspace_id: studio,
            name: name.to_owned(),
            permissions: permissions.to_vec(),
            is_everyone: position == 0,
        };
        let everyone = role("everyone", 0, &[WorkspacePermission::ViewChannel]);
        let guests = role("guests", 1, &[]);
        let helpers = role("helpers", 2, &[WorkspacePermission::ManageRoles]);
        let leads = role("leads", 3, &[WorkspacePermission::ManageChannel]);
        let snapshot = Snapshot {
            cursor: Cursor {
                epoch: StreamEpoch::generate(),
                position: 1,
            },
            account: OwnAccount {
                id: AccountId::generate(),
                name: "Luca".to_owned(),
                avatar: None,
                is_owner: false,
                server_role_ids: Vec::new(),
                server_permissions: Vec::new(),
                workspace_ids: vec![studio],
            },
            workspaces: vec![WorkspaceState {
                workspace: Workspace {
                    id: studio,
                    name: "Studio".to_owned(),
                    is_default: true,
                },
                own_role_ids: vec![helpers.id],
                roles: vec![
                    everyone.clone(),
                    guests.clone(),
                    helpers.clone(),
                    leads.clone(),
                ],
                categories: Vec::new(),
                channels: Vec::new(),
                calls: Vec::new(),
                permissions: vec![
                    WorkspacePermission::ViewChannel,
                    WorkspacePermission::ManageRoles,
                ],
                channel_permissions: Vec::new(),
                notification_default: None,
            }],
            own_call: None,
            direct_calls: Vec::new(),
            call_invitations: Vec::new(),
            sent_call_invitations: Vec::new(),
            call_moves: Vec::new(),
            entry_requests: Vec::new(),
            direct_mutes: Vec::new(),
            messages: Vec::new(),
            threads: Vec::new(),
            threads_more: false,
            threads_next: None,
            notifications: Vec::new(),
            notification_settings: Vec::new(),
        };
        Scene {
            studio,
            everyone: everyone.id,
            guests: guests.id,
            helpers: helpers.id,
            leads: leads.id,
            snapshot,
        }
    }

    impl Scene {
        fn model(&self) -> ServerModel {
            ServerModel::from_snapshot(self.snapshot.clone())
        }

        fn role(&self, role_id: WorkspaceRoleId) -> RoleRef {
            RoleRef::Workspace(self.studio, role_id)
        }

        fn page(&self, model: &ServerModel) -> RolesPage {
            derive(model, RoleLevel::Workspace(self.studio)).unwrap()
        }
    }

    fn names(page: &RolesPage) -> Vec<&str> {
        page.roles
            .loaded()
            .unwrap()
            .iter()
            .map(|row| row.name.as_str())
            .collect()
    }

    fn server_role(name: &str, position: i32, permissions: &[ServerPermission]) -> ServerRole {
        ServerRole {
            position,
            id: ServerRoleId::generate(),
            name: name.to_owned(),
            permissions: permissions.to_vec(),
        }
    }

    /// The server roles as the server lists them for the account of the state: what the
    /// account may do with each one follows role order, as the server states it without
    /// channel overrides (ADR 0311).
    fn with_server_roles(model: &mut ServerModel, roles: Vec<ServerRole>) {
        let mut roles = roles;
        roles.sort_by_key(|role| std::cmp::Reverse(role.position));
        let own = roles
            .iter()
            .filter(|role| model.account.server_role_ids.contains(&role.id))
            .map(|role| role.position)
            .max();
        let actions = roles
            .iter()
            .map(|role| {
                let below = model.account.is_owner || own.is_some_and(|own| role.position < own);
                (
                    role.id,
                    RoleActions {
                        may_assign: below,
                        may_remove: below,
                        may_delete: below,
                    },
                )
            })
            .collect();
        model.admin = Arc::new(AdminModel {
            server_roles: Collection::Loaded(roles),
            server_role_actions: actions,
            ..AdminModel::default()
        });
    }

    #[test]
    fn the_roles_of_a_workspace_are_listed_from_the_highest_with_everyone_last() {
        let scene = scene();
        let page = scene.page(&scene.model());
        assert_eq!(names(&page), ["leads", "helpers", "guests", "everyone"]);
        assert_eq!(page.workspace_name.as_deref(), Some("Studio"));
        assert_eq!(page.standing, Some(Standing::Role("helpers".to_owned())));
        assert_eq!(page.create, Ok(()));
    }

    #[test]
    fn a_role_at_or_above_the_highest_role_of_the_person_is_locked_before_the_person_tries() {
        let scene = scene();
        let page = scene.page(&scene.model());
        let row = |role_id| page.row(scene.role(role_id)).unwrap();
        // The role above the person, and the highest role of the person itself.
        assert_eq!(row(scene.leads).locked, Some(Locked::Above));
        assert_eq!(row(scene.helpers).locked, Some(Locked::OwnHighest));
        assert!(row(scene.helpers).held && !row(scene.leads).held);
        // The roles below the person can be changed, the "everyone" role too.
        assert_eq!(row(scene.guests).locked, None);
        assert_eq!(row(scene.everyone).locked, None);
        assert!(row(scene.everyone).is_everyone && !row(scene.everyone).named);
        assert!(row(scene.guests).named);
    }

    #[test]
    fn a_person_without_a_role_can_act_on_no_role_and_create_none() {
        let mut scene = scene();
        scene.snapshot.workspaces[0].own_role_ids = Vec::new();
        let page = scene.page(&scene.model());
        assert_eq!(page.standing, Some(Standing::NoRole));
        assert_eq!(page.create, Err(NoCreation::NoRole));
        for row in page.roles.loaded().unwrap() {
            assert!(row.locked.is_some(), "{}", row.name);
        }
        // The "everyone" role is the place of the person.
        assert_eq!(
            page.row(scene.role(scene.everyone)).unwrap().locked,
            Some(Locked::OwnHighest)
        );
    }

    #[test]
    fn the_owner_and_an_administrator_stand_above_every_role_of_a_workspace() {
        let scene = scene();
        let mut owner = scene.model();
        owner.account.is_owner = true;
        let page = scene.page(&owner);
        assert_eq!(page.standing, Some(Standing::Owner));
        assert!(
            page.roles
                .loaded()
                .unwrap()
                .iter()
                .all(|row| row.locked.is_none())
        );

        // An administrator stands at a server role, whose position must be read first.
        let stewards = server_role("stewards", 4, &[ServerPermission::Administrator]);
        let mut administrator = scene.model();
        administrator.account.server_role_ids = vec![stewards.id];
        administrator.account.server_permissions = vec![ServerPermission::Administrator];
        let page = scene.page(&administrator);
        assert_eq!(page.standing, None);
        assert_eq!(page.roles, Listed::Loading);
        administrator.admin = Arc::new(AdminModel {
            server_roles: Collection::Failed(LoadFailure::NoAnswer),
            ..AdminModel::default()
        });
        assert_eq!(
            scene.page(&administrator).roles,
            Listed::Failed(LoadFailure::NoAnswer)
        );
        with_server_roles(&mut administrator, vec![stewards]);
        let page = scene.page(&administrator);
        assert_eq!(page.standing, Some(Standing::Administrator));
        assert!(
            page.roles
                .loaded()
                .unwrap()
                .iter()
                .all(|row| row.locked.is_none())
        );
        assert_eq!(page.create, Ok(()));
    }

    #[test]
    fn a_permission_that_the_person_does_not_hold_cannot_be_given_to_a_role() {
        let scene = scene();
        let page = scene.page(&scene.model());
        let guests = page.row(scene.role(scene.guests)).unwrap();
        let switch = |permission| {
            *guests
                .permissions
                .iter()
                .find(|held| held.permission == RolePermission::Workspace(permission))
                .unwrap()
        };
        // The person holds "view channel" and "manage roles" in the workspace.
        assert!(!switch(WorkspacePermission::ViewChannel).beyond_own);
        assert!(!switch(WorkspacePermission::ManageRoles).beyond_own);
        assert!(switch(WorkspacePermission::ManageChannel).beyond_own);
        assert!(!switch(WorkspacePermission::ManageChannel).on);
        assert_eq!(guests.permissions.len(), WorkspacePermission::ALL.len());
        assert_eq!(guests.granted(), 0);
        assert_eq!(page.row(scene.role(scene.everyone)).unwrap().granted(), 1);
    }

    #[test]
    fn a_role_moves_only_among_the_roles_below_the_person() {
        let mut scene = scene();
        // The person holds "leads": "helpers" and "guests" are below.
        scene.snapshot.workspaces[0].own_role_ids = vec![scene.leads];
        let page = scene.page(&scene.model());
        let row = |role_id| page.row(scene.role(role_id)).unwrap();
        assert_eq!(row(scene.leads).up, Err(Fixed::Locked(Locked::OwnHighest)));
        assert_eq!(
            row(scene.leads).down,
            Err(Fixed::Locked(Locked::OwnHighest))
        );
        // The place above "helpers" is the highest role of the person.
        assert_eq!(row(scene.helpers).up, Err(Fixed::NoPlaceAbove));
        assert_eq!(row(scene.helpers).down, Ok(2));
        assert_eq!(row(scene.guests).up, Ok(1));
        // The place below "guests" is the "everyone" role, which does not move.
        assert_eq!(row(scene.guests).down, Err(Fixed::NoPlaceBelow));
        assert_eq!(row(scene.everyone).up, Err(Fixed::Everyone));
        assert_eq!(row(scene.everyone).down, Err(Fixed::Everyone));

        // One place: the two roles exchange their positions.
        assert_eq!(
            page.moved(scene.role(scene.guests), 1),
            [
                (scene.role(scene.guests), 2),
                (scene.role(scene.helpers), 1)
            ]
        );
        assert!(page.takes(scene.role(scene.helpers), 2));
        // No place at or above the person, none at the "everyone" role, none outside the
        // list, and the own place is no move.
        assert!(!page.takes(scene.role(scene.guests), 0));
        assert!(!page.takes(scene.role(scene.guests), 3));
        assert!(!page.takes(scene.role(scene.guests), 9));
        assert!(!page.takes(scene.role(scene.guests), 2));
        assert!(!page.takes(scene.role(scene.leads), 2));
        assert!(!page.takes(scene.role(scene.everyone), 1));
    }

    #[test]
    fn a_move_over_several_places_gives_each_role_between_them_a_position_of_the_group() {
        let scene = scene();
        let mut owner = scene.model();
        owner.account.is_owner = true;
        let page = scene.page(&owner);
        // "guests" from the third place to the first: "leads" and "helpers" move down.
        assert_eq!(
            page.moved(scene.role(scene.guests), 0),
            [
                (scene.role(scene.guests), 3),
                (scene.role(scene.leads), 2),
                (scene.role(scene.helpers), 1),
            ]
        );
        // "leads" from the first place to the third.
        assert_eq!(
            page.moved(scene.role(scene.leads), 2),
            [
                (scene.role(scene.helpers), 3),
                (scene.role(scene.guests), 2),
                (scene.role(scene.leads), 1),
            ]
        );
        // The positions of the group are used again, also when they have gaps.
        let mut gapped = scene.snapshot.clone();
        for role in &mut gapped.workspaces[0].roles {
            role.position *= 10;
        }
        let mut owner = ServerModel::from_snapshot(gapped);
        owner.account.is_owner = true;
        let page = scene.page(&owner);
        assert_eq!(
            page.moved(scene.role(scene.guests), 1),
            [
                (scene.role(scene.guests), 20),
                (scene.role(scene.helpers), 10)
            ]
        );
    }

    #[test]
    fn the_server_roles_are_listed_for_an_administrator_below_its_highest_role() {
        let scene = scene();
        let mut model = scene.model();
        let hosts = server_role("hosts", 1, &[ServerPermission::CreateInvites]);
        let wardens = server_role("wardens", 2, &[ServerPermission::BanMembers]);
        let stewards = server_role("stewards", 3, &[ServerPermission::Administrator]);
        let elders = server_role("elders", 4, &[ServerPermission::Administrator]);
        model.account.server_role_ids = vec![stewards.id, hosts.id];
        model.account.server_permissions = vec![
            ServerPermission::Administrator,
            ServerPermission::CreateInvites,
        ];
        // The roles are on their way: the place of the person is not known.
        let page = derive(&model, RoleLevel::Server).unwrap();
        assert_eq!(page.roles, Listed::Loading);
        assert_eq!(page.standing, None);

        with_server_roles(
            &mut model,
            vec![
                hosts.clone(),
                wardens.clone(),
                stewards.clone(),
                elders.clone(),
            ],
        );
        let page = derive(&model, RoleLevel::Server).unwrap();
        assert_eq!(names(&page), ["elders", "stewards", "wardens", "hosts"]);
        assert_eq!(page.standing, Some(Standing::Role("stewards".to_owned())));
        assert_eq!(page.workspace_name, None);
        let row = |role: &ServerRole| page.row(RoleRef::Server(role.id)).unwrap();
        assert_eq!(row(&elders).locked, Some(Locked::Above));
        assert_eq!(row(&stewards).locked, Some(Locked::OwnHighest));
        assert_eq!(row(&wardens).locked, None);
        // A lower role of the person itself can be changed (ADR 0309).
        assert_eq!(row(&hosts).locked, None);
        assert!(row(&hosts).held);
        assert_eq!(row(&wardens).up, Err(Fixed::NoPlaceAbove));
        assert_eq!(row(&wardens).down, Ok(3));
        assert_eq!(row(&hosts).up, Ok(2));
        assert_eq!(row(&hosts).down, Err(Fixed::NoPlaceBelow));
        // No server permission is beyond an administrator.
        assert!(
            row(&wardens)
                .permissions
                .iter()
                .all(|held| !held.beyond_own)
        );
        assert_eq!(row(&wardens).permissions.len(), ServerPermission::ALL.len());
        assert_eq!(page.create, Ok(()));
    }

    #[test]
    fn a_page_whose_person_stands_at_an_unknown_place_shows_no_row_and_takes_no_new_role() {
        let scene = scene();
        let mut model = scene.model();
        let hosts = server_role("hosts", 1, &[ServerPermission::Administrator]);
        // The account holds a server role that the loaded list does not name yet: the
        // list was read before the role was given.
        model.account.server_role_ids = vec![hosts.id, ServerRoleId::generate()];
        model.account.server_permissions = vec![ServerPermission::Administrator];
        with_server_roles(&mut model, vec![hosts]);
        for level in [RoleLevel::Server, RoleLevel::Workspace(scene.studio)] {
            let page = derive(&model, level).unwrap();
            assert_eq!(page.roles, Listed::Loading, "{level:?}");
            assert_eq!(page.standing, None);
            assert_eq!(page.create, Err(NoCreation::NotRead));
        }
        assert_eq!(server_choices(&model, &[]), None);
    }

    #[test]
    fn a_role_is_given_or_taken_only_below_the_person_and_within_the_own_permissions() {
        let mut scene = scene();
        scene.snapshot.workspaces[0].own_role_ids = vec![scene.leads];
        // "helpers" holds "manage roles" and "guests" nothing; the person holds "view
        // channel" and "manage channel" and not "manage roles".
        scene.snapshot.workspaces[0].permissions = vec![
            WorkspacePermission::ViewChannel,
            WorkspacePermission::ManageChannel,
        ];
        let model = scene.model();
        let workspace = model.workspace(scene.studio).unwrap();
        let choices = workspace_choices(&model, workspace, &[scene.helpers]).unwrap();
        let names: Vec<&str> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, ["leads", "helpers", "guests"]);
        let choice = |role_id| {
            choices
                .iter()
                .find(|choice| choice.role == scene.role(role_id))
                .unwrap()
        };
        assert_eq!(
            choice(scene.leads).blocked,
            Some(NoAssignment::Locked(Locked::OwnHighest))
        );
        // The member holds "helpers": taking it adds no permission, so it is possible.
        assert!(choice(scene.helpers).held);
        assert_eq!(choice(scene.helpers).blocked, None);
        assert_eq!(choice(scene.guests).blocked, None);
        // Giving "helpers" to a member who does not hold it would give "manage roles",
        // which the person does not hold.
        let choices = workspace_choices(&model, workspace, &[]).unwrap();
        let helpers = choices
            .iter()
            .find(|choice| choice.role == scene.role(scene.helpers))
            .unwrap();
        assert_eq!(helpers.blocked, Some(NoAssignment::BeyondOwn));

        // A member with the most roles takes no further one and can lose one.
        let many: Vec<WorkspaceRoleId> = (0..MOST_HELD_ROLES - 1)
            .map(|_| WorkspaceRoleId::generate())
            .chain([scene.helpers])
            .collect();
        let choices = workspace_choices(&model, workspace, &many).unwrap();
        let of = |role_id| {
            choices
                .iter()
                .find(|choice| choice.role == scene.role(role_id))
                .unwrap()
                .blocked
        };
        assert_eq!(of(scene.guests), Some(NoAssignment::Full));
        assert_eq!(of(scene.helpers), None);
    }

    #[test]
    fn the_statement_of_the_server_decides_giving_taking_and_deleting_a_role() {
        let mut scene = scene();
        scene.snapshot.workspaces[0].own_role_ids = vec![scene.leads];
        let mut model = scene.model();
        let allowed = RoleActions {
            may_assign: true,
            may_remove: true,
            may_delete: true,
        };
        let refused = RoleActions {
            may_assign: false,
            may_remove: false,
            may_delete: false,
        };
        let set = |model: &mut ServerModel,
                   actions: Option<Vec<(WorkspaceRoleId, RoleActions)>>| {
            let mut admin = model.admin.as_ref().clone();
            admin.role_actions = vec![(
                scene.studio,
                match actions {
                    Some(actions) => Collection::Loaded(actions),
                    None => Collection::Loading,
                },
            )];
            model.admin = Arc::new(admin);
        };
        // The server allows "helpers" and refuses "guests": an override of "guests" in a
        // channel denies a permission that the person does not hold.
        set(
            &mut model,
            Some(vec![(scene.helpers, allowed), (scene.guests, refused)]),
        );
        let page = scene.page(&model);
        let row = |role_id| page.row(scene.role(role_id)).unwrap().clone();
        assert_eq!(row(scene.helpers).deletion(), Ok(()));
        assert_eq!(row(scene.guests).deletion(), Err(NoDeletion::NotAllowed));
        // Role order explains a refusal of a role above the person.
        assert_eq!(
            row(scene.leads).deletion(),
            Err(NoDeletion::Locked(Locked::OwnHighest))
        );
        let workspace = model.workspace(scene.studio).unwrap();
        let choices = workspace_choices(&model, workspace, &[scene.guests]).unwrap();
        let of = |role_id| {
            choices
                .iter()
                .find(|choice| choice.role == scene.role(role_id))
                .unwrap()
                .blocked
        };
        assert_eq!(of(scene.helpers), None);
        assert_eq!(of(scene.guests), Some(NoAssignment::NotAllowed));
        // A role that the list does not name was added after the read: the read after
        // that event is on its way, and so is the first read.
        assert_eq!(row(scene.everyone).stated, Stated::Reading);
        set(&mut model, None);
        let page = scene.page(&model);
        let guests = page.row(scene.role(scene.guests)).unwrap();
        assert_eq!(guests.deletion(), Err(NoDeletion::Reading));
        let workspace = model.workspace(scene.studio).unwrap();
        let choices = workspace_choices(&model, workspace, &[]).unwrap();
        assert!(
            choices
                .iter()
                .filter(|choice| choice.role != scene.role(scene.leads))
                .all(|choice| choice.blocked == Some(NoAssignment::Reading))
        );
    }

    #[test]
    fn the_server_roles_of_an_account_are_offered_below_the_person() {
        let scene = scene();
        let mut model = scene.model();
        let hosts = server_role("hosts", 1, &[ServerPermission::CreateInvites]);
        let stewards = server_role("stewards", 2, &[ServerPermission::Administrator]);
        model.account.server_role_ids = vec![stewards.id];
        model.account.server_permissions = vec![ServerPermission::Administrator];
        assert_eq!(server_choices(&model, &[]), None);
        with_server_roles(&mut model, vec![hosts.clone(), stewards.clone()]);
        let choices = server_choices(&model, &[hosts.id]).unwrap();
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].name, "stewards");
        assert_eq!(
            choices[0].blocked,
            Some(NoAssignment::Locked(Locked::OwnHighest))
        );
        assert!(choices[1].held);
        assert_eq!(choices[1].blocked, None);
    }

    #[test]
    fn the_pages_of_the_roles_are_offered_to_the_people_who_manage_roles_of_that_level() {
        use crate::state::admin::view::{nav, needs};
        use crate::state::admin::{AdminOpen, AdminPage};
        use junto_client_core::model::AdminPart;
        use junto_client_core::protocol::ids::ServerId;

        let mut scene = scene();
        let pages = |model: &ServerModel| -> Vec<AdminPage> {
            nav(model, model.workspace(scene.studio))
                .into_iter()
                .flat_map(|group| group.pages)
                .collect()
        };
        let needed = |model: &ServerModel, page| {
            let open = AdminOpen::Server {
                server_id: ServerId::generate(),
                page,
            };
            needs(&open, model, Some(scene.studio))
        };
        // The list of the workspace holds "manage roles": the page of the workspace, and
        // not the page of the server roles.
        let model = scene.model();
        assert!(pages(&model).contains(&AdminPage::Roles));
        assert!(!pages(&model).contains(&AdminPage::ServerRoles));
        // A person who is no administrator stands at a role of the workspace: no server
        // role is read for the page. The server states what the person may do with each
        // role of the workspace (ADR 0311).
        assert_eq!(
            needed(&model, AdminPage::Roles),
            [AdminPart::RoleActions(scene.studio)]
        );

        // Without the permission in the list, the role of the person changes nothing.
        scene.snapshot.workspaces[0].permissions = vec![WorkspacePermission::ViewChannel];
        assert!(!pages(&scene.model()).contains(&AdminPage::Roles));

        // An administrator has both pages, and stands in a workspace at a server role.
        let mut administrator = scene.model();
        administrator.account.server_permissions = vec![ServerPermission::Administrator];
        crate::state::fixtures::restate(&mut administrator);
        let offered = pages(&administrator);
        assert!(offered.contains(&AdminPage::Roles) && offered.contains(&AdminPage::ServerRoles));
        assert_eq!(
            needed(&administrator, AdminPage::Roles),
            [AdminPart::RoleActions(scene.studio), AdminPart::ServerRoles]
        );
        assert_eq!(
            needed(&administrator, AdminPage::ServerRoles),
            [AdminPart::ServerRoles]
        );
        // The owner stands above every role without a position.
        administrator.account.is_owner = true;
        assert_eq!(
            needed(&administrator, AdminPage::Roles),
            [AdminPart::RoleActions(scene.studio)]
        );
        // The people page reads the order of the server roles for every person who has
        // it: a holder of "ban members" too (ADR 0309).
        assert_eq!(
            needed(&model, AdminPage::People),
            [AdminPart::Accounts, AdminPart::ServerRoles]
        );
    }

    #[test]
    fn no_move_is_offered_while_the_positions_of_the_roles_are_not_in_order() {
        let scene = scene();
        let owner_page = |snapshot: &Snapshot| {
            let mut owner = ServerModel::from_snapshot(snapshot.clone());
            owner.account.is_owner = true;
            scene.page(&owner)
        };
        let with_positions = |positions: [(WorkspaceRoleId, i32); 2]| {
            let mut snapshot = scene.snapshot.clone();
            for role in &mut snapshot.workspaces[0].roles {
                if let Some((_, position)) = positions.iter().find(|(id, _)| *id == role.id) {
                    role.position = *position;
                }
            }
            snapshot
        };
        // A reorder of another person arrives as one event per role: between the two
        // events two roles hold one position. A move that names them would be refused,
        // and an exchange of two equal positions would change nothing.
        let page = owner_page(&with_positions([(scene.helpers, 1), (scene.guests, 1)]));
        let row = |page: &RolesPage, role_id| page.row(scene.role(role_id)).unwrap().clone();
        assert_eq!(row(&page, scene.leads).down, Err(Fixed::Unsettled));
        assert_eq!(row(&page, scene.helpers).up, Err(Fixed::Unsettled));
        assert_eq!(row(&page, scene.guests).up, Err(Fixed::Unsettled));
        assert!(!page.takes(scene.role(scene.leads), 2));
        assert!(!page.takes(scene.role(scene.guests), 0));
        // The move of "leads" by one place names "leads" and its neighbour only when
        // their two positions differ.
        let page = owner_page(&scene.snapshot);
        assert_eq!(row(&page, scene.leads).down, Ok(1));
        // A position that the server does not take in a reorder.
        let page = owner_page(&with_positions([(scene.guests, 0), (scene.helpers, 2)]));
        assert_eq!(row(&page, scene.helpers).down, Err(Fixed::OutOfRange));
        let page = owner_page(&with_positions([
            (scene.leads, MOST_POSITION + 1),
            (scene.helpers, 2),
        ]));
        assert_eq!(row(&page, scene.leads).down, Err(Fixed::OutOfRange));
        assert!(!page.takes(scene.role(scene.helpers), 0));
    }

    #[test]
    fn a_level_with_the_most_roles_takes_no_new_one() {
        let mut scene = scene();
        let studio = scene.studio;
        let roles = &mut scene.snapshot.workspaces[0].roles;
        for position in 4..=i32::try_from(MOST_ROLES).unwrap() - 1 {
            roles.push(WorkspaceRole {
                position,
                id: WorkspaceRoleId::generate(),
                workspace_id: studio,
                name: format!("role {position}"),
                permissions: Vec::new(),
                is_everyone: false,
            });
        }
        assert_eq!(roles.len(), MOST_ROLES);
        let page = scene.page(&scene.model());
        assert_eq!(page.create, Err(NoCreation::Full));
    }
}
