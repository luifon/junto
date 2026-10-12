//! The pages of the people of a server (ADR 0761): every account, the members of the
//! shown workspace, the invites and the join requests that wait.

use junto_client_core::model::{Collection, Loadable, ServerModel, WorkspaceModel};
use junto_client_core::protocol::account::{AccountStatus, AccountSummary};
use junto_client_core::protocol::ids::{
    AccountId, InviteId, JoinRequestId, ServerId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::invite::{Invite, InviteRevocation};
use junto_client_core::protocol::roles::ServerRole;
use junto_client_core::protocol::secret::InviteCode;
use time::{Duration, OffsetDateTime};

use super::Listed;
use crate::state::admin::{AdminState, Made};
use crate::state::view::Known;

/// The lifetimes of a new invite that the form offers, in days.
pub const EXPIRY_DAYS: [u32; 4] = [1, 7, 30, 365];
/// The lifetime of an invite that names no expiry (ADR 0111).
pub const DEFAULT_EXPIRY_DAYS: u32 = 7;
/// The longest lifetime of an invite (ADR 0111).
pub const LONGEST_EXPIRY_DAYS: u32 = 365;
/// The numbers of uses of a new invite that the form offers.
pub const USES: [u32; 4] = [1, 5, 25, 100];
/// The number of uses of an invite that names none (ADR 0111).
pub const DEFAULT_USES: u32 = 1;
/// The largest number of uses of an invite (ADR 0111).
pub const MOST_USES: u32 = 10_000;
/// The most workspaces that an invite or an approval names (ADR 0306).
pub const MOST_WORKSPACES: usize = 100;

/// The server refuses an expiry that is more than the longest lifetime after its own
/// clock. The longest lifetime is shortened by this time, so that it stays valid when the
/// clock of this device is ahead of the clock of the server.
const CLOCK_MARGIN: Duration = Duration::minutes(10);

/// Why a removal, a ban or an unban of an account is not possible (ADR 0113).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// The server owner is not removed or banned (`owner_protected`).
    Owner,
    /// An account does not remove or ban itself (`own_account_target`).
    OwnAccount,
    /// A removed account takes no removal, ban or unban.
    Removed,
    /// A ban needs an active account, and this one is banned.
    Banned,
    /// An unban needs a banned account.
    NotBanned,
}

/// The server roles of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleNames {
    /// The names, in the order of the roles of the server.
    Named(Vec<String>),
    /// The server roles are not loaded: the number of roles of the account.
    Count(usize),
}

/// One account of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLine {
    pub id: AccountId,
    pub name: String,
    pub status: AccountStatus,
    pub is_owner: bool,
    /// The account of the person.
    pub is_self: bool,
    pub roles: RoleNames,
    /// When the account was created, in the time zone of the person.
    pub created: OffsetDateTime,
    pub remove: Result<(), Blocked>,
    pub ban: Result<(), Blocked>,
    pub unban: Result<(), Blocked>,
}

impl AccountLine {
    fn of(
        account: &AccountSummary,
        own: AccountId,
        roles: Option<&[ServerRole]>,
        known: &Known,
    ) -> Self {
        // The server names the owner first when the owner asks about the own account.
        let target = if account.is_owner {
            Err(Blocked::Owner)
        } else if account.id == own {
            Err(Blocked::OwnAccount)
        } else {
            Ok(())
        };
        let (remove, ban, unban) = match account.status {
            AccountStatus::Active => (Ok(()), Ok(()), Err(Blocked::NotBanned)),
            AccountStatus::Banned => (Ok(()), Err(Blocked::Banned), Ok(())),
            AccountStatus::Removed => (
                Err(Blocked::Removed),
                Err(Blocked::Removed),
                Err(Blocked::Removed),
            ),
        };
        Self {
            id: account.id,
            name: account.name.clone(),
            status: account.status,
            is_owner: account.is_owner,
            is_self: account.id == own,
            roles: match roles {
                Some(roles) => RoleNames::Named(
                    roles
                        .iter()
                        .filter(|role| account.server_role_ids.contains(&role.id))
                        .map(|role| role.name.clone())
                        .collect(),
                ),
                None => RoleNames::Count(account.server_role_ids.len()),
            },
            created: known.zone.local(account.created_at),
            remove: target.and(remove),
            ban: target.and(ban),
            unban: target.and(unban),
        }
    }
}

/// Every account of the server, for a person with "ban members".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeoplePage {
    /// In the order of their names. The page shows all of them or the banned ones.
    pub accounts: Listed<Vec<AccountLine>>,
    /// The number of banned accounts.
    pub banned: usize,
}

/// One member of the shown workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberLine {
    pub account_id: AccountId,
    pub name: String,
    /// The account of the person.
    pub is_self: bool,
    /// The names of the roles of the member in the workspace, without the "everyone"
    /// role.
    pub roles: Vec<String>,
}

/// The members of a workspace as the page shows them. The window reads them with the
/// shown workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberList {
    Loading,
    Failed,
    /// In the order of their names.
    Loaded(Vec<MemberLine>),
}

/// An active account of the server that is not a member of the shown workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddableLine {
    pub account_id: AccountId,
    pub name: String,
}

/// The members of the shown workspace with their roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembersPage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub members: MemberList,
    /// Present for an administrator: the active accounts that can be added to the
    /// workspace (ADR 0115), in the order of their names. They are on their way until the
    /// accounts and the members are read.
    pub addable: Option<Listed<Vec<AddableLine>>>,
}

/// A workspace that an invite or an approval can grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceChoice {
    pub id: WorkspaceId,
    pub name: String,
    /// The workspace that a request without a workspace grants.
    pub is_default: bool,
}

/// Why an invite or an approval cannot be sent with the chosen workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantFault {
    /// No workspace is chosen, and the person may not grant the default workspace.
    NoneChosen,
    /// More workspaces are chosen than one request names (ADR 0306).
    TooMany,
}

