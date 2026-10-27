//! The pages "Permissions" of a channel and of a category (ADR 0301, ADR 0304, ADR 0306):
//! the rules of the roles and the people of the place, and the changes that the grant
//! ceiling and role order allow the person. A channel in a category states whether it takes
//! the rules of the category.

use junto_client_core::model::{
    Collection, ServerModel, WorkspaceModel, comparable_name, outranks,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{AccountId, CategoryId, ChannelId};
use junto_client_core::protocol::structure::{
    Category, Channel, OverrideState, PermissionRule, RuleSubject,
};

use super::Listed;

/// The most person rules of one category or channel (ADR 0306).
pub const MOST_PERSON_RULES: usize = 250;

/// What a rule states about one permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Choice {
    /// The permission is in the allow set.
    Allow,
    /// The permission is in no set: the levels before decide.
    Neutral,
    /// The permission is in the deny set.
    Deny,
}

impl Choice {
    pub const ALL: [Self; 3] = [Self::Allow, Self::Neutral, Self::Deny];
}

/// Why a choice is refused before a request (ADR 0306).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The choice adds the permission to the allow set, and the person does not hold it at
    /// the place.
    Allows,
    /// The choice removes the permission from the deny set, and the person does not hold
    /// it at the place.
    LiftsDeny,
    /// Only the server owner adds a deny of the administrator permission.
    OwnerOnly,
}

/// One permission of one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionLine<P = WorkspacePermission> {
    pub permission: P,
    /// What the rule states now.
    pub current: Choice,
    /// The person holds the permission at the place: nobody gives what they do not hold.
    pub held: bool,
    /// A deny of this permission is the server owner's alone.
    pub owner_denies: bool,
}

impl<P: Copy> PermissionLine<P> {
    /// Why the person may not make a choice now. Absent: the person may. The choice of
    /// now is not a change, and adding a deny and removing an allow give nothing.
    pub const fn refused(&self, choice: Choice) -> Option<Refused> {
        match (self.current, choice) {
            (Choice::Allow | Choice::Neutral, Choice::Deny) if self.owner_denies => {
                Some(Refused::OwnerOnly)
            }
            _ if self.held => None,
            (Choice::Neutral, Choice::Allow) => Some(Refused::Allows),
            (Choice::Deny, Choice::Neutral | Choice::Allow) => Some(Refused::LiftsDeny),
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

/// Why the rule of a subject or the permissions of a person take no change from the
/// person (ADR 0306, ADR 0309).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locked {
    /// The member stands at or above the person in the order of the roles.
    Outranked,
    /// The order of the roles is on its way, so the page cannot tell yet whether the member
    /// stands below the person.
    OrderNotRead,
    /// The server refuses a change of the own permissions of the person.
    OwnAccount,
    /// The account of the server owner takes no rule.
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectKind {
    /// The role that every member holds.
    Everyone,
    Role,
    Person,
}

/// The rule of one role or one person at a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectLine {
    pub subject: RuleSubject,
    pub name: String,
    pub kind: SubjectKind,
    /// The subject has a rule at the place now.
    pub has_rule: bool,
    pub locked: Option<Locked>,
    /// In the order of `WorkspacePermission::ALL`.
    pub permissions: Vec<PermissionLine>,
}

impl SubjectLine {
    fn count(&self, choice: Choice) -> usize {
        self.permissions
            .iter()
            .filter(|line| line.current == choice)
            .count()
    }

    /// The number of permissions that the rule allows.
    pub fn allowed(&self) -> usize {
        self.count(Choice::Allow)
    }

    /// The number of permissions that the rule denies.
    pub fn denied(&self) -> usize {
        self.count(Choice::Deny)
    }

    /// The permissions that the rule denies and the person does not hold at the place.
    /// The removal of the rule removes every deny, so the grant ceiling refuses it while
    /// this list is not empty.
    pub fn blocks_removal(&self) -> Vec<WorkspacePermission> {
        self.permissions
            .iter()
            .filter(|line| line.current == Choice::Deny && !line.held)
            .map(|line| line.permission)
            .collect()
    }
}

/// A role or a member of the workspace that has no line on the page yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub subject: RuleSubject,
    pub name: String,
}

impl Candidate {
    /// True when `typed` is a part of the name, without case and surrounding spaces.
    pub fn matches(&self, typed: &str) -> bool {
        comparable_name(&self.name).contains(&comparable_name(typed))
    }
}

/// The rules of the roles and the people at one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleList {
    /// The "everyone" role first, then the roles with a rule or that the person added, in
    /// the order of the workspace, then such people in the order of their names.
    pub subjects: Vec<SubjectLine>,
    /// The roles and the members without a line, the roles first in the order of the
    /// workspace, then the members in the order of their names.
    pub candidates: Vec<Candidate>,
    /// The members of the workspace are read: the candidates hold them.
    pub members_read: bool,
    /// The place holds the most person rules: no person is added.
    pub people_full: bool,
}

