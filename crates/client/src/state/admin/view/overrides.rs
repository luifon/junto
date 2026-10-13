//! The page "Permissions" of the settings of a channel (ADR 0301, ADR 0304): the override
//! of each role of the workspace in the channel, and the changes that the grant rule
//! allows the person.

use junto_client_core::model::{ServerModel, WorkspaceModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::WorkspaceRoleId;
use junto_client_core::protocol::structure::{Channel, ChannelOverride};

use super::Listed;

/// What the override of a role states about one permission in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Choice {
    /// The permission is in the allow set.
    Allow,
    /// The permission is in no set: the roles of the workspace decide.
    Default,
    /// The permission is in the deny set.
    Deny,
}

impl Choice {
    pub const ALL: [Self; 3] = [Self::Allow, Self::Default, Self::Deny];
}

/// Why the grant rule refuses a choice (ADR 0304): the person does not hold the
/// permission in the channel, and the choice can give it to a holder of the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The choice adds the permission to the allow set.
    Allows,
    /// The choice removes the permission from the deny set.
    LiftsDeny,
}

/// One permission of one role in the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionLine {
    pub permission: WorkspacePermission,
    /// What the override states now.
    pub current: Choice,
    /// The person holds the permission in the channel.
    pub held: bool,
}

impl PermissionLine {
    /// Why the person may not make a choice now. Absent: the person may. The choice of
    /// now is not a change, and adding a deny and removing an allow give nothing.
    pub const fn refused(&self, choice: Choice) -> Option<Refused> {
        if self.held {
            return None;
        }
        match (self.current, choice) {
            (Choice::Default, Choice::Allow) => Some(Refused::Allows),
            (Choice::Deny, Choice::Default | Choice::Allow) => Some(Refused::LiftsDeny),
            _ => None,
        }
    }

    /// The reason that the row of the permission states: that of its first refused
    /// choice.
    pub fn reason(&self) -> Option<Refused> {
        Choice::ALL
            .into_iter()
            .find_map(|choice| self.refused(choice))
    }
}

/// One role of the workspace with its override in the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleLine {
    pub id: WorkspaceRoleId,
    pub name: String,
    /// The role that every member of the workspace holds.
    pub is_everyone: bool,
    /// The role has an override in the channel now.
    pub has_override: bool,
    /// In the order of `WorkspacePermission::ALL`.
    pub permissions: Vec<PermissionLine>,
}

impl RoleLine {
    fn count(&self, choice: Choice) -> usize {
        self.permissions
            .iter()
            .filter(|line| line.current == choice)
            .count()
    }

    /// The number of permissions that the override allows.
    pub fn allowed(&self) -> usize {
        self.count(Choice::Allow)
    }

    /// The number of permissions that the override denies.
    pub fn denied(&self) -> usize {
        self.count(Choice::Deny)
    }

    /// The permissions that the override denies and the person does not hold in the
    /// channel. The removal of the override removes every deny, so the grant rule refuses
    /// it while this list is not empty.
    pub fn blocks_removal(&self) -> Vec<WorkspacePermission> {
        self.permissions
            .iter()
            .filter(|line| line.current == Choice::Deny && !line.held)
            .map(|line| line.permission)
            .collect()
    }
}

/// The overrides of the roles in one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionsPage {
    /// The "everyone" role first, then the other roles in the order of the workspace.
    pub roles: Listed<Vec<RoleLine>>,
}

/// What an override states about a permission. A permission in both sets, which the
/// server does not store, counts as denied: that gives the role less.
pub fn choice_of(current: Option<&ChannelOverride>, permission: WorkspacePermission) -> Choice {
    match current {
        Some(held) if held.deny.contains(&permission) => Choice::Deny,
        Some(held) if held.allow.contains(&permission) => Choice::Allow,
        _ => Choice::Default,
    }
}

/// The allow set and the deny set of an override after one permission is set to a
/// choice. The other permissions stay. Each set is in the order of
/// `WorkspacePermission::ALL`, and no permission is in both. Two empty sets remove the
/// override.
pub fn changed(
    current: Option<&ChannelOverride>,
    permission: WorkspacePermission,
    choice: Choice,
) -> (Vec<WorkspacePermission>, Vec<WorkspacePermission>) {
    let mut allow = Vec::new();
    let mut deny = Vec::new();
    for held in WorkspacePermission::ALL.iter().copied() {
        let next = if held == permission {
            choice
        } else {
            choice_of(current, held)
        };
        match next {
            Choice::Allow => allow.push(held),
            Choice::Deny => deny.push(held),
            Choice::Default => {}
        }
    }
    (allow, deny)
}