/// The workspaces that the person may grant with an invite or an approval: every
/// workspace for an administrator, and the workspaces of the account for every other
/// person (ADR 0111, ADR 0304).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// In the order of the server.
    pub workspaces: Vec<WorkspaceChoice>,
}

impl Grant {
    fn of(model: &ServerModel) -> Self {
        let administrator = model.is_administrator();
        Self {
            workspaces: model
                .workspaces
                .iter()
                .filter(|held| {
                    administrator || model.account.workspace_ids.contains(&held.workspace.id)
                })
                .map(|held| WorkspaceChoice {
                    id: held.workspace.id,
                    name: held.workspace.name.clone(),
                    is_default: held.workspace.is_default,
                })
                .collect(),
        }
    }

    /// The default workspace, when the person may grant it. A request that names no
    /// workspace grants it.
    pub fn default_workspace(&self) -> Option<&WorkspaceChoice> {
        self.workspaces.iter().find(|choice| choice.is_default)
    }

    /// The chosen workspaces that the person may still grant, in the order of the
    /// server.
    pub fn kept(&self, chosen: &[WorkspaceId]) -> Vec<WorkspaceId> {
        self.workspaces
            .iter()
            .filter(|choice| chosen.contains(&choice.id))
            .map(|choice| choice.id)
            .collect()
    }

    /// Why a request that names `chosen` workspaces cannot be sent.
    pub fn fault(&self, chosen: usize) -> Option<GrantFault> {
        if chosen > MOST_WORKSPACES {
            Some(GrantFault::TooMany)
        } else if chosen == 0 && self.default_workspace().is_none() {
            Some(GrantFault::NoneChosen)
        } else {
            None
        }
    }
}

/// Whether an invite code works. The order is the one in which the server refuses a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteState {
    Usable,
    Revoked,
    /// Revoked by the ban of the account that created it (ADR 0106).
    CreatorBanned,
    /// The day of the expiry is before today.
    Expired,
    UsedUp,
}

/// One invite of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLine {
    pub id: InviteId,
    pub created_by: AccountId,
    /// The name of the account that created the invite. Absent: the state does not hold
    /// it.
    pub creator: Option<String>,
    /// In the time zone of the person.
    pub created: OffsetDateTime,
    pub expires: OffsetDateTime,
    pub uses: u32,
    pub max_uses: u32,
    /// The names of the workspaces of the invite that the state holds.
    pub workspaces: Vec<String>,
    /// The number of workspaces of the invite that the state does not hold.
    pub other_workspaces: usize,
    pub state: InviteState,
    pub may_revoke: bool,
}

/// The code of a new invite. It is shown until the person puts it away, and no answer of
/// the server carries it again (ADR 0111).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCode {
    pub invite_id: InviteId,
    pub code: InviteCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitesPage {
    /// The newest first.
    pub invites: Listed<Vec<InviteLine>>,
    /// The workspaces that a new invite can grant.
    pub grant: Grant,
    pub made: Option<NewCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine {
    pub id: JoinRequestId,
    pub name: String,
    /// In the time zone of the person.
    pub created: OffsetDateTime,
    pub expires: OffsetDateTime,
}

/// The join requests that wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestsPage {
    /// The oldest first.
    pub requests: Listed<Vec<RequestLine>>,
    /// The workspaces that an approval can grant.
    pub grant: Grant,
}

/// The expiry that a new invite with a lifetime of `days` asks for at the moment `now`.
/// Absent for the default lifetime: the server sets it (ADR 0111).
pub fn expiry(days: u32, now: OffsetDateTime) -> Option<OffsetDateTime> {
    if days == DEFAULT_EXPIRY_DAYS {
        return None;
    }
    let days = days.clamp(1, LONGEST_EXPIRY_DAYS);
    let ahead = Duration::days(i64::from(days));
    Some(if days == LONGEST_EXPIRY_DAYS {
        now + ahead - CLOCK_MARGIN
    } else {
        now + ahead
    })
}

/// The number of uses that a person typed, when it is one that an invite can have.
pub fn typed_uses(text: &str) -> Option<u32> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|uses| (1..=MOST_USES).contains(uses))
}

fn by_name(one: (&str, AccountId), other: (&str, AccountId)) -> std::cmp::Ordering {
    one.0
        .to_lowercase()
        .cmp(&other.0.to_lowercase())
        .then(one.1.cmp(&other.1))
}

/// The name of an account, when the state holds it: the own account, the loaded accounts,
/// the loaded members of a workspace, and the names that were read by themselves.
fn account_name(
    server_id: ServerId,
    account_id: AccountId,
    model: &ServerModel,
    known: &Known,
) -> Option<String> {
    if account_id == model.account.id {
        return Some(model.account.name.clone());
    }
    model
        .admin
        .accounts
        .loaded()
        .and_then(|accounts| accounts.iter().find(|held| held.id == account_id))
        .map(|held| held.name.clone())
        .or_else(|| {
            model
                .workspaces
                .iter()
                .filter_map(|workspace| workspace.members.loaded())
                .flatten()
                .find(|member| member.account_id == account_id)
                .map(|member| member.name.clone())
        })
        .or_else(|| known.names.get(&(server_id, account_id)).cloned())
}

fn invite_state(invite: &Invite, known: &Known) -> InviteState {
    match invite.revocation {
        Some(InviteRevocation::Revoked) => InviteState::Revoked,
        Some(InviteRevocation::CreatorBanned) => InviteState::CreatorBanned,
        None => {
            // The derivation holds the date of today and no time. An invite that expires
            // today counts as usable until the next day.
            let expired = known
                .today
                .is_some_and(|today| known.zone.local(invite.expires_at).date() < today);
            if expired {
                InviteState::Expired
            } else if invite.uses >= invite.max_uses {
                InviteState::UsedUp
            } else {
                InviteState::Usable
            }
        }
    }
}