/// Whether a channel takes the rules of its category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncLine {
    pub category_id: CategoryId,
    pub category_name: String,
    pub synced: bool,
}

/// The rules of one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionsPage {
    pub rules: Listed<RuleList>,
    /// Present for a channel in a category, once its rules are read.
    pub sync: Option<SyncLine>,
}

/// How many channels of a category take its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncedCount {
    /// The rules of a channel that the person manages are on their way.
    Reading,
    /// `synced` of the `channels` of the category that the person sees.
    Counted { synced: usize, channels: usize },
}

/// The rules of one category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryPermissionsPage {
    pub rules: Listed<RuleList>,
    pub synced: SyncedCount,
}

/// What a rule states about a permission. A permission in both sets, which the server
/// does not store, counts as denied: that gives the subject less.
pub fn choice_of(current: Option<&PermissionRule>, permission: WorkspacePermission) -> Choice {
    match current {
        Some(held) if held.deny.contains(&permission) => Choice::Deny,
        Some(held) if held.allow.contains(&permission) => Choice::Allow,
        _ => Choice::Neutral,
    }
}

/// The state that the request for one permission of a rule names for a choice
/// (ADR 0304). The server changes that permission and keeps every other one.
pub const fn state_of(choice: Choice) -> OverrideState {
    match choice {
        Choice::Allow => OverrideState::Allow,
        Choice::Neutral => OverrideState::Inherit,
        Choice::Deny => OverrideState::Deny,
    }
}

/// The state of "view channel" in the rule of the "everyone" role that makes a channel
/// private or open (ADR 0304): denied, or decided by the roles. Absent when the channel is
/// private or open already; an allow of "view channel" stays in an open channel.
pub fn private_state(current: Option<&PermissionRule>, private: bool) -> Option<OverrideState> {
    let denied = choice_of(current, WorkspacePermission::ViewChannel) == Choice::Deny;
    (denied != private).then_some(if private {
        OverrideState::Deny
    } else {
        OverrideState::Inherit
    })
}

/// Whether role order lets the person change what concerns one member of a workspace:
/// the person must stand above the member (ADR 0309). Absent: the page offers the change
/// and the server decides it. `own` states whether the own account takes a change.
pub fn member_lock(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    account_id: AccountId,
    own: bool,
) -> Option<Locked> {
    if account_id == model.account.id {
        return (!own).then_some(Locked::OwnAccount);
    }
    if model.account.is_owner {
        return None;
    }
    let workspace_id = workspace.workspace.id;
    let failed = matches!(model.admin.server_roles, Collection::Failed(_))
        || matches!(model.admin.accounts, Collection::Failed(_));
    let not_read = if failed {
        None
    } else {
        Some(Locked::OrderNotRead)
    };
    let Some(role_ids) = workspace.members.loaded().and_then(|members| {
        members
            .iter()
            .find(|member| member.account_id == account_id)
            .map(|member| member.role_ids.clone())
    }) else {
        return not_read;
    };
    let Some(person) = model.workspace_rank(workspace_id) else {
        return not_read;
    };
    // An administrator reads the accounts, and with them the server roles of the member;
    // every other person compares the roles of the workspace, which the server roles of
    // the member can only raise.
    if model.is_administrator() {
        let account = model
            .admin
            .accounts
            .loaded()
            .and_then(|accounts| accounts.iter().find(|account| account.id == account_id));
        let target =
            account.and_then(|account| model.workspace_rank_of(workspace_id, account, &role_ids));
        return match target {
            Some(target) if outranks(person, target) => None,
            Some(_) => Some(Locked::Outranked),
            None => not_read,
        };
    }
    match model.workspace_rank_by_roles(workspace_id, &role_ids) {
        Some(target) if !outranks(person, target) => Some(Locked::Outranked),
        Some(_) => None,
        None => not_read,
    }
}