/// The sets of the override of the "everyone" role that make a channel private or open
/// (ADR 0304): "view channel" enters the deny set or leaves it. Absent when the channel
/// is private or open already; an allow of "view channel" stays in an open channel.
pub fn private_change(
    current: Option<&ChannelOverride>,
    private: bool,
) -> Option<(Vec<WorkspacePermission>, Vec<WorkspacePermission>)> {
    let view = WorkspacePermission::ViewChannel;
    let denied = choice_of(current, view) == Choice::Deny;
    let choice = if private {
        Choice::Deny
    } else {
        Choice::Default
    };
    (denied != private).then(|| changed(current, view, choice))
}

/// The page of the overrides of a channel. The permissions that the person holds in the
/// channel decide which choices the person may make; the server decides every request.
pub fn derive(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    channel: &Channel,
) -> PermissionsPage {
    // What the person holds in the channel is the list that the server stated
    // (ADR 0310).
    let held = model.permissions_in(channel.id);
    let roles = Listed::of(model.admin.overrides(channel.id), |overrides| {
        let everyone = workspace.roles.iter().filter(|role| role.is_everyone);
        let others = workspace.roles.iter().filter(|role| !role.is_everyone);
        everyone
            .chain(others)
            .map(|role| {
                let current = overrides.iter().find(|of| of.role_id == role.id);
                RoleLine {
                    id: role.id,
                    name: role.name.clone(),
                    is_everyone: role.is_everyone,
                    has_override: current
                        .is_some_and(|of| !of.allow.is_empty() || !of.deny.is_empty()),
                    permissions: WorkspacePermission::ALL
                        .iter()
                        .copied()
                        .map(|permission| PermissionLine {
                            permission,
                            current: choice_of(current, permission),
                            held: held.is_some_and(|held| held.contains(&permission)),
                        })
                        .collect(),
                }
            })
            .collect()
    });
    PermissionsPage { roles }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use junto_client_core::model::{AdminModel, Collection, LoadFailure};
    use junto_client_core::protocol::ServerPermission;
    use junto_client_core::protocol::WorkspacePermission::{
        DeafenMembers, LockRoom, ManageChannel, MentionEveryone, MoveMembers, MuteMembers,
        RecordCalls, RunMeetingMode, ViewChannel,
    };
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{AccountId, ChannelId, StreamEpoch, WorkspaceId};
    use junto_client_core::protocol::roles::WorkspaceRole;
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{ChannelKind, Workspace};

    use super::*;

    /// One server with the workspace "Studio" and its channel "general". The workspace
    /// holds the roles "leads", "everyone" and "guests" in this order. The "everyone"
    /// role gives "view channel". The account holds the role "leads", which gives "manage
    /// channel".
    struct Scene {
        studio: WorkspaceId,
        everyone: WorkspaceRoleId,
        leads: WorkspaceRoleId,
        guests: WorkspaceRoleId,
        general: ChannelId,
        snapshot: Snapshot,
        admin: AdminModel,
    }

    fn scene() -> Scene {
        let studio = WorkspaceId::generate();
        let role = |name: &str, permissions: &[WorkspacePermission], position| WorkspaceRole {
            position,
            id: WorkspaceRoleId::generate(),
            workspace_id: studio,
            name: name.to_owned(),
            permissions: permissions.to_vec(),
            is_everyone: position == 0,
        };
        let leads = role("leads", &[ManageChannel], 2);
        let everyone = role("everyone", &[ViewChannel], 0);
        let guests = role("guests", &[], 1);
        let general = Channel {
            id: ChannelId::generate(),
            workspace_id: studio,
            category_id: None,
            kind: ChannelKind::Text,
            name: "general".to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        };
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
                own_role_ids: vec![leads.id],
                roles: vec![leads.clone(), everyone.clone(), guests.clone()],
                categories: Vec::new(),
                channels: vec![general.clone()],
                calls: Vec::new(),
                permissions: Vec::new(),
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
        let mut scene = Scene {
            studio,
            everyone: everyone.id,
            leads: leads.id,
            guests: guests.id,
            general: general.id,
            snapshot,
            admin: AdminModel::default(),
        };
        scene.load(Vec::new());
        scene
    }

    fn sets(
        role_id: WorkspaceRoleId,
        allow: &[WorkspacePermission],
        deny: &[WorkspacePermission],
    ) -> ChannelOverride {
        ChannelOverride {
            role_id,
            allow: allow.to_vec(),
            deny: deny.to_vec(),
        }
    }

    impl Scene {
        /// Gives the role of the account these permissions in the workspace.
        fn lead_holds(&mut self, permissions: &[WorkspacePermission]) {
            let leads = self.leads;
            for role in &mut self.snapshot.workspaces[0].roles {
                if role.id == leads {
                    role.permissions = permissions.to_vec();
                }
            }
        }

        /// The server states this list for the account in the channel (ADR 0310): what
        /// its permission function gives with the overrides of the channel.
        fn holds_in_channel(&mut self, permissions: &[WorkspacePermission]) {
            let general = self.general;
            self.snapshot.workspaces[0].channel_permissions =
                vec![junto_client_core::protocol::structure::ChannelPermissions {
                    channel_id: general,
                    permissions: permissions.to_vec(),
                }];
        }

        /// The overrides of the channel are read and hold these.
        fn load(&mut self, overrides: Vec<ChannelOverride>) {
            self.admin.overrides = vec![(self.general, Collection::Loaded(overrides))];
        }

        fn page(&self) -> PermissionsPage {
            let mut model =
                ServerModel::from_snapshot(crate::state::fixtures::stated(self.snapshot.clone()));
            model.admin = Arc::new(self.admin.clone());
            let workspace = model.workspace(self.studio).unwrap();
            let channel = model.readable_channel(self.general).unwrap();
            derive(&model, workspace, channel)
        }

        fn roles(&self) -> Vec<RoleLine> {
            match self.page().roles {
                Listed::Loaded(roles) => roles,
                other => panic!("the overrides are loaded, not {other:?}"),
            }
        }

        fn role(&self, role_id: WorkspaceRoleId) -> RoleLine {
            self.roles()
                .into_iter()
                .find(|role| role.id == role_id)
                .unwrap()
        }

        fn line(
            &self,
            role_id: WorkspaceRoleId,
            permission: WorkspacePermission,
        ) -> PermissionLine {
            self.role(role_id)
                .permissions
                .into_iter()
                .find(|line| line.permission == permission)
                .unwrap()
        }
    }

    #[test]
    fn the_everyone_role_is_listed_first_and_the_other_roles_in_the_order_of_the_workspace() {
        let scene = scene();
        let roles = scene.roles();
        assert_eq!(
            roles.iter().map(|role| role.id).collect::<Vec<_>>(),
            [scene.everyone, scene.leads, scene.guests]
        );
        assert_eq!(
            roles
                .iter()
                .map(|role| (role.name.as_str(), role.is_everyone))
                .collect::<Vec<_>>(),
            [("everyone", true), ("leads", false), ("guests", false)]
        );
    }

    #[test]
    fn each_role_lists_every_permission_in_the_order_of_the_protocol() {
        let scene = scene();
        for role in scene.roles() {
            assert_eq!(
                role.permissions
                    .iter()
                    .map(|line| line.permission)
                    .collect::<Vec<_>>(),
                WorkspacePermission::ALL
            );
        }
        assert_eq!(WorkspacePermission::ALL.len(), 16);
    }

    #[test]
    fn a_permission_shows_the_choice_that_the_override_of_its_role_holds() {
        let mut scene = scene();
        scene.load(vec![sets(
            scene.guests,
            &[MentionEveryone, RecordCalls],
            &[ViewChannel],
        )]);
        assert_eq!(
            scene.line(scene.guests, MentionEveryone).current,
            Choice::Allow
        );
        assert_eq!(scene.line(scene.guests, RecordCalls).current, Choice::Allow);
        assert_eq!(scene.line(scene.guests, ViewChannel).current, Choice::Deny);
        assert_eq!(
            scene.line(scene.guests, MuteMembers).current,
            Choice::Default
        );
        let guests = scene.role(scene.guests);
        assert!(guests.has_override);
        assert_eq!((guests.allowed(), guests.denied()), (2, 1));

        // The override of one role says nothing about another role.
        let leads = scene.role(scene.leads);
        assert!(!leads.has_override);
        assert_eq!((leads.allowed(), leads.denied()), (0, 0));
        assert!(
            leads
                .permissions
                .iter()
                .all(|line| line.current == Choice::Default)
        );
    }

    #[test]
    fn an_override_of_a_role_that_the_workspace_does_not_hold_is_not_shown() {
        let mut scene = scene();
        scene.load(vec![sets(
            WorkspaceRoleId::generate(),
            &[RecordCalls],
            &[ViewChannel],
        )]);
        let roles = scene.roles();
        assert_eq!(roles.len(), 3);
        assert!(roles.iter().all(|role| !role.has_override));
    }

    #[test]
    fn allowing_a_permission_needs_that_permission_in_the_channel() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        let held = scene.line(scene.guests, MuteMembers);
        assert_eq!((held.current, held.held), (Choice::Default, true));
        assert_eq!(held.refused(Choice::Allow), None);
        let missing = scene.line(scene.guests, DeafenMembers);
        assert_eq!((missing.current, missing.held), (Choice::Default, false));
        assert_eq!(missing.refused(Choice::Allow), Some(Refused::Allows));
    }

    #[test]
    fn denying_a_permission_needs_only_manage_channel() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        assert_eq!(
            scene.line(scene.guests, MuteMembers).refused(Choice::Deny),
            None
        );
        assert_eq!(
            scene
                .line(scene.guests, DeafenMembers)
                .refused(Choice::Deny),
            None
        );
    }

    #[test]
    fn an_allow_is_removed_or_made_a_deny_with_and_without_the_permission() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        scene.load(vec![sets(scene.guests, &[MuteMembers, DeafenMembers], &[])]);
        for (permission, holds) in [(MuteMembers, true), (DeafenMembers, false)] {
            let line = scene.line(scene.guests, permission);
            assert_eq!((line.current, line.held), (Choice::Allow, holds));
            assert_eq!(line.refused(Choice::Default), None);
            assert_eq!(line.refused(Choice::Deny), None);
        }
    }

    #[test]
    fn removing_a_deny_needs_the_permission_in_the_channel() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        scene.load(vec![sets(scene.guests, &[], &[MuteMembers, DeafenMembers])]);
        let held = scene.line(scene.guests, MuteMembers);
        assert_eq!((held.current, held.held), (Choice::Deny, true));
        assert_eq!(held.refused(Choice::Default), None);
        let missing = scene.line(scene.guests, DeafenMembers);
        assert_eq!((missing.current, missing.held), (Choice::Deny, false));
        assert_eq!(missing.refused(Choice::Default), Some(Refused::LiftsDeny));
    }

    #[test]
    fn making_a_deny_an_allow_needs_the_permission_in_the_channel() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        scene.load(vec![sets(scene.guests, &[], &[MuteMembers, DeafenMembers])]);
        assert_eq!(
            scene.line(scene.guests, MuteMembers).refused(Choice::Allow),
            None
        );
        assert_eq!(
            scene
                .line(scene.guests, DeafenMembers)
                .refused(Choice::Allow),
            Some(Refused::LiftsDeny)
        );
    }

    #[test]
    fn the_choice_of_now_is_not_a_change_and_is_not_refused() {
        let mut scene = scene();
        scene.load(vec![sets(scene.guests, &[RecordCalls], &[DeafenMembers])]);
        for (permission, current) in [
            (RecordCalls, Choice::Allow),
            (MuteMembers, Choice::Default),
            (DeafenMembers, Choice::Deny),
        ] {
            let line = scene.line(scene.guests, permission);
            assert_eq!((line.current, line.held), (current, false));
            assert_eq!(line.refused(current), None);
        }
    }

    #[test]
    fn a_row_states_the_reason_of_its_refused_choices() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MoveMembers]);
        scene.load(vec![sets(scene.guests, &[RecordCalls], &[DeafenMembers])]);
        assert_eq!(
            scene.line(scene.guests, MuteMembers).reason(),
            Some(Refused::Allows)
        );
        assert_eq!(
            scene.line(scene.guests, DeafenMembers).reason(),
            Some(Refused::LiftsDeny)
        );
        // No choice of an allowed permission gives anything.
        assert_eq!(scene.line(scene.guests, RecordCalls).reason(), None);
        assert_eq!(scene.line(scene.guests, MoveMembers).reason(), None);
    }

    #[test]
    fn an_administrator_and_the_server_owner_may_make_every_choice() {
        let mut scene = scene();
        scene.lead_holds(&[]);
        scene.load(vec![
            sets(scene.everyone, &[], &[ViewChannel]),
            sets(scene.guests, &[RecordCalls], &[DeafenMembers]),
        ]);
        let every_choice_is_free = |scene: &Scene| {
            scene.roles().iter().all(|role| {
                role.blocks_removal().is_empty()
                    && role.permissions.iter().all(|line| {
                        line.held
                            && Choice::ALL
                                .into_iter()
                                .all(|choice| line.refused(choice).is_none())
                    })
            })
        };
        // The account holds nothing in the channel: the channel is private.
        assert!(!every_choice_is_free(&scene));
        scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
        assert!(every_choice_is_free(&scene));
        scene.snapshot.account.server_permissions = Vec::new();
        scene.snapshot.account.is_owner = true;
        assert!(every_choice_is_free(&scene));
    }

    #[test]
    fn the_choices_follow_the_list_that_the_server_states_for_the_channel() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        scene.load(vec![sets(scene.leads, &[RecordCalls], &[MuteMembers])]);
        // The page compares no override: until the server states another list for the
        // channel, the list of the workspace holds.
        assert!(!scene.line(scene.guests, RecordCalls).held);
        assert!(scene.line(scene.guests, MuteMembers).held);
        // The server states the list of the channel: the override of the role of the
        // person allows "record calls" there.
        scene.holds_in_channel(&[ViewChannel, ManageChannel, RecordCalls]);
        assert!(scene.line(scene.guests, RecordCalls).held);
        assert_eq!(
            scene.line(scene.guests, RecordCalls).refused(Choice::Allow),
            None
        );
        // It denies "mute members", which the role gives in the workspace.
        assert!(!scene.line(scene.guests, MuteMembers).held);
        assert_eq!(
            scene.line(scene.guests, MuteMembers).refused(Choice::Allow),
            Some(Refused::Allows)
        );
        // The person cannot remove a deny of a role that the person holds (ADR 0304).
        assert_eq!(
            scene
                .line(scene.leads, MuteMembers)
                .refused(Choice::Default),
            Some(Refused::LiftsDeny)
        );
        // An allow of the "everyone" role counts for the person too.
        scene.load(vec![sets(scene.everyone, &[DeafenMembers], &[])]);
        scene.holds_in_channel(&[ViewChannel, ManageChannel, MuteMembers, DeafenMembers]);
        assert_eq!(
            scene
                .line(scene.guests, DeafenMembers)
                .refused(Choice::Allow),
            None
        );
    }

    #[test]
    fn the_removal_of_an_override_needs_every_permission_that_it_denies() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        scene.load(vec![
            sets(scene.guests, &[RecordCalls], &[MuteMembers, DeafenMembers]),
            sets(scene.everyone, &[LockRoom], &[]),
        ]);
        assert_eq!(scene.role(scene.guests).blocks_removal(), [DeafenMembers]);
        // An override that only allows is removed without any of its permissions.
        assert!(scene.role(scene.everyone).blocks_removal().is_empty());
        scene.lead_holds(&[ManageChannel, MuteMembers, DeafenMembers]);
        assert!(scene.role(scene.guests).blocks_removal().is_empty());
    }

    #[test]
    fn the_page_waits_for_the_overrides_and_states_a_read_that_failed() {
        let mut scene = scene();
        scene.admin.overrides = Vec::new();
        assert_eq!(scene.page().roles, Listed::Loading);
        scene.admin.overrides = vec![(scene.general, Collection::Loading)];
        assert_eq!(scene.page().roles, Listed::Loading);
        scene.admin.overrides = vec![(scene.general, Collection::Failed(LoadFailure::NotAllowed))];
        assert_eq!(scene.page().roles, Listed::Failed(LoadFailure::NotAllowed));
        // The overrides of another channel are not those of this one.
        scene.admin.overrides = vec![(ChannelId::generate(), Collection::Loaded(Vec::new()))];
        assert_eq!(scene.page().roles, Listed::Loading);
    }

    #[test]
    fn a_change_makes_every_transition_between_the_three_choices() {
        let role_id = WorkspaceRoleId::generate();
        let of = |choice: Choice| match choice {
            Choice::Allow => sets(role_id, &[ViewChannel, MuteMembers], &[LockRoom]),
            Choice::Default => sets(role_id, &[ViewChannel], &[LockRoom]),
            Choice::Deny => sets(role_id, &[ViewChannel], &[MuteMembers, LockRoom]),
        };
        for from in Choice::ALL {
            for to in Choice::ALL {
                let (allow, deny) = changed(Some(&of(from)), MuteMembers, to);
                let expected = of(to);
                assert_eq!(
                    (allow, deny),
                    (expected.allow, expected.deny),
                    "from {from:?} to {to:?}"
                );
            }
        }
    }

    #[test]
    fn a_change_of_a_role_without_an_override_makes_one_with_the_single_choice() {
        assert_eq!(
            changed(None, RecordCalls, Choice::Allow),
            (vec![RecordCalls], vec![])
        );
        assert_eq!(
            changed(None, RecordCalls, Choice::Deny),
            (vec![], vec![RecordCalls])
        );
        assert_eq!(
            changed(None, RecordCalls, Choice::Default),
            (vec![], vec![])
        );
    }

    #[test]
    fn the_last_choice_set_to_default_leaves_two_empty_sets() {
        let role_id = WorkspaceRoleId::generate();
        let allowed = sets(role_id, &[RecordCalls], &[]);
        assert_eq!(
            changed(Some(&allowed), RecordCalls, Choice::Default),
            (vec![], vec![])
        );
        let denied = sets(role_id, &[], &[RecordCalls]);
        assert_eq!(
            changed(Some(&denied), RecordCalls, Choice::Default),
            (vec![], vec![])
        );
    }

    #[test]
    fn a_change_keeps_the_sets_in_order_without_duplicates_and_apart() {
        let role_id = WorkspaceRoleId::generate();
        // Not in order, with a permission twice and with "lock room" in both sets.
        let held = sets(
            role_id,
            &[RunMeetingMode, ViewChannel, ViewChannel, LockRoom],
            &[LockRoom, MoveMembers, MoveMembers],
        );
        assert_eq!(
            changed(Some(&held), MentionEveryone, Choice::Allow),
            (
                vec![ViewChannel, MentionEveryone, RunMeetingMode],
                vec![MoveMembers, LockRoom]
            )
        );
        let ordered = |set: &[WorkspacePermission]| set.windows(2).all(|pair| pair[0] < pair[1]);
        for permission in WorkspacePermission::ALL.iter().copied() {
            for choice in Choice::ALL {
                let (allow, deny) = changed(Some(&held), permission, choice);
                assert!(
                    ordered(&allow) && ordered(&deny),
                    "{permission:?} {choice:?}"
                );
                assert!(
                    allow.iter().all(|allowed| !deny.contains(allowed)),
                    "{permission:?} {choice:?}"
                );
                assert_eq!(allow.contains(&permission), choice == Choice::Allow);
                assert_eq!(deny.contains(&permission), choice == Choice::Deny);
            }
        }
    }

    #[test]
    fn a_private_channel_denies_view_channel_to_the_everyone_role() {
        let mut scene = scene();
        let everyone = scene.everyone;
        assert_eq!(
            private_change(None, true),
            Some((vec![], vec![ViewChannel]))
        );
        // The other permissions of the override stay.
        let held = sets(everyone, &[MentionEveryone], &[RecordCalls]);
        let (allow, deny) = private_change(Some(&held), true).unwrap();
        assert_eq!(
            (&allow, &deny),
            (&vec![MentionEveryone], &vec![ViewChannel, RecordCalls])
        );
        let private = sets(everyone, &allow, &deny);
        assert_eq!(private_change(Some(&private), true), None);
        assert_eq!(
            private_change(Some(&private), false),
            Some((vec![MentionEveryone], vec![RecordCalls]))
        );
        // An open channel stays as it is, and keeps an allow of "view channel".
        assert_eq!(private_change(None, false), None);
        assert_eq!(private_change(Some(&held), false), None);
        let allowed = sets(everyone, &[ViewChannel], &[]);
        assert_eq!(private_change(Some(&allowed), false), None);
        assert_eq!(
            private_change(Some(&allowed), true),
            Some((vec![], vec![ViewChannel]))
        );
        // The last deny that leaves removes the override.
        let only = sets(everyone, &[], &[ViewChannel]);
        assert_eq!(private_change(Some(&only), false), Some((vec![], vec![])));

        // The page shows the deny at the "everyone" role.
        scene.load(vec![only]);
        let line = scene.line(everyone, ViewChannel);
        assert_eq!(line.current, Choice::Deny);
        assert!(scene.role(everyone).has_override);
        // A person who is no administrator holds nothing in a private channel, and cannot
        // open it again.
        scene.holds_in_channel(&[]);
        let line = scene.line(everyone, ViewChannel);
        assert_eq!(line.refused(Choice::Default), Some(Refused::LiftsDeny));
    }
}