fn invite_line(
    server_id: ServerId,
    invite: &Invite,
    model: &ServerModel,
    known: &Known,
) -> InviteLine {
    let workspaces: Vec<String> = invite
        .workspace_ids
        .iter()
        .filter_map(|id| model.workspace(*id))
        .map(|workspace| workspace.workspace.name.clone())
        .collect();
    let state = invite_state(invite, known);
    InviteLine {
        id: invite.id,
        created_by: invite.created_by,
        creator: account_name(server_id, invite.created_by, model, known),
        created: known.zone.local(invite.created_at),
        expires: known.zone.local(invite.expires_at),
        uses: invite.uses,
        max_uses: invite.max_uses,
        other_workspaces: invite.workspace_ids.len() - workspaces.len(),
        workspaces,
        state,
        may_revoke: state == InviteState::Usable,
    }
}

/// The page of every account of the server.
pub fn people(model: &ServerModel, known: &Known) -> PeoplePage {
    let own = model.account.id;
    let roles = model.admin.server_roles.loaded().map(Vec::as_slice);
    let accounts = Listed::of(&model.admin.accounts, |accounts| {
        let mut rows: Vec<AccountLine> = accounts
            .iter()
            .map(|account| AccountLine::of(account, own, roles, known))
            .collect();
        rows.sort_by(|one, other| {
            by_name((one.name.as_str(), one.id), (other.name.as_str(), other.id))
        });
        rows
    });
    let banned = accounts.loaded().map_or(0, |rows| {
        rows.iter()
            .filter(|row| row.status == AccountStatus::Banned)
            .count()
    });
    PeoplePage { accounts, banned }
}

/// The page of the members of the shown workspace.
pub fn members(
    server_id: ServerId,
    model: &ServerModel,
    workspace: &WorkspaceModel,
    known: &Known,
) -> MembersPage {
    let workspace_id = workspace.workspace.id;
    let role_names = |role_ids: &[WorkspaceRoleId]| -> Vec<String> {
        workspace
            .roles
            .iter()
            .filter(|role| !role.is_everyone && role_ids.contains(&role.id))
            .map(|role| role.name.clone())
            .collect()
    };
    let list = match &workspace.members {
        Loadable::Loaded(members) => {
            let mut rows: Vec<MemberLine> = members
                .iter()
                .map(|member| MemberLine {
                    account_id: member.account_id,
                    name: member.name.clone(),
                    is_self: member.account_id == model.account.id,
                    roles: role_names(member.role_ids.as_slice()),
                })
                .collect();
            rows.sort_by(|one, other| {
                by_name(
                    (one.name.as_str(), one.account_id),
                    (other.name.as_str(), other.account_id),
                )
            });
            MemberList::Loaded(rows)
        }
        Loadable::Loading => MemberList::Loading,
        Loadable::NotLoaded => {
            if known.members_failed.contains(&(server_id, workspace_id)) {
                MemberList::Failed
            } else {
                MemberList::Loading
            }
        }
    };
    let addable = model.is_administrator().then(|| {
        match (&model.admin.accounts, workspace.members.loaded()) {
            (Collection::Failed(failure), _) => Listed::Failed(*failure),
            (Collection::Loaded(accounts), Some(members)) => {
                let mut rows: Vec<AddableLine> = accounts
                    .iter()
                    .filter(|account| {
                        account.status == AccountStatus::Active
                            && !members.iter().any(|member| member.account_id == account.id)
                    })
                    .map(|account| AddableLine {
                        account_id: account.id,
                        name: account.name.clone(),
                    })
                    .collect();
                rows.sort_by(|one, other| {
                    by_name(
                        (one.name.as_str(), one.account_id),
                        (other.name.as_str(), other.account_id),
                    )
                });
                Listed::Loaded(rows)
            }
            _ => Listed::Loading,
        }
    });
    MembersPage {
        workspace_id,
        workspace_name: workspace.workspace.name.clone(),
        members: list,
        addable,
    }
}

/// The page of the invites of the server.
pub fn invites(
    server_id: ServerId,
    model: &ServerModel,
    known: &Known,
    admin: &AdminState,
) -> InvitesPage {
    InvitesPage {
        invites: Listed::of(&model.admin.invites, |invites| {
            invites
                .iter()
                .map(|invite| invite_line(server_id, invite, model, known))
                .collect()
        }),
        grant: Grant::of(model),
        made: match &admin.made {
            Some(Made::Invite { invite_id, code }) => Some(NewCode {
                invite_id: *invite_id,
                code: code.clone(),
            }),
            _ => None,
        },
    }
}