/// The name of a member of the workspace, while the members are read.
fn member_name(workspace: &WorkspaceModel, account_id: AccountId) -> Option<String> {
    workspace
        .members
        .loaded()?
        .iter()
        .find(|member| member.account_id == account_id)
        .map(|member| member.name.clone())
}

/// The rules of a place as the page lists them. `held` are the permissions of the person
/// at the place, `added` the subjects that the person added and that have no rule yet.
pub fn rule_list(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    rules: &[PermissionRule],
    held: &[WorkspacePermission],
    added: &[RuleSubject],
) -> RuleList {
    let current = |subject: RuleSubject| rules.iter().find(|rule| rule.subject == subject);
    let shown = |subject: RuleSubject| current(subject).is_some() || added.contains(&subject);
    let line = |subject: RuleSubject, name: String, kind: SubjectKind, locked| {
        let rule = current(subject);
        SubjectLine {
            subject,
            name,
            kind,
            has_rule: rule.is_some_and(|of| !of.allow.is_empty() || !of.deny.is_empty()),
            locked,
            permissions: WorkspacePermission::ALL
                .iter()
                .copied()
                .map(|permission| PermissionLine {
                    permission,
                    current: choice_of(rule, permission),
                    held: held.contains(&permission),
                    owner_denies: false,
                })
                .collect(),
        }
    };
    let mut subjects = Vec::new();
    let mut candidates = Vec::new();
    let everyone = workspace.roles.iter().filter(|role| role.is_everyone);
    let others = workspace.roles.iter().filter(|role| !role.is_everyone);
    for role in everyone.chain(others) {
        let subject = RuleSubject::Role { role_id: role.id };
        if role.is_everyone || shown(subject) {
            let kind = if role.is_everyone {
                SubjectKind::Everyone
            } else {
                SubjectKind::Role
            };
            subjects.push(line(subject, role.name.clone(), kind, None));
        } else {
            candidates.push(Candidate {
                subject,
                name: role.name.clone(),
            });
        }
    }
    let mut people: Vec<SubjectLine> = rules
        .iter()
        .map(|rule| rule.subject)
        .chain(added.iter().copied())
        .filter_map(|subject| match subject {
            RuleSubject::Person { account_id } => Some((subject, account_id)),
            RuleSubject::Role { .. } => None,
        })
        .fold(Vec::new(), |mut people, held| {
            if !people.contains(&held) {
                people.push(held);
            }
            people
        })
        .into_iter()
        .map(|(subject, account_id)| {
            let name = member_name(workspace, account_id).unwrap_or_else(|| {
                if workspace.members.loaded().is_some() {
                    crate::text::admin::overrides::FORMER_MEMBER.to_owned()
                } else {
                    String::new()
                }
            });
            let locked = member_lock(model, workspace, account_id, true);
            line(subject, name, SubjectKind::Person, locked)
        })
        .collect();
    people.sort_by(|one, other| {
        comparable_name(&one.name)
            .cmp(&comparable_name(&other.name))
            .then(one.subject.cmp(&other.subject))
    });
    subjects.extend(people);
    let members_read = workspace.members.loaded().is_some();
    if let Some(members) = workspace.members.loaded() {
        let mut listed: Vec<Candidate> = members
            .iter()
            .map(|member| Candidate {
                subject: RuleSubject::Person {
                    account_id: member.account_id,
                },
                name: member.name.clone(),
            })
            .filter(|candidate| !shown(candidate.subject))
            .collect();
        listed.sort_by(|one, other| {
            comparable_name(&one.name)
                .cmp(&comparable_name(&other.name))
                .then(one.subject.cmp(&other.subject))
        });
        candidates.extend(listed);
    }
    let person_rules = rules
        .iter()
        .filter(|rule| matches!(rule.subject, RuleSubject::Person { .. }))
        .count();
    RuleList {
        subjects,
        candidates,
        members_read,
        people_full: person_rules >= MOST_PERSON_RULES,
    }
}

/// The page of the rules of a channel. The permissions that the person holds in the
/// channel, as the server stated them, decide which choices the person may make; the server
/// decides every request.
pub fn derive(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    channel: &Channel,
    added: &[RuleSubject],
) -> PermissionsPage {
    let held = model.permissions_in(channel.id).unwrap_or_default();
    let collection = model.admin.rules(channel.id);
    let rules = Listed::of(collection, |rules| {
        rule_list(model, workspace, &rules.rules, held, added)
    });
    // The category of the channel is the one of the state: an event moves a channel at
    // once, and its rules are read again when the move changed them.
    let sync = channel
        .category_id
        .and_then(|category_id| category_of(workspace, category_id))
        .zip(collection.loaded())
        .map(|(category, rules)| SyncLine {
            category_id: category.id,
            category_name: category.name.clone(),
            synced: rules.synced && rules.category_id == Some(category.id),
        });
    PermissionsPage { rules, sync }
}

fn category_of(workspace: &WorkspaceModel, category_id: CategoryId) -> Option<&Category> {
    workspace
        .categories
        .iter()
        .find(|category| category.id == category_id)
}

/// The channels of a category whose rules the page reads to count the synced ones: those
/// in which the person holds "manage channel". A synced channel uses the rules of its
/// category, so a person who manages the category manages each synced channel too; a
/// channel that the person does not manage is not synced.
pub fn counted_channels(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    category_id: CategoryId,
) -> Vec<ChannelId> {
    workspace
        .channels
        .iter()
        .filter(|held| held.channel.category_id == Some(category_id))
        .filter(|held| model.holds_in(held.channel.id, WorkspacePermission::ManageChannel))
        .map(|held| held.channel.id)
        .collect()
}