/// The page of the join requests that wait.
pub fn requests(model: &ServerModel, known: &Known) -> RequestsPage {
    RequestsPage {
        requests: Listed::of(&model.admin.join_requests, |requests| {
            requests
                .iter()
                .map(|request| RequestLine {
                    id: request.id,
                    name: request.name.clone(),
                    created: known.zone.local(request.created_at),
                    expires: known.zone.local(request.expires_at),
                })
                .collect()
        }),
        grant: Grant::of(model),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use junto_client_core::model::{AdminModel, LoadFailure};
    use junto_client_core::protocol::ServerPermission;
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{ServerRoleId, StreamEpoch};
    use junto_client_core::protocol::invite::JoinRequest;
    use junto_client_core::protocol::roles::WorkspaceRole;
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{Workspace, WorkspaceMember};
    use time::UtcOffset;

    use super::*;
    use crate::state::feed::Zone;

    fn time() -> OffsetDateTime {
        // 15 January 2027, 08:00 UTC.
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn known() -> Known {
        Known {
            zone: Zone::Fixed(UtcOffset::UTC),
            ..Known::default()
        }
    }

    /// One server with the workspaces "Studio" (default) and "Lab". The account "Luca" is
    /// a member of "Studio" only and holds no permission.
    struct Scene {
        server_id: ServerId,
        own: AccountId,
        studio: WorkspaceId,
        lab: WorkspaceId,
        snapshot: Snapshot,
    }

    fn workspace(id: WorkspaceId, name: &str, is_default: bool) -> WorkspaceState {
        WorkspaceState {
            workspace: Workspace {
                id,
                name: name.to_owned(),
                is_default,
            },
            own_role_ids: Vec::new(),
            roles: Vec::new(),
            categories: Vec::new(),
            channels: Vec::new(),
            calls: Vec::new(),
        }
    }

    fn scene() -> Scene {
        let own = AccountId::generate();
        let (studio, lab) = (WorkspaceId::generate(), WorkspaceId::generate());
        Scene {
            server_id: ServerId::generate(),
            own,
            studio,
            lab,
            snapshot: Snapshot {
                cursor: Cursor {
                    epoch: StreamEpoch::generate(),
                    position: 1,
                },
                account: OwnAccount {
                    id: own,
                    name: "Luca".to_owned(),
                    avatar: None,
                    is_owner: false,
                    server_role_ids: Vec::new(),
                    server_permissions: Vec::new(),
                    workspace_ids: vec![studio],
                },
                workspaces: vec![
                    workspace(studio, "Studio", true),
                    workspace(lab, "Lab", false),
                ],
                own_call: None,
                direct_calls: Vec::new(),
                call_invitations: Vec::new(),
                direct_mutes: Vec::new(),
                messages: Vec::new(),
                threads: Vec::new(),
                threads_more: false,
                threads_next: None,
                notifications: Vec::new(),
                notification_settings: Vec::new(),
            },
        }
    }

    impl Scene {
        fn administrator(mut self) -> Self {
            self.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
            self
        }

        fn model(&self, admin: AdminModel) -> ServerModel {
            let mut model = ServerModel::from_snapshot(self.snapshot.clone());
            model.admin = Arc::new(admin);
            model
        }
    }

    fn account(name: &str, status: AccountStatus) -> AccountSummary {
        AccountSummary {
            id: AccountId::generate(),
            name: name.to_owned(),
            status,
            is_owner: false,
            server_role_ids: Vec::new(),
            created_at: time(),
        }
    }

    fn with_accounts(accounts: Vec<AccountSummary>) -> AdminModel {
        AdminModel {
            accounts: Collection::Loaded(accounts),
            ..AdminModel::default()
        }
    }

    fn member(
        account_id: AccountId,
        name: &str,
        role_ids: Vec<WorkspaceRoleId>,
    ) -> WorkspaceMember {
        WorkspaceMember {
            account_id,
            name: name.to_owned(),
            role_ids,
        }
    }

    fn set_members(model: &mut ServerModel, index: usize, members: Loadable<Vec<WorkspaceMember>>) {
        Arc::make_mut(&mut model.workspaces[index]).members = members;
    }

    fn invite(created_by: AccountId) -> Invite {
        Invite {
            id: InviteId::generate(),
            created_by,
            created_at: time(),
            expires_at: time() + Duration::days(7),
            max_uses: 1,
            uses: 0,
            workspace_ids: Vec::new(),
            revocation: None,
        }
    }

    fn with_invites(invites: Vec<Invite>) -> AdminModel {
        AdminModel {
            invites: Collection::Loaded(invites),
            ..AdminModel::default()
        }
    }

    fn line(page: &PeoplePage, id: AccountId) -> &AccountLine {
        page.accounts
            .loaded()
            .unwrap()
            .iter()
            .find(|row| row.id == id)
            .unwrap()
    }

    fn first_invite(page: &InvitesPage) -> &InviteLine {
        &page.invites.loaded().unwrap()[0]
    }

    #[test]
    fn a_list_is_on_its_way_until_it_is_read() {
        let scene = scene();
        for admin in [
            AdminModel::default(),
            AdminModel {
                accounts: Collection::Loading,
                invites: Collection::Loading,
                join_requests: Collection::Loading,
                ..AdminModel::default()
            },
        ] {
            let model = scene.model(admin);
            let page = people(&model, &known());
            assert_eq!(page.accounts, Listed::Loading);
            assert_eq!(page.banned, 0);
            let page = invites(scene.server_id, &model, &known(), &AdminState::default());
            assert_eq!(page.invites, Listed::Loading);
            assert_eq!(requests(&model, &known()).requests, Listed::Loading);
        }
    }

    #[test]
    fn a_list_that_was_not_read_carries_its_failure() {
        let scene = scene();
        let model = scene.model(AdminModel {
            accounts: Collection::Failed(LoadFailure::NotAllowed),
            invites: Collection::Failed(LoadFailure::Unavailable),
            join_requests: Collection::Failed(LoadFailure::NoAnswer),
            ..AdminModel::default()
        });
        assert_eq!(
            people(&model, &known()).accounts,
            Listed::Failed(LoadFailure::NotAllowed)
        );
        assert_eq!(
            invites(scene.server_id, &model, &known(), &AdminState::default()).invites,
            Listed::Failed(LoadFailure::Unavailable)
        );
        assert_eq!(
            requests(&model, &known()).requests,
            Listed::Failed(LoadFailure::NoAnswer)
        );
    }

    #[test]
    fn a_list_that_was_read_and_holds_nothing_is_empty() {
        let scene = scene();
        let model = scene.model(AdminModel {
            accounts: Collection::Loaded(Vec::new()),
            invites: Collection::Loaded(Vec::new()),
            join_requests: Collection::Loaded(Vec::new()),
            ..AdminModel::default()
        });
        assert_eq!(
            people(&model, &known()).accounts,
            Listed::Loaded(Vec::new())
        );
        assert_eq!(
            invites(scene.server_id, &model, &known(), &AdminState::default()).invites,
            Listed::Loaded(Vec::new())
        );
        assert_eq!(
            requests(&model, &known()).requests,
            Listed::Loaded(Vec::new())
        );
    }

    #[test]
    fn an_active_account_can_be_removed_and_banned_and_not_unbanned() {
        let scene = scene();
        let ana = account("Ana", AccountStatus::Active);
        let page = people(&scene.model(with_accounts(vec![ana.clone()])), &known());
        let row = line(&page, ana.id);
        assert_eq!(row.status, AccountStatus::Active);
        assert_eq!(row.remove, Ok(()));
        assert_eq!(row.ban, Ok(()));
        assert_eq!(row.unban, Err(Blocked::NotBanned));
        assert!(!row.is_self);
        assert!(!row.is_owner);
        assert_eq!(page.banned, 0);
    }

    #[test]
    fn a_banned_account_can_be_removed_and_unbanned_and_is_counted() {
        let scene = scene();
        let ana = account("Ana", AccountStatus::Banned);
        let bea = account("Bea", AccountStatus::Banned);
        let caio = account("Caio", AccountStatus::Active);
        let page = people(
            &scene.model(with_accounts(vec![ana.clone(), bea, caio])),
            &known(),
        );
        let row = line(&page, ana.id);
        assert_eq!(row.status, AccountStatus::Banned);
        assert_eq!(row.remove, Ok(()));
        assert_eq!(row.ban, Err(Blocked::Banned));
        assert_eq!(row.unban, Ok(()));
        assert_eq!(page.banned, 2);
    }

    #[test]
    fn a_removed_account_takes_no_removal_no_ban_and_no_unban() {
        let scene = scene();
        let ana = account("Ana", AccountStatus::Removed);
        let page = people(&scene.model(with_accounts(vec![ana.clone()])), &known());
        let row = line(&page, ana.id);
        assert_eq!(row.status, AccountStatus::Removed);
        assert_eq!(row.remove, Err(Blocked::Removed));
        assert_eq!(row.ban, Err(Blocked::Removed));
        assert_eq!(row.unban, Err(Blocked::Removed));
        assert_eq!(page.banned, 0);
    }

    #[test]
    fn the_own_account_cannot_be_removed_or_banned() {
        let scene = scene();
        let mut own = account("Luca", AccountStatus::Active);
        own.id = scene.own;
        let page = people(&scene.model(with_accounts(vec![own])), &known());
        let row = line(&page, scene.own);
        assert!(row.is_self);
        assert_eq!(row.remove, Err(Blocked::OwnAccount));
        assert_eq!(row.ban, Err(Blocked::OwnAccount));
        assert_eq!(row.unban, Err(Blocked::OwnAccount));
    }

    #[test]
    fn the_server_owner_cannot_be_removed_or_banned() {
        let mut scene = scene();
        let mut owner = account("Olga", AccountStatus::Active);
        owner.is_owner = true;
        let page = people(&scene.model(with_accounts(vec![owner.clone()])), &known());
        let row = line(&page, owner.id);
        assert!(row.is_owner);
        assert_eq!(row.remove, Err(Blocked::Owner));
        assert_eq!(row.ban, Err(Blocked::Owner));
        assert_eq!(row.unban, Err(Blocked::Owner));

        // The owner who looks at the own account gets the reason that the server gives.
        scene.snapshot.account.is_owner = true;
        owner.id = scene.own;
        let page = people(&scene.model(with_accounts(vec![owner])), &known());
        let row = line(&page, scene.own);
        assert!(row.is_self);
        assert_eq!(row.remove, Err(Blocked::Owner));
    }

    #[test]
    fn the_roles_of_an_account_are_named_when_the_server_roles_are_loaded() {
        let scene = scene();
        let role = |name: &str| ServerRole {
            id: ServerRoleId::generate(),
            name: name.to_owned(),
            permissions: Vec::new(),
        };
        let (moderators, recruiters, unused) =
            (role("Moderators"), role("Recruiters"), role("Unused"));
        let mut ana = account("Ana", AccountStatus::Active);
        // A role that the server has deleted is not named.
        ana.server_role_ids = vec![recruiters.id, moderators.id, ServerRoleId::generate()];
        let bea = account("Bea", AccountStatus::Active);
        let model = scene.model(AdminModel {
            accounts: Collection::Loaded(vec![ana.clone(), bea.clone()]),
            server_roles: Collection::Loaded(vec![moderators, recruiters, unused]),
            ..AdminModel::default()
        });
        let page = people(&model, &known());
        assert_eq!(
            line(&page, ana.id).roles,
            RoleNames::Named(vec!["Moderators".to_owned(), "Recruiters".to_owned()])
        );
        assert_eq!(line(&page, bea.id).roles, RoleNames::Named(Vec::new()));
    }

    #[test]
    fn the_roles_of_an_account_are_counted_when_the_server_roles_are_not_loaded() {
        let scene = scene();
        let mut ana = account("Ana", AccountStatus::Active);
        ana.server_role_ids = vec![ServerRoleId::generate(), ServerRoleId::generate()];
        let bea = account("Bea", AccountStatus::Active);
        for server_roles in [
            Collection::NotLoaded,
            Collection::Loading,
            Collection::Failed(LoadFailure::NotAllowed),
        ] {
            let model = scene.model(AdminModel {
                accounts: Collection::Loaded(vec![ana.clone(), bea.clone()]),
                server_roles,
                ..AdminModel::default()
            });
            let page = people(&model, &known());
            assert_eq!(line(&page, ana.id).roles, RoleNames::Count(2));
            assert_eq!(line(&page, bea.id).roles, RoleNames::Count(0));
        }
    }

    #[test]
    fn accounts_are_in_the_order_of_their_names_with_their_date_in_the_zone_of_the_person() {
        let scene = scene();
        let mut late = account("bruno", AccountStatus::Active);
        // 23:30 UTC is the next day two hours east of it.
        late.created_at = time() + Duration::minutes(15 * 60 + 30);
        let accounts = vec![
            account("Zoe", AccountStatus::Active),
            late.clone(),
            account("Ana", AccountStatus::Removed),
        ];
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        let known = Known {
            zone: Zone::Fixed(offset),
            ..Known::default()
        };
        let page = people(&scene.model(with_accounts(accounts)), &known);
        let names: Vec<&str> = page
            .accounts
            .loaded()
            .unwrap()
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(names, ["Ana", "bruno", "Zoe"]);
        let row = line(&page, late.id);
        assert_eq!(row.created.offset(), offset);
        assert_eq!(row.created.date(), (time() + Duration::days(1)).date());
        assert_eq!((row.created.hour(), row.created.minute()), (1, 30));
    }

    #[test]
    fn the_members_of_a_workspace_carry_the_names_of_their_roles_without_the_everyone_role() {
        let mut scene = scene();
        let role = |name: &str, is_everyone| WorkspaceRole {
            id: WorkspaceRoleId::generate(),
            workspace_id: scene.studio,
            name: name.to_owned(),
            permissions: Vec::new(),
            is_everyone,
        };
        let (everyone, designers, leads) = (
            role("everyone", true),
            role("Designers", false),
            role("Leads", false),
        );
        scene.snapshot.workspaces[0].roles =
            vec![everyone.clone(), designers.clone(), leads.clone()];
        let mut model = scene.model(AdminModel::default());
        let ana = AccountId::generate();
        set_members(
            &mut model,
            0,
            Loadable::Loaded(vec![
                member(scene.own, "Luca", Vec::new()),
                member(ana, "Ana", vec![leads.id, everyone.id, designers.id]),
            ]),
        );
        let page = members(
            scene.server_id,
            &model,
            model.workspace(scene.studio).unwrap(),
            &known(),
        );
        assert_eq!(page.workspace_id, scene.studio);
        assert_eq!(page.workspace_name, "Studio");
        assert_eq!(
            page.members,
            MemberList::Loaded(vec![
                MemberLine {
                    account_id: ana,
                    name: "Ana".to_owned(),
                    is_self: false,
                    roles: vec!["Designers".to_owned(), "Leads".to_owned()],
                },
                MemberLine {
                    account_id: scene.own,
                    name: "Luca".to_owned(),
                    is_self: true,
                    roles: Vec::new(),
                },
            ])
        );
    }

    #[test]
    fn the_members_are_on_their_way_until_they_are_read_and_failed_when_the_read_failed() {
        let scene = scene();
        let mut model = scene.model(AdminModel::default());
        let page = |model: &ServerModel, known: &Known| {
            members(
                scene.server_id,
                model,
                model.workspace(scene.studio).unwrap(),
                known,
            )
            .members
        };
        assert_eq!(page(&model, &known()), MemberList::Loading);
        let mut failed = known();
        failed.members_failed.push((scene.server_id, scene.studio));
        assert_eq!(page(&model, &failed), MemberList::Failed);
        // The failure of another workspace does not count.
        let mut other = known();
        other.members_failed.push((scene.server_id, scene.lab));
        assert_eq!(page(&model, &other), MemberList::Loading);
        set_members(&mut model, 0, Loadable::Loading);
        assert_eq!(page(&model, &failed), MemberList::Loading);
        set_members(&mut model, 0, Loadable::Loaded(Vec::new()));
        assert_eq!(page(&model, &known()), MemberList::Loaded(Vec::new()));
    }

    #[test]
    fn an_administrator_sees_the_active_accounts_that_are_not_members_yet() {
        let scene = scene().administrator();
        let mut own = account("Luca", AccountStatus::Active);
        own.id = scene.own;
        let zoe = account("Zoe", AccountStatus::Active);
        let ana = account("Ana", AccountStatus::Active);
        let banned = account("Bea", AccountStatus::Banned);
        let removed = account("Rui", AccountStatus::Removed);
        let mut model = scene.model(with_accounts(vec![
            own,
            zoe.clone(),
            ana.clone(),
            banned,
            removed,
        ]));
        set_members(
            &mut model,
            0,
            Loadable::Loaded(vec![member(scene.own, "Luca", Vec::new())]),
        );
        let page = members(
            scene.server_id,
            &model,
            model.workspace(scene.studio).unwrap(),
            &known(),
        );
        assert_eq!(
            page.addable,
            Some(Listed::Loaded(vec![
                AddableLine {
                    account_id: ana.id,
                    name: "Ana".to_owned(),
                },
                AddableLine {
                    account_id: zoe.id,
                    name: "Zoe".to_owned(),
                },
            ]))
        );
    }

    #[test]
    fn a_person_who_is_not_an_administrator_sees_no_account_to_add() {
        let mut scene = scene();
        scene.snapshot.account.server_permissions = vec![ServerPermission::BanMembers];
        let mut model = scene.model(with_accounts(vec![account("Ana", AccountStatus::Active)]));
        set_members(&mut model, 0, Loadable::Loaded(Vec::new()));
        let page = members(
            scene.server_id,
            &model,
            model.workspace(scene.studio).unwrap(),
            &known(),
        );
        assert_eq!(page.addable, None);
    }

    #[test]
    fn the_accounts_to_add_wait_for_the_accounts_and_for_the_members() {
        let scene = scene().administrator();
        let addable = |model: &ServerModel| {
            members(
                scene.server_id,
                model,
                model.workspace(scene.studio).unwrap(),
                &known(),
            )
            .addable
        };
        // The accounts are read and the members are not.
        let model = scene.model(with_accounts(vec![account("Ana", AccountStatus::Active)]));
        assert_eq!(addable(&model), Some(Listed::Loading));
        // The members are read and the accounts are not.
        let mut model = scene.model(AdminModel::default());
        set_members(&mut model, 0, Loadable::Loaded(Vec::new()));
        assert_eq!(addable(&model), Some(Listed::Loading));
        let mut model = scene.model(AdminModel {
            accounts: Collection::Failed(LoadFailure::Unavailable),
            ..AdminModel::default()
        });
        set_members(&mut model, 0, Loadable::Loaded(Vec::new()));
        assert_eq!(
            addable(&model),
            Some(Listed::Failed(LoadFailure::Unavailable))
        );
    }

    #[test]
    fn an_invite_names_its_creator_when_the_state_holds_the_name() {
        let scene = scene();
        let listed = account("Ana", AccountStatus::Active);
        let (in_lab, read_alone, unknown) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let mut model = scene.model(AdminModel {
            accounts: Collection::Loaded(vec![listed.clone()]),
            invites: Collection::Loaded(vec![
                invite(scene.own),
                invite(listed.id),
                invite(in_lab),
                invite(read_alone),
                invite(unknown),
            ]),
            ..AdminModel::default()
        });
        // The members of a workspace that the window does not show count too.
        set_members(
            &mut model,
            1,
            Loadable::Loaded(vec![member(in_lab, "Bea", Vec::new())]),
        );
        let mut known = known();
        known
            .names
            .insert((scene.server_id, read_alone), "Caio".to_owned());
        // A name that was read for another server is not the name of this account.
        known
            .names
            .insert((ServerId::generate(), unknown), "Dora".to_owned());
        let page = invites(scene.server_id, &model, &known, &AdminState::default());
        let creators: Vec<(AccountId, Option<&str>)> = page
            .invites
            .loaded()
            .unwrap()
            .iter()
            .map(|row| (row.created_by, row.creator.as_deref()))
            .collect();
        assert_eq!(
            creators,
            [
                (scene.own, Some("Luca")),
                (listed.id, Some("Ana")),
                (in_lab, Some("Bea")),
                (read_alone, Some("Caio")),
                (unknown, None),
            ]
        );
    }

    #[test]
    fn an_invite_names_its_workspaces_and_counts_those_that_the_state_does_not_hold() {
        let scene = scene();
        let mut held = invite(scene.own);
        held.workspace_ids = vec![
            scene.lab,
            WorkspaceId::generate(),
            scene.studio,
            WorkspaceId::generate(),
        ];
        held.max_uses = 25;
        held.uses = 3;
        let page = invites(
            scene.server_id,
            &scene.model(with_invites(vec![held.clone()])),
            &known(),
            &AdminState::default(),
        );
        let row = first_invite(&page);
        assert_eq!(row.id, held.id);
        assert_eq!(row.workspaces, ["Lab", "Studio"]);
        assert_eq!(row.other_workspaces, 2);
        assert_eq!((row.uses, row.max_uses), (3, 25));
        assert_eq!(row.created, time());
        assert_eq!(row.expires, time() + Duration::days(7));
    }

    #[test]
    fn an_invite_is_usable_used_up_revoked_or_revoked_by_the_ban_of_its_creator() {
        let scene = scene();
        let state = |change: &dyn Fn(&mut Invite)| {
            let mut held = invite(scene.own);
            change(&mut held);
            let page = invites(
                scene.server_id,
                &scene.model(with_invites(vec![held])),
                &known(),
                &AdminState::default(),
            );
            let row = first_invite(&page);
            (row.state, row.may_revoke)
        };
        assert_eq!(state(&|_| {}), (InviteState::Usable, true));
        assert_eq!(
            state(&|held| {
                held.max_uses = 5;
                held.uses = 4;
            }),
            (InviteState::Usable, true)
        );
        assert_eq!(state(&|held| held.uses = 1), (InviteState::UsedUp, false));
        assert_eq!(
            state(&|held| held.revocation = Some(InviteRevocation::Revoked)),
            (InviteState::Revoked, false)
        );
        assert_eq!(
            state(&|held| held.revocation = Some(InviteRevocation::CreatorBanned)),
            (InviteState::CreatorBanned, false)
        );
        // A revoked invite that is used up too is shown as revoked.
        assert_eq!(
            state(&|held| {
                held.uses = 1;
                held.revocation = Some(InviteRevocation::Revoked);
            }),
            (InviteState::Revoked, false)
        );
    }

    #[test]
    fn an_invite_is_expired_when_the_day_of_its_expiry_is_before_today() {
        let scene = scene();
        // The invite expires on 22 January 2027 at 23:30 UTC.
        let mut held = invite(scene.own);
        held.expires_at = time() + Duration::days(7) + Duration::minutes(15 * 60 + 30);
        let model = scene.model(with_invites(vec![held.clone()]));
        let state = |known: &Known| {
            let page = invites(scene.server_id, &model, known, &AdminState::default());
            let row = first_invite(&page);
            (row.state, row.may_revoke)
        };
        let day = |days: i64| Some((time() + Duration::days(days)).date());
        // Without the date of today nothing is known about the expiry.
        assert_eq!(state(&known()), (InviteState::Usable, true));
        let mut known = known();
        known.today = day(6);
        assert_eq!(state(&known), (InviteState::Usable, true));
        // On the day of the expiry the invite still counts as usable.
        known.today = day(7);
        assert_eq!(state(&known), (InviteState::Usable, true));
        known.today = day(8);
        assert_eq!(state(&known), (InviteState::Expired, false));
        // Two hours east of UTC the expiry is on 23 January.
        known.zone = Zone::Fixed(UtcOffset::from_hms(2, 0, 0).unwrap());
        assert_eq!(state(&known), (InviteState::Usable, true));
        known.today = day(9);
        assert_eq!(state(&known), (InviteState::Expired, false));

        // An expired invite that is used up too is shown as expired, as the server
        // refuses its code.
        held.uses = 1;
        let model = scene.model(with_invites(vec![held]));
        let page = invites(scene.server_id, &model, &known, &AdminState::default());
        assert_eq!(first_invite(&page).state, InviteState::Expired);
    }

    #[test]
    fn an_administrator_may_grant_every_workspace_of_the_state() {
        let scene = scene().administrator();
        let model = scene.model(AdminModel::default());
        let choices = vec![
            WorkspaceChoice {
                id: scene.studio,
                name: "Studio".to_owned(),
                is_default: true,
            },
            WorkspaceChoice {
                id: scene.lab,
                name: "Lab".to_owned(),
                is_default: false,
            },
        ];
        let page = invites(scene.server_id, &model, &known(), &AdminState::default());
        assert_eq!(page.grant.workspaces, choices);
        assert_eq!(requests(&model, &known()).grant.workspaces, choices);
        assert_eq!(
            page.grant.default_workspace().map(|held| held.id),
            Some(scene.studio)
        );

        // The server owner is an administrator without the permission.
        let mut owner = self::scene();
        owner.snapshot.account.is_owner = true;
        let page = requests(&owner.model(AdminModel::default()), &known());
        assert_eq!(page.grant.workspaces.len(), 2);
    }

    #[test]
    fn a_person_who_is_not_an_administrator_grants_the_workspaces_of_the_account() {
        let mut scene = scene();
        scene.snapshot.account.server_permissions = vec![
            ServerPermission::CreateInvites,
            ServerPermission::ApproveJoinRequests,
        ];
        let model = scene.model(AdminModel::default());
        let studio = vec![WorkspaceChoice {
            id: scene.studio,
            name: "Studio".to_owned(),
            is_default: true,
        }];
        let page = invites(scene.server_id, &model, &known(), &AdminState::default());
        assert_eq!(page.grant.workspaces, studio);
        assert_eq!(requests(&model, &known()).grant.workspaces, studio);
        // The default workspace can be granted: a request may name no workspace.
        assert_eq!(page.grant.fault(0), None);

        // A member of "Lab" only may not grant the default workspace.
        scene.snapshot.account.workspace_ids = vec![scene.lab];
        let model = scene.model(AdminModel::default());
        let grant = requests(&model, &known()).grant;
        assert_eq!(
            grant.workspaces,
            [WorkspaceChoice {
                id: scene.lab,
                name: "Lab".to_owned(),
                is_default: false,
            }]
        );
        assert_eq!(grant.default_workspace(), None);
        assert_eq!(grant.fault(0), Some(GrantFault::NoneChosen));
        assert_eq!(grant.fault(1), None);
    }

    #[test]
    fn a_request_names_at_most_100_workspaces_that_the_person_may_grant() {
        let scene = scene().administrator();
        let grant = requests(&scene.model(AdminModel::default()), &known()).grant;
        assert_eq!(grant.fault(MOST_WORKSPACES), None);
        assert_eq!(grant.fault(MOST_WORKSPACES + 1), Some(GrantFault::TooMany));
        assert_eq!(MOST_WORKSPACES, 100);
        // A chosen workspace that the state holds no longer is not sent, and the order
        // is the one of the server.
        let gone = WorkspaceId::generate();
        assert_eq!(
            grant.kept(&[gone, scene.lab, scene.studio]),
            [scene.studio, scene.lab]
        );
        assert!(grant.kept(&[gone]).is_empty());
    }

    #[test]
    fn the_code_of_a_new_invite_is_carried_with_its_invite_until_it_is_put_away() {
        let scene = scene();
        let model = scene.model(with_invites(Vec::new()));
        let invite_id = InviteId::generate();
        let admin = AdminState {
            made: Some(Made::Invite {
                invite_id,
                code: InviteCode::new("ABCD-EFGH-JKMN-PQRS"),
            }),
            ..AdminState::default()
        };
        let page = invites(scene.server_id, &model, &known(), &admin);
        let made = page.made.unwrap();
        assert_eq!(made.invite_id, invite_id);
        assert_eq!(made.code.expose(), "ABCD-EFGH-JKMN-PQRS");

        let page = invites(scene.server_id, &model, &known(), &AdminState::default());
        assert_eq!(page.made, None);
        // What another request made is not a code of an invite.
        let admin = AdminState {
            made: Some(Made::Entered(vec![scene.lab])),
            ..AdminState::default()
        };
        let page = invites(scene.server_id, &model, &known(), &admin);
        assert_eq!(page.made, None);
    }

    #[test]
    fn a_new_invite_asks_for_its_expiry_within_the_longest_lifetime() {
        let now = time();
        // The server sets the default lifetime of 7 days.
        assert_eq!(expiry(DEFAULT_EXPIRY_DAYS, now), None);
        assert_eq!(expiry(1, now), Some(now + Duration::days(1)));
        assert_eq!(expiry(30, now), Some(now + Duration::days(30)));
        let longest = expiry(LONGEST_EXPIRY_DAYS, now).unwrap();
        assert!(longest < now + Duration::days(365));
        assert!(longest > now + Duration::days(364));
        // A lifetime outside the bounds of the server is brought inside them.
        assert_eq!(expiry(4000, now), Some(longest));
        assert_eq!(expiry(0, now), Some(now + Duration::days(1)));
        assert_eq!(EXPIRY_DAYS, [1, 7, 30, 365]);
        assert!(EXPIRY_DAYS.contains(&DEFAULT_EXPIRY_DAYS));
    }

    #[test]
    fn a_typed_number_of_uses_is_from_1_to_10000() {
        assert_eq!(typed_uses("1"), Some(1));
        assert_eq!(typed_uses(" 250 "), Some(250));
        assert_eq!(typed_uses("10000"), Some(10_000));
        assert_eq!(typed_uses("10001"), None);
        assert_eq!(typed_uses("0"), None);
        assert_eq!(typed_uses("-3"), None);
        assert_eq!(typed_uses("ten"), None);
        assert_eq!(typed_uses(""), None);
        assert_eq!(USES, [1, 5, 25, 100]);
        assert!(USES.contains(&DEFAULT_USES));
    }

    #[test]
    fn join_requests_carry_their_name_and_their_times_in_the_zone_of_the_person() {
        let scene = scene();
        let request = |name: &str, created_at: OffsetDateTime| JoinRequest {
            id: JoinRequestId::generate(),
            name: name.to_owned(),
            created_at,
            expires_at: created_at + Duration::days(7),
        };
        let (first, second) = (
            request("Nina", time()),
            request("Otto", time() + Duration::hours(1)),
        );
        let model = scene.model(AdminModel {
            join_requests: Collection::Loaded(vec![first.clone(), second.clone()]),
            ..AdminModel::default()
        });
        let offset = UtcOffset::from_hms(-3, 0, 0).unwrap();
        let known = Known {
            zone: Zone::Fixed(offset),
            ..Known::default()
        };
        let page = requests(&model, &known);
        let rows = page.requests.loaded().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.id, row.name.as_str()))
                .collect::<Vec<_>>(),
            [(first.id, "Nina"), (second.id, "Otto")]
        );
        assert_eq!(rows[0].created, time());
        assert_eq!(rows[0].created.offset(), offset);
        assert_eq!(rows[0].created.hour(), 5);
        assert_eq!(rows[0].expires, time() + Duration::days(7));
        assert_eq!(rows[0].expires.offset(), offset);
    }
}