/// The page of the rules of a category. The server states no list of the permissions of
/// the person in a category: they are evaluated from the rules of the category with the
/// permission function (ADR 0790).
pub fn category_page(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    category_id: CategoryId,
    added: &[RuleSubject],
) -> CategoryPermissionsPage {
    let workspace_id = workspace.workspace.id;
    let rules = Listed::of(
        model.admin.category_rules(workspace_id, category_id),
        |rules| {
            let held = model
                .permissions_with_rules(workspace_id, &rules.rules)
                .unwrap_or_default();
            rule_list(model, workspace, &rules.rules, &held, added)
        },
    );
    let channels = workspace
        .channels
        .iter()
        .filter(|held| held.channel.category_id == Some(category_id))
        .count();
    let mut synced = 0;
    let mut reading = false;
    for channel_id in counted_channels(model, workspace, category_id) {
        match model.admin.rules(channel_id) {
            Collection::Loaded(rules) => {
                if rules.synced && rules.category_id == Some(category_id) {
                    synced += 1;
                }
            }
            // A channel whose rules the person may not read is not counted.
            Collection::Failed(_) => {}
            Collection::NotLoaded | Collection::Loading => reading = true,
        }
    }
    CategoryPermissionsPage {
        rules,
        synced: if reading {
            SyncedCount::Reading
        } else {
            SyncedCount::Counted { synced, channels }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use junto_client_core::model::{AdminModel, Collection, LoadFailure, Loadable};
    use junto_client_core::protocol::ServerPermission;
    use junto_client_core::protocol::WorkspacePermission::{
        DeafenMembers, ManageChannel, MentionEveryone, MuteMembers, RecordCalls, SendMessages,
        ViewChannel,
    };
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{StreamEpoch, WorkspaceId, WorkspaceRoleId};
    use junto_client_core::protocol::roles::WorkspaceRole;
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{
        CategoryRules, ChannelKind, ChannelRules, Workspace, WorkspaceMember,
    };

    use super::*;

    /// One server with the workspace "Studio", its category "Teams" and the channel
    /// "general" in it. The workspace holds the roles "leads", "everyone" and "guests". The
    /// "everyone" role gives "view channel". The account Luca holds the role "leads",
    /// which gives "manage channel". Bruno is a member without a role, Cara holds "leads".
    struct Scene {
        studio: WorkspaceId,
        teams: CategoryId,
        everyone: WorkspaceRoleId,
        leads: WorkspaceRoleId,
        guests: WorkspaceRoleId,
        general: ChannelId,
        bruno: AccountId,
        cara: AccountId,
        snapshot: Snapshot,
        admin: AdminModel,
        members: Vec<WorkspaceMember>,
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
        let teams = Category {
            id: CategoryId::generate(),
            workspace_id: studio,
            name: "Teams".to_owned(),
            position: 0,
        };
        let general = Channel {
            id: ChannelId::generate(),
            workspace_id: studio,
            category_id: Some(teams.id),
            kind: ChannelKind::Text,
            name: "general".to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        };
        let luca = AccountId::generate();
        let (bruno, cara) = (AccountId::generate(), AccountId::generate());
        let member = |account_id, name: &str, role_ids: Vec<WorkspaceRoleId>| WorkspaceMember {
            avatar: None,
            account_id,
            name: name.to_owned(),
            role_ids,
        };
        let members = vec![
            member(luca, "Luca", vec![leads.id]),
            member(bruno, "Bruno", Vec::new()),
            member(cara, "Cara", vec![leads.id]),
        ];
        let snapshot = Snapshot {
            cursor: Cursor {
                epoch: StreamEpoch::generate(),
                position: 1,
            },
            account: OwnAccount {
                id: luca,
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
                categories: vec![teams.clone()],
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
            teams: teams.id,
            everyone: everyone.id,
            leads: leads.id,
            guests: guests.id,
            general: general.id,
            bruno,
            cara,
            snapshot,
            admin: AdminModel::default(),
            members,
        };
        scene.load(Vec::new(), false);
        scene
    }

    fn rule(
        subject: RuleSubject,
        allow: &[WorkspacePermission],
        deny: &[WorkspacePermission],
    ) -> PermissionRule {
        PermissionRule {
            subject,
            allow: allow.to_vec(),
            deny: deny.to_vec(),
        }
    }

    impl Scene {
        fn role(&self, role_id: WorkspaceRoleId) -> RuleSubject {
            RuleSubject::Role { role_id }
        }

        fn person(&self, account_id: AccountId) -> RuleSubject {
            RuleSubject::Person { account_id }
        }

        /// Gives the role of the account these permissions in the workspace.
        fn lead_holds(&mut self, permissions: &[WorkspacePermission]) {
            let leads = self.leads;
            for role in &mut self.snapshot.workspaces[0].roles {
                if role.id == leads {
                    role.permissions = permissions.to_vec();
                }
            }
        }

        /// The rules of the channel are read and hold these.
        fn load(&mut self, rules: Vec<PermissionRule>, synced: bool) {
            self.admin.rules = vec![(
                self.general,
                Collection::Loaded(ChannelRules {
                    channel_id: self.general,
                    category_id: Some(self.teams),
                    synced,
                    rules,
                }),
            )];
        }

        fn model(&self) -> ServerModel {
            let mut model = ServerModel::from_snapshot(crate::state::fixtures::from_roles(
                self.snapshot.clone(),
            ));
            model.admin = Arc::new(self.admin.clone());
            let members = self.members.clone();
            if let Some(workspace) = model
                .workspaces
                .iter_mut()
                .find(|held| held.workspace.id == self.studio)
            {
                Arc::make_mut(workspace).members = Loadable::Loaded(members);
            }
            model
        }

        fn page(&self, added: &[RuleSubject]) -> PermissionsPage {
            let model = self.model();
            let workspace = model.workspace(self.studio).unwrap();
            let channel = model.readable_channel(self.general).unwrap();
            derive(&model, workspace, channel, added)
        }

        fn list(&self, added: &[RuleSubject]) -> RuleList {
            match self.page(added).rules {
                Listed::Loaded(list) => list,
                other => panic!("the rules are loaded, not {other:?}"),
            }
        }

        fn subject(&self, subject: RuleSubject) -> SubjectLine {
            self.list(&[subject])
                .subjects
                .into_iter()
                .find(|line| line.subject == subject)
                .unwrap()
        }

        fn line(&self, subject: RuleSubject, permission: WorkspacePermission) -> PermissionLine {
            self.subject(subject)
                .permissions
                .into_iter()
                .find(|line| line.permission == permission)
                .unwrap()
        }
    }

    #[test]
    fn a_rule_of_a_person_who_left_stays_listed_and_names_the_person_as_a_former_member() {
        let mut scene = scene();
        let former = AccountId::generate();
        scene.load(
            vec![rule(scene.person(former), &[], &[SendMessages])],
            false,
        );
        let line = scene.subject(scene.person(former));
        assert_eq!(line.name, crate::text::admin::overrides::FORMER_MEMBER);
        assert_eq!(line.kind, SubjectKind::Person);
    }

    #[test]
    fn the_page_lists_the_everyone_role_and_the_subjects_with_a_rule_or_that_were_added() {
        let mut scene = scene();
        let listed = |list: &RuleList| {
            list.subjects
                .iter()
                .map(|line| line.subject)
                .collect::<Vec<_>>()
        };
        // Without a rule only the "everyone" role is listed; the other roles and the
        // members are candidates, the roles first.
        let list = scene.list(&[]);
        assert_eq!(listed(&list), [scene.role(scene.everyone)]);
        assert_eq!(list.subjects[0].kind, SubjectKind::Everyone);
        assert_eq!(
            list.candidates
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["leads", "guests", "Bruno", "Cara", "Luca"]
        );
        // A rule of a role and of a person lists them: roles in the order of the workspace,
        // then people by name.
        scene.load(
            vec![
                rule(scene.person(scene.cara), &[], &[SendMessages]),
                rule(scene.role(scene.guests), &[MentionEveryone], &[]),
            ],
            false,
        );
        let list = scene.list(&[scene.person(scene.bruno), scene.role(scene.leads)]);
        assert_eq!(
            listed(&list),
            [
                scene.role(scene.everyone),
                scene.role(scene.leads),
                scene.role(scene.guests),
                scene.person(scene.bruno),
                scene.person(scene.cara),
            ]
        );
        let names: Vec<(&str, SubjectKind, bool)> = list
            .subjects
            .iter()
            .map(|line| (line.name.as_str(), line.kind, line.has_rule))
            .collect();
        assert_eq!(
            names,
            [
                ("everyone", SubjectKind::Everyone, false),
                ("leads", SubjectKind::Role, false),
                ("guests", SubjectKind::Role, true),
                ("Bruno", SubjectKind::Person, false),
                ("Cara", SubjectKind::Person, true),
            ]
        );
        assert_eq!(
            list.candidates
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["Luca"]
        );
        assert!(list.candidates[0].matches(" lu"));
        assert!(!list.candidates[0].matches("bru"));
    }

    #[test]
    fn each_subject_lists_every_permission_in_the_order_of_the_protocol() {
        let scene = scene();
        for line in scene.list(&[scene.person(scene.bruno)]).subjects {
            assert_eq!(
                line.permissions
                    .iter()
                    .map(|permission| permission.permission)
                    .collect::<Vec<_>>(),
                WorkspacePermission::ALL
            );
        }
    }

    #[test]
    fn a_permission_shows_the_choice_that_the_rule_holds() {
        let mut scene = scene();
        let bruno = scene.person(scene.bruno);
        scene.load(
            vec![rule(bruno, &[MentionEveryone, RecordCalls], &[ViewChannel])],
            false,
        );
        assert_eq!(scene.line(bruno, MentionEveryone).current, Choice::Allow);
        assert_eq!(scene.line(bruno, ViewChannel).current, Choice::Deny);
        assert_eq!(scene.line(bruno, MuteMembers).current, Choice::Neutral);
        let line = scene.subject(bruno);
        assert_eq!((line.allowed(), line.denied()), (2, 1));
    }

    #[test]
    fn nobody_gives_in_a_channel_what_they_do_not_hold_there() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, MuteMembers]);
        let bruno = scene.person(scene.bruno);
        let held = scene.line(bruno, MuteMembers);
        assert_eq!((held.current, held.held), (Choice::Neutral, true));
        assert_eq!(held.refused(Choice::Allow), None);
        let missing = scene.line(bruno, DeafenMembers);
        assert_eq!(missing.refused(Choice::Allow), Some(Refused::Allows));
        // A deny needs only "manage channel"; removing it needs the permission.
        assert_eq!(missing.refused(Choice::Deny), None);
        scene.load(vec![rule(bruno, &[], &[MuteMembers, DeafenMembers])], false);
        assert_eq!(
            scene.line(bruno, MuteMembers).refused(Choice::Neutral),
            None
        );
        assert_eq!(
            scene.line(bruno, DeafenMembers).refused(Choice::Neutral),
            Some(Refused::LiftsDeny)
        );
        assert_eq!(scene.subject(bruno).blocks_removal(), [DeafenMembers]);
    }

    #[test]
    fn an_administrator_and_the_server_owner_may_make_every_choice() {
        let mut scene = scene();
        scene.lead_holds(&[]);
        scene.load(
            vec![rule(scene.role(scene.everyone), &[], &[ViewChannel])],
            false,
        );
        let free = |scene: &Scene| {
            scene
                .list(&[scene.person(scene.bruno)])
                .subjects
                .iter()
                .all(|line| {
                    line.permissions
                        .iter()
                        .all(|permission| permission.reason().is_none())
                })
        };
        assert!(!free(&scene));
        scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
        assert!(free(&scene));
        scene.snapshot.account.server_permissions = Vec::new();
        scene.snapshot.account.is_owner = true;
        assert!(free(&scene));
    }

    #[test]
    fn a_person_rule_of_a_member_who_stands_as_high_as_the_person_is_locked() {
        let scene = scene();
        // Cara holds the role of Luca: she does not stand below him.
        assert_eq!(
            scene.subject(scene.person(scene.cara)).locked,
            Some(Locked::Outranked)
        );
        assert_eq!(scene.subject(scene.person(scene.bruno)).locked, None);
        // The own rule of the person takes a change.
        let luca = scene.snapshot.account.id;
        assert_eq!(scene.subject(scene.person(luca)).locked, None);
        let model = scene.model();
        let workspace = model.workspace(scene.studio).unwrap();
        assert_eq!(
            member_lock(&model, workspace, luca, false),
            Some(Locked::OwnAccount)
        );
    }

    #[test]
    fn a_channel_of_a_category_states_whether_it_is_synced() {
        let mut scene = scene();
        let sync = scene.page(&[]).sync.unwrap();
        assert_eq!((sync.category_name.as_str(), sync.synced), ("Teams", false));
        scene.load(Vec::new(), true);
        assert!(scene.page(&[]).sync.unwrap().synced);
        // A channel without a category states nothing.
        scene.snapshot.workspaces[0].channels[0].category_id = None;
        assert_eq!(scene.page(&[]).sync, None);
        // Nothing is stated while the rules are read.
        let mut scene = self::scene();
        scene.admin.rules = vec![(scene.general, Collection::Loading)];
        assert_eq!(scene.page(&[]).sync, None);
        assert_eq!(scene.page(&[]).rules, Listed::Loading);
        scene.admin.rules = vec![(scene.general, Collection::Failed(LoadFailure::NotAllowed))];
        assert_eq!(
            scene.page(&[]).rules,
            Listed::Failed(LoadFailure::NotAllowed)
        );
    }

    #[test]
    fn the_permissions_of_the_person_in_a_category_follow_its_rules() {
        let mut scene = scene();
        scene.lead_holds(&[ManageChannel, SendMessages]);
        let category_rules = |rules| CategoryRules {
            category_id: scene.teams,
            rules,
        };
        let leads = scene.role(scene.leads);
        let everyone = scene.role(scene.everyone);
        scene.admin.category_rules = vec![(
            (scene.studio, scene.teams),
            Collection::Loaded(category_rules(vec![
                rule(everyone, &[RecordCalls], &[]),
                rule(leads, &[], &[SendMessages]),
            ])),
        )];
        let model = scene.model();
        let workspace = model.workspace(scene.studio).unwrap();
        let page = category_page(&model, workspace, scene.teams, &[]);
        let list = page.rules.loaded().unwrap();
        let line = |permission| {
            list.subjects[0]
                .permissions
                .iter()
                .find(|line| line.permission == permission)
                .copied()
                .unwrap()
        };
        // The allow of the "everyone" role gives the person "record calls" there; the deny
        // of the role of the person takes "send messages".
        assert!(line(RecordCalls).held);
        assert!(!line(SendMessages).held);
        assert!(line(ManageChannel).held);
        // The channel has its own rules.
        assert_eq!(
            page.synced,
            SyncedCount::Counted {
                synced: 0,
                channels: 1
            }
        );
    }

    #[test]
    fn the_count_of_the_synced_channels_reads_the_channels_that_the_person_manages() {
        let mut scene = scene();
        scene.load(Vec::new(), true);
        scene.admin.category_rules = vec![(
            (scene.studio, scene.teams),
            Collection::Loaded(CategoryRules {
                category_id: scene.teams,
                rules: Vec::new(),
            }),
        )];
        let model = scene.model();
        let workspace = model.workspace(scene.studio).unwrap();
        assert_eq!(
            counted_channels(&model, workspace, scene.teams),
            [scene.general]
        );
        let page = category_page(&model, workspace, scene.teams, &[]);
        assert_eq!(
            page.synced,
            SyncedCount::Counted {
                synced: 1,
                channels: 1
            }
        );
        scene.load(Vec::new(), false);
        let model = scene.model();
        let workspace = model.workspace(scene.studio).unwrap();
        let page = category_page(&model, workspace, scene.teams, &[]);
        assert_eq!(
            page.synced,
            SyncedCount::Counted {
                synced: 0,
                channels: 1
            }
        );
    }

    #[test]
    fn a_choice_names_the_state_of_its_one_permission() {
        assert_eq!(state_of(Choice::Allow), OverrideState::Allow);
        assert_eq!(state_of(Choice::Deny), OverrideState::Deny);
        assert_eq!(state_of(Choice::Neutral), OverrideState::Inherit);
    }

    #[test]
    fn only_the_owner_adds_a_deny_of_the_administrator_permission() {
        let line = PermissionLine {
            permission: ServerPermission::Administrator,
            current: Choice::Neutral,
            held: true,
            owner_denies: true,
        };
        assert_eq!(line.refused(Choice::Deny), Some(Refused::OwnerOnly));
        assert_eq!(line.refused(Choice::Allow), None);
        // A deny that the owner set can be lifted.
        let denied = PermissionLine {
            current: Choice::Deny,
            ..line
        };
        assert_eq!(denied.refused(Choice::Neutral), None);
        assert_eq!(denied.refused(Choice::Deny), None);
    }

    #[test]
    fn a_private_channel_denies_view_channel_to_the_everyone_role() {
        let scene = scene();
        let everyone = scene.role(scene.everyone);
        assert_eq!(private_state(None, true), Some(OverrideState::Deny));
        let private = rule(everyone, &[MentionEveryone], &[ViewChannel, RecordCalls]);
        assert_eq!(private_state(Some(&private), true), None);
        assert_eq!(
            private_state(Some(&private), false),
            Some(OverrideState::Inherit)
        );
        assert_eq!(private_state(None, false), None);
        let allowed = rule(everyone, &[ViewChannel], &[]);
        assert_eq!(private_state(Some(&allowed), false), None);
    }
}
