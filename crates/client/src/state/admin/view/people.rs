//! The pages of the people of a server (ADR 0761): every account, the members of the
//! shown workspace, the invites and the join requests that wait.

use junto_client_core::model::{Collection, Loadable, ServerModel, WorkspaceModel, outranks};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::account::{AccountStatus, AccountSummary, RecoverAccount};
use junto_client_core::protocol::ids::{
    AccountId, InviteId, JoinRequestId, ServerId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::invite::{Invite, InviteRevocation};
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::protocol::structure::WorkspaceMember;
use time::{Duration, OffsetDateTime};

use super::Listed;
use super::roles::{RoleChoice, server_choices, workspace_choices};
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
/// The characters of a name of an account (ADR 0119).
const NAME_CHARACTERS: usize = 64;

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
    /// The account holds a server role at or above the highest server role of the
    /// person: role order refuses the removal, the ban and the unban (ADR 0309).
    Outranked,
    /// The order of the server roles is on its way, so the page cannot tell yet whether
    /// the account stands below the person.
    OrderNotRead,
    /// The server states that the person may not unban the account: with the roles that
    /// it gets back in its workspaces it would stand at or above the person (ADR 0311).
    NotUnbannable,
}

/// Why the server owner cannot recover an account onto a new device (ADR 0104).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoRecovery {
    /// The own account of the owner gets a new device by a link (ADR 0112).
    OwnAccount,
    /// The server recovers an active account only.
    NotActive,
}

/// Why a member cannot be removed from the shown workspace (ADR 0106).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoRemoval {
    /// The server owner is not removed from a workspace (`owner_protected`).
    Owner,
    /// An account does not remove itself (`own_account_target`).
    OwnAccount,
    /// The member stands at or above the person in the workspace: role order refuses the
    /// removal (ADR 0309).
    Outranked,
    /// The accounts or the order of the roles are on their way, so the page cannot tell
    /// yet whether the member stands below the person.
    OrderNotRead,
}

/// The most characters of the reason of a ban or of a removal (ADR 0106).
pub const LONGEST_REASON: usize = junto_client_core::api::LONGEST_REASON;

/// Why a typed reason cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonFault {
    TooLong,
    /// The reason holds a control character, which the server refuses.
    Control,
}

/// The reason that a request carries for what the person typed: without the space
/// around it, and absent when nothing is left.
pub fn typed_reason(text: &str) -> Result<Option<String>, ReasonFault> {
    let Some(reason) = junto_client_core::api::stated_reason(Some(text)) else {
        return Ok(None);
    };
    if reason.chars().any(char::is_control) {
        Err(ReasonFault::Control)
    } else if reason.chars().count() > LONGEST_REASON {
        Err(ReasonFault::TooLong)
    } else {
        Ok(Some(reason))
    }
}

/// Why a typed recovery cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryFault {
    /// The text is no public key of a device.
    Key,
    /// The name of the new device breaks the rule of a device name (ADR 0112).
    DeviceName,
}

/// The recovery that a request carries for what the owner typed: the public key without
/// the spaces between the groups in which a window shows it (ADR 0782), and the name of
/// the new device by the rule of the client core.
pub fn typed_recovery(key: &str, device: &str) -> Result<RecoverAccount, RecoveryFault> {
    let key: String = key
        .chars()
        .filter(|symbol| !symbol.is_whitespace())
        .collect();
    let public_key = key.parse().map_err(|_| RecoveryFault::Key)?;
    let device_name =
        junto_client_core::profile::device_name(device).map_err(|_| RecoveryFault::DeviceName)?;
    Ok(RecoverAccount {
        public_key,
        device_name,
    })
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
    /// For an administrator and an active account: the server roles that the account
    /// holds or can be given. Absent while the order of the roles is not read.
    pub assignable: Option<Vec<RoleChoice>>,
    /// For the server owner: whether the account can be recovered onto a new device, and
    /// why not. Absent for every other person: only the owner recovers (ADR 0104).
    pub recover: Option<Result<(), NoRecovery>>,
}

impl AccountLine {
    /// True when role order is the reason why the account takes no removal, ban or unban.
    pub fn outranked(&self) -> bool {
        [self.remove, self.ban, self.unban].contains(&Err(Blocked::Outranked))
    }

    fn of(account: &AccountSummary, model: &ServerModel, known: &Known) -> Self {
        let own = model.account.id;
        let roles = model.admin.server_roles.loaded().map(Vec::as_slice);
        // A removal, a ban and an unban need the person above the account in the order
        // of the server roles; the owner stands above everyone (ADR 0309).
        let order = match (model.server_rank(), model.server_rank_of(account)) {
            // The owner stands above every account, whatever roles it holds.
            _ if model.account.is_owner => Ok(()),
            (Some(person), Some(target)) if outranks(person, target) => Ok(()),
            (Some(_), Some(_)) => Err(Blocked::Outranked),
            // A read that failed leaves the decision to the server.
            _ if matches!(model.admin.server_roles, Collection::Failed(_)) => Ok(()),
            _ => Err(Blocked::OrderNotRead),
        };
        let administrator = model.is_administrator();
        // The server names the owner first when the owner asks about the own account.
        let target = if account.is_owner {
            Err(Blocked::Owner)
        } else if account.id == own {
            Err(Blocked::OwnAccount)
        } else {
            Ok(())
        };
        // The server decides the unban with the roles that the account gets back, which
        // the client does not hold (ADR 0311). While its statement is read again the
        // action waits; a read that failed leaves the decision to the request.
        let unbannable = match model.admin.may_unban(account.id) {
            Some(true) => Ok(()),
            Some(false) => Err(Blocked::NotUnbannable),
            None if matches!(model.admin.accounts, Collection::Failed(_)) => Ok(()),
            None => Err(Blocked::OrderNotRead),
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
            remove: target.and(remove).and(order),
            ban: target.and(ban).and(order),
            unban: target.and(unban).and(order).and(unbannable),
            assignable: (administrator && account.status == AccountStatus::Active)
                .then(|| server_choices(model, &account.server_role_ids))
                .flatten(),
            recover: model.account.is_owner.then_some(if account.id == own {
                Err(NoRecovery::OwnAccount)
            } else if account.status != AccountStatus::Active {
                Err(NoRecovery::NotActive)
            } else {
                Ok(())
            }),
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
    /// The member list of the server states that the account set a picture (ADR 0119).
    pub pictured: bool,
    /// The names of the roles of the member in the workspace, without the "everyone"
    /// role.
    pub roles: Vec<String>,
    /// For a person with "manage roles": the roles of the workspace that the member holds
    /// or can be given. Absent while the order of the roles is not read.
    pub assignable: Option<Vec<RoleChoice>>,
    /// For an administrator: whether the member can be removed from the workspace, and
    /// why not. Absent for every other person: the server gives the removal to
    /// administrators only (ADR 0106).
    pub remove: Option<Result<(), NoRemoval>>,
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
    /// The name of the account that created the invite: the one that the state holds,
    /// and otherwise the one that the server states with the invite (ADR 0111). Absent
    /// for an invite of an earlier server whose creator the state does not hold.
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
        .or_else(|| known.people.name(server_id, account_id))
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
    // The name that the server states is text of another party.
    let stated = invite.created_by_name.as_deref().and_then(|name| {
        let name: String = name
            .chars()
            .filter(|character| !character.is_control())
            .take(NAME_CHARACTERS)
            .collect();
        (!name.trim().is_empty()).then_some(name)
    });
    InviteLine {
        id: invite.id,
        created_by: invite.created_by,
        creator: account_name(server_id, invite.created_by, model, known).or(stated),
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

/// Whether an administrator can remove a member from a workspace: the owner and the own
/// account are protected, and the member must stand below the person in the workspace
/// (ADR 0106, ADR 0309). The server decides the request; this is what the row states
/// before the person tries.
fn removal(
    model: &ServerModel,
    workspace_id: WorkspaceId,
    member: &WorkspaceMember,
) -> Result<(), NoRemoval> {
    if member.account_id == model.account.id {
        return Err(NoRemoval::OwnAccount);
    }
    let account = match &model.admin.accounts {
        Collection::Loaded(accounts) => accounts
            .iter()
            .find(|account| account.id == member.account_id),
        // A read that failed leaves the decision to the server.
        Collection::Failed(_) => return Ok(()),
        Collection::NotLoaded | Collection::Loading => None,
    };
    let Some(account) = account else {
        return Err(NoRemoval::OrderNotRead);
    };
    if account.is_owner {
        return Err(NoRemoval::Owner);
    }
    if model.account.is_owner {
        return Ok(());
    }
    let person = model.workspace_rank(workspace_id);
    let target = model.workspace_rank_of(workspace_id, account, &member.role_ids);
    match (person, target) {
        (Some(person), Some(target)) if outranks(person, target) => Ok(()),
        (Some(_), Some(_)) => Err(NoRemoval::Outranked),
        _ if matches!(model.admin.server_roles, Collection::Failed(_)) => Ok(()),
        _ => Err(NoRemoval::OrderNotRead),
    }
}

/// The page of every account of the server.
pub fn people(model: &ServerModel, known: &Known) -> PeoplePage {
    let accounts = Listed::of(&model.admin.accounts, |accounts| {
        let mut rows: Vec<AccountLine> = accounts
            .iter()
            .map(|account| AccountLine::of(account, model, known))
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
    let may_assign = workspace.holds(WorkspacePermission::ManageRoles);
    let administrator = model.is_administrator();
    let list = match &workspace.members {
        Loadable::Loaded(members) => {
            let mut rows: Vec<MemberLine> = members
                .iter()
                .map(|member| MemberLine {
                    account_id: member.account_id,
                    name: member.name.clone(),
                    is_self: member.account_id == model.account.id,
                    pictured: member.avatar.is_some(),
                    roles: role_names(member.role_ids.as_slice()),
                    assignable: may_assign
                        .then(|| workspace_choices(model, workspace, &member.role_ids))
                        .flatten(),
                    remove: administrator.then(|| removal(model, workspace_id, member)),
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
    use junto_client_core::protocol::roles::{ServerRole, WorkspaceRole};
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{Workspace, WorkspaceMember};
    use time::UtcOffset;

    use super::*;
    use crate::state::admin::RoleRef;
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
            permissions: Vec::new(),
            channel_permissions: Vec::new(),
            notification_default: None,
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
            },
        }
    }

    impl Scene {
        fn administrator(mut self) -> Self {
            self.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
            self
        }

        fn model(&self, admin: AdminModel) -> ServerModel {
            let mut model =
                ServerModel::from_snapshot(crate::state::fixtures::stated(self.snapshot.clone()));
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

    fn server_role(name: &str, position: i32) -> ServerRole {
        ServerRole {
            position,
            id: ServerRoleId::generate(),
            name: name.to_owned(),
            permissions: vec![ServerPermission::BanMembers],
        }
    }

    /// The person holds the server role "Wardens" at position 2, and the state holds the
    /// server roles "Stewards" (3), "Wardens" and "Clerks" (1) with these accounts.
    fn as_warden(
        scene: &mut Scene,
        accounts: Vec<AccountSummary>,
    ) -> (AdminModel, [ServerRole; 3]) {
        let roles = [
            server_role("Stewards", 3),
            server_role("Wardens", 2),
            server_role("Clerks", 1),
        ];
        scene.snapshot.account.server_role_ids = vec![roles[1].id];
        scene.snapshot.account.server_permissions = vec![ServerPermission::BanMembers];
        let mut admin = AdminModel {
            accounts: Collection::Loaded(accounts),
            server_roles: Collection::Loaded(roles.to_vec()),
            ..AdminModel::default()
        };
        state_flags(&mut admin, Some(roles[1].position));
        (admin, roles)
    }

    /// What the server states with the lists for a person who stands at the position
    /// `own` among the server roles, or for the owner: the person may unban, give, take
    /// and delete what stands below (ADR 0311).
    fn state_flags(admin: &mut AdminModel, own: Option<i32>) {
        let roles = admin.server_roles.loaded().cloned().unwrap_or_default();
        let below = |position: i32| own.is_none_or(|own| position < own);
        admin.server_role_actions = roles
            .iter()
            .map(|role| {
                let may = below(role.position);
                (
                    role.id,
                    junto_client_core::protocol::roles::RoleActions {
                        may_assign: may,
                        may_remove: may,
                        may_delete: may,
                    },
                )
            })
            .collect();
        let highest = |account: &AccountSummary| {
            roles
                .iter()
                .filter(|role| account.server_role_ids.contains(&role.id))
                .map(|role| role.position)
                .max()
                .unwrap_or(0)
        };
        admin.unbannable = admin
            .accounts
            .loaded()
            .into_iter()
            .flatten()
            .filter(|account| account.status == AccountStatus::Banned && below(highest(account)))
            .map(|account| account.id)
            .collect();
    }

    fn member(
        account_id: AccountId,
        name: &str,
        role_ids: Vec<WorkspaceRoleId>,
    ) -> WorkspaceMember {
        WorkspaceMember {
            avatar: None,
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
            created_by_name: None,
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
    fn a_typed_recovery_is_a_key_without_its_spaces_and_a_device_name() {
        let key = junto_client_core::protocol::keys::PublicKey::from_bytes([7; 32]);
        let grouped = crate::text::account::key_fingerprint(&key);
        assert!(grouped.contains(' '));
        let typed = typed_recovery(&format!(" {grouped}\n"), "  Replacement ").unwrap();
        assert_eq!(typed.public_key, key);
        assert_eq!(typed.device_name, "Replacement");
        assert_eq!(typed_recovery(&grouped, "").unwrap().device_name, "");
        assert_eq!(
            typed_recovery("not a key", "").unwrap_err(),
            RecoveryFault::Key
        );
        assert_eq!(
            typed_recovery(&grouped, &"n".repeat(65)).unwrap_err(),
            RecoveryFault::DeviceName
        );
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
        let mut scene = scene();
        let ana = account("Ana", AccountStatus::Active);
        let (admin, _) = as_warden(&mut scene, vec![ana.clone()]);
        let page = people(&scene.model(admin), &known());
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
        let mut scene = scene();
        let ana = account("Ana", AccountStatus::Banned);
        let bea = account("Bea", AccountStatus::Banned);
        let caio = account("Caio", AccountStatus::Active);
        let (admin, _) = as_warden(&mut scene, vec![ana.clone(), bea, caio]);
        let page = people(&scene.model(admin), &known());
        let row = line(&page, ana.id);
        assert_eq!(row.status, AccountStatus::Banned);
        assert_eq!(row.remove, Ok(()));
        assert_eq!(row.ban, Err(Blocked::Banned));
        assert_eq!(row.unban, Ok(()));
        assert_eq!(page.banned, 2);
    }

    #[test]
    fn an_account_at_or_above_the_person_takes_no_removal_no_ban_and_no_unban() {
        let mut scene = scene();
        let mut above = account("Ana", AccountStatus::Active);
        let mut equal = account("Bea", AccountStatus::Active);
        let mut below = account("Caio", AccountStatus::Active);
        let mut banned = account("Dora", AccountStatus::Banned);
        let plain = account("Emil", AccountStatus::Active);
        let accounts = [&above, &equal, &below, &banned, &plain].map(|held| held.id);
        let (mut admin, [stewards, wardens, clerks]) = as_warden(&mut scene, Vec::new());
        above.server_role_ids = vec![clerks.id, stewards.id];
        equal.server_role_ids = vec![wardens.id];
        below.server_role_ids = vec![clerks.id];
        banned.server_role_ids = vec![stewards.id];
        admin.accounts = Collection::Loaded(vec![above, equal, below, banned, plain]);
        state_flags(&mut admin, Some(wardens.position));
        let page = people(&scene.model(admin.clone()), &known());
        let states = |page: &PeoplePage, index: usize| {
            let row = line(page, accounts[index]);
            (row.remove, row.ban, row.unban, row.outranked())
        };
        let held = Err(Blocked::Outranked);
        // An account with a higher role, and one with the same highest role (ADR 0309).
        assert_eq!(
            states(&page, 0),
            (held, held, Err(Blocked::NotBanned), true)
        );
        assert_eq!(
            states(&page, 1),
            (held, held, Err(Blocked::NotBanned), true)
        );
        // An account below the person, and one without a role.
        assert_eq!(
            states(&page, 2),
            (Ok(()), Ok(()), Err(Blocked::NotBanned), false)
        );
        assert_eq!(states(&page, 3), (held, Err(Blocked::Banned), held, true));
        assert_eq!(
            states(&page, 4),
            (Ok(()), Ok(()), Err(Blocked::NotBanned), false)
        );
        // A person who is no administrator gives and takes no server role.
        assert!(line(&page, accounts[2]).assignable.is_none());

        // The owner stands above every account.
        scene.snapshot.account.is_owner = true;
        state_flags(&mut admin, None);
        let page = people(&scene.model(admin.clone()), &known());
        assert_eq!(
            states(&page, 0),
            (Ok(()), Ok(()), Err(Blocked::NotBanned), false)
        );
        assert_eq!(
            states(&page, 3),
            (Ok(()), Err(Blocked::Banned), Ok(()), false)
        );
        scene.snapshot.account.is_owner = false;
        state_flags(&mut admin, Some(wardens.position));

        // The server states that the person may not unban an account that it would let
        // back with roles above the person in a workspace (ADR 0311). The row says so.
        admin.unbannable.clear();
        scene.snapshot.account.is_owner = true;
        let page = people(&scene.model(admin.clone()), &known());
        assert_eq!(
            states(&page, 3),
            (
                Ok(()),
                Err(Blocked::Banned),
                Err(Blocked::NotUnbannable),
                false
            )
        );
        // While the accounts are read again after a change of the order, the unban waits.
        state_flags(&mut admin, None);
        admin.flags_stale = vec![junto_client_core::model::AdminPart::Accounts];
        let page = people(&scene.model(admin.clone()), &known());
        assert_eq!(states(&page, 3).2, Err(Blocked::OrderNotRead));
        admin.flags_stale.clear();
        scene.snapshot.account.is_owner = false;
        state_flags(&mut admin, Some(wardens.position));

        // The order of the roles is on its way: nothing is offered yet. After a read that
        // failed the server decides.
        let reading = Err(Blocked::OrderNotRead);
        admin.server_roles = Collection::Loading;
        let page = people(&scene.model(admin.clone()), &known());
        assert_eq!(
            states(&page, 2),
            (reading, reading, Err(Blocked::NotBanned), false)
        );
        admin.server_roles = Collection::Failed(LoadFailure::NoAnswer);
        let page = people(&scene.model(admin), &known());
        assert_eq!(
            states(&page, 0),
            (Ok(()), Ok(()), Err(Blocked::NotBanned), false)
        );
    }

    #[test]
    fn an_administrator_gives_and_takes_the_server_roles_below_its_own() {
        let mut scene = scene();
        let mut ana = account("Ana", AccountStatus::Active);
        let banned = account("Bea", AccountStatus::Banned);
        let (mut admin, [stewards, wardens, clerks]) = as_warden(&mut scene, Vec::new());
        scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
        ana.server_role_ids = vec![clerks.id];
        let (ana_id, banned_id) = (ana.id, banned.id);
        admin.accounts = Collection::Loaded(vec![ana, banned]);
        state_flags(&mut admin, Some(wardens.position));
        let page = people(&scene.model(admin), &known());
        let choices = line(&page, ana_id).assignable.clone().unwrap();
        let shown: Vec<(&str, bool, bool)> = choices
            .iter()
            .map(|choice| (choice.name.as_str(), choice.held, choice.blocked.is_some()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Stewards", false, true),
                ("Wardens", false, true),
                ("Clerks", true, false)
            ]
        );
        assert_eq!(choices[0].role, RoleRef::Server(stewards.id));
        assert_eq!(choices[1].role, RoleRef::Server(wardens.id));
        // A banned account gets no role (ADR 0113).
        assert!(line(&page, banned_id).assignable.is_none());
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
        let role = |name: &str, position| ServerRole {
            position,
            id: ServerRoleId::generate(),
            name: name.to_owned(),
            permissions: Vec::new(),
        };
        let (moderators, recruiters, unused) = (
            role("Moderators", 3),
            role("Recruiters", 2),
            role("Unused", 1),
        );
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
        let role = |name: &str, position| WorkspaceRole {
            position,
            id: WorkspaceRoleId::generate(),
            workspace_id: scene.studio,
            name: name.to_owned(),
            permissions: Vec::new(),
            is_everyone: position == 0,
        };
        let (everyone, designers, leads) =
            (role("everyone", 0), role("Designers", 2), role("Leads", 1));
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
                    pictured: false,
                    roles: vec!["Designers".to_owned(), "Leads".to_owned()],
                    assignable: None,
                    remove: None,
                },
                MemberLine {
                    account_id: scene.own,
                    name: "Luca".to_owned(),
                    is_self: true,
                    pictured: false,
                    roles: Vec::new(),
                    assignable: None,
                    remove: None,
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
        known.people.read_name(scene.server_id, read_alone, "Caio");
        // A name that was read for another server is not the name of this account.
        known
            .people
            .read_name(ServerId::generate(), unknown, "Dora");
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

    #[test]
    fn a_typed_reason_is_trimmed_and_checked_before_it_is_sent() {
        assert_eq!(typed_reason(""), Ok(None));
        assert_eq!(typed_reason("  \u{a0} "), Ok(None));
        assert_eq!(
            typed_reason("  Spam in #general "),
            Ok(Some("Spam in #general".to_owned()))
        );
        let longest = "é".repeat(LONGEST_REASON);
        assert_eq!(typed_reason(&longest), Ok(Some(longest.clone())));
        assert_eq!(
            typed_reason(&format!("{longest}a")),
            Err(ReasonFault::TooLong)
        );
        // The space around a reason does not count.
        assert_eq!(typed_reason(&format!(" {longest} ")), Ok(Some(longest)));
        assert_eq!(typed_reason("two\tparts"), Err(ReasonFault::Control));
        assert_eq!(typed_reason("a\u{7}b"), Err(ReasonFault::Control));
        assert_eq!(LONGEST_REASON, 1000);
    }

    /// The members of "Studio" as an administrator sees them: the removal of each one.
    fn removals(
        model: &ServerModel,
        scene: &Scene,
    ) -> Vec<(String, Option<Result<(), NoRemoval>>)> {
        let workspace = model.workspace(scene.studio).unwrap();
        let page = members(scene.server_id, model, workspace, &known());
        let MemberList::Loaded(rows) = page.members else {
            panic!("the members are not loaded");
        };
        rows.into_iter().map(|row| (row.name, row.remove)).collect()
    }

    #[test]
    fn an_administrator_sees_before_trying_whether_a_member_can_be_removed_from_a_workspace() {
        let administrators = |name: &str, position: i32| ServerRole {
            position,
            id: ServerRoleId::generate(),
            name: name.to_owned(),
            permissions: vec![ServerPermission::Administrator],
        };
        let (chiefs, admins) = (administrators("Chiefs", 3), administrators("Admins", 2));
        let clerks = server_role("Clerks", 5);
        let mut scene = scene();
        scene.snapshot.account.server_role_ids = vec![admins.id];
        scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
        let leads = WorkspaceRole {
            position: 9,
            id: WorkspaceRoleId::generate(),
            workspace_id: scene.studio,
            name: "Leads".to_owned(),
            permissions: Vec::new(),
            is_everyone: false,
        };
        scene.snapshot.workspaces[0].roles = vec![leads.clone()];
        let own = AccountSummary {
            id: scene.own,
            server_role_ids: vec![admins.id],
            ..account("Luca", AccountStatus::Active)
        };
        let ana = account("Ana", AccountStatus::Active);
        let bea = AccountSummary {
            server_role_ids: vec![chiefs.id],
            ..account("Bea", AccountStatus::Active)
        };
        let cid = AccountSummary {
            server_role_ids: vec![admins.id],
            ..account("Cid", AccountStatus::Active)
        };
        // A server role without the administrator permission gives no place in a
        // workspace, whatever its position.
        let dan = AccountSummary {
            server_role_ids: vec![clerks.id],
            ..account("Dan", AccountStatus::Active)
        };
        let zoe = AccountSummary {
            is_owner: true,
            ..account("Zoe", AccountStatus::Active)
        };
        let accounts = vec![
            own.clone(),
            ana.clone(),
            bea.clone(),
            cid.clone(),
            dan.clone(),
            zoe.clone(),
        ];
        let listed = vec![
            member(ana.id, "Ana", vec![leads.id]),
            member(bea.id, "Bea", Vec::new()),
            member(cid.id, "Cid", Vec::new()),
            member(dan.id, "Dan", Vec::new()),
            member(scene.own, "Luca", Vec::new()),
            member(zoe.id, "Zoe", Vec::new()),
        ];
        let roles = vec![chiefs.clone(), admins.clone(), clerks.clone()];
        let with = |accounts: Collection<Vec<AccountSummary>>,
                    server_roles: Collection<Vec<ServerRole>>| {
            let mut model = scene.model(AdminModel {
                accounts,
                server_roles,
                ..AdminModel::default()
            });
            set_members(&mut model, 0, Loadable::Loaded(listed.clone()));
            removals(&model, &scene)
        };
        let named = |rows: &[(String, Option<Result<(), NoRemoval>>)], name: &str| {
            rows.iter().find(|(held, _)| held == name).unwrap().1
        };

        let rows = with(
            Collection::Loaded(accounts.clone()),
            Collection::Loaded(roles.clone()),
        );
        // A member with a role of the workspace stands below every administrator.
        assert_eq!(named(&rows, "Ana"), Some(Ok(())));
        assert_eq!(named(&rows, "Dan"), Some(Ok(())));
        // An administrator at a higher position, and one at the same position.
        assert_eq!(named(&rows, "Bea"), Some(Err(NoRemoval::Outranked)));
        assert_eq!(named(&rows, "Cid"), Some(Err(NoRemoval::Outranked)));
        assert_eq!(named(&rows, "Luca"), Some(Err(NoRemoval::OwnAccount)));
        assert_eq!(named(&rows, "Zoe"), Some(Err(NoRemoval::Owner)));

        // While the accounts or the order of the roles are read, the page cannot tell.
        let rows = with(Collection::Loading, Collection::Loaded(roles.clone()));
        assert_eq!(named(&rows, "Ana"), Some(Err(NoRemoval::OrderNotRead)));
        assert_eq!(named(&rows, "Luca"), Some(Err(NoRemoval::OwnAccount)));
        let rows = with(Collection::Loaded(accounts.clone()), Collection::Loading);
        assert_eq!(named(&rows, "Ana"), Some(Err(NoRemoval::OrderNotRead)));
        assert_eq!(named(&rows, "Zoe"), Some(Err(NoRemoval::Owner)));
        // A read that failed leaves the decision to the server.
        let rows = with(
            Collection::Failed(LoadFailure::Unavailable),
            Collection::Loaded(roles.clone()),
        );
        assert_eq!(named(&rows, "Bea"), Some(Ok(())));
        let rows = with(
            Collection::Loaded(accounts.clone()),
            Collection::Failed(LoadFailure::Unavailable),
        );
        assert_eq!(named(&rows, "Bea"), Some(Ok(())));
        assert_eq!(named(&rows, "Zoe"), Some(Err(NoRemoval::Owner)));
        // A member that the list of the accounts does not hold yet.
        let rows = with(
            Collection::Loaded(vec![own.clone(), zoe.clone()]),
            Collection::Loaded(roles.clone()),
        );
        assert_eq!(named(&rows, "Ana"), Some(Err(NoRemoval::OrderNotRead)));

        // The server owner stands above every member.
        scene.snapshot.account.is_owner = true;
        scene.snapshot.account.server_role_ids = Vec::new();
        let zoe_now = AccountSummary {
            is_owner: false,
            ..zoe.clone()
        };
        let own_now = AccountSummary {
            is_owner: true,
            server_role_ids: Vec::new(),
            ..own.clone()
        };
        let mut model = scene.model(AdminModel {
            accounts: Collection::Loaded(vec![own_now, ana, bea, cid, dan, zoe_now]),
            ..AdminModel::default()
        });
        set_members(&mut model, 0, Loadable::Loaded(listed.clone()));
        let rows = removals(&model, &scene);
        for name in ["Ana", "Bea", "Cid", "Dan", "Zoe"] {
            assert_eq!(named(&rows, name), Some(Ok(())), "{name}");
        }
        assert_eq!(named(&rows, "Luca"), Some(Err(NoRemoval::OwnAccount)));
    }

    #[test]
    fn a_person_who_is_no_administrator_is_offered_no_removal_from_a_workspace() {
        let scene = scene();
        let ana = account("Ana", AccountStatus::Active);
        let mut model = scene.model(with_accounts(vec![ana.clone()]));
        set_members(
            &mut model,
            0,
            Loadable::Loaded(vec![member(ana.id, "Ana", Vec::new())]),
        );
        assert_eq!(removals(&model, &scene), [("Ana".to_owned(), None)]);
    }

    #[test]
    fn a_member_whose_list_entry_describes_a_picture_is_marked() {
        use junto_client_core::protocol::account::{Avatar, ImageType};
        let scene = scene();
        let (ana, bea) = (AccountId::generate(), AccountId::generate());
        let mut model = scene.model(AdminModel::default());
        let mut pictured = member(ana, "Ana", Vec::new());
        pictured.avatar = Some(Avatar {
            hash: "a".repeat(64),
            media_type: ImageType::Png,
            bytes: 1024,
            width: 64,
            height: 64,
        });
        set_members(
            &mut model,
            0,
            Loadable::Loaded(vec![pictured, member(bea, "Bea", Vec::new())]),
        );
        let workspace = model.workspace(scene.studio).unwrap();
        let page = members(scene.server_id, &model, workspace, &known());
        let MemberList::Loaded(rows) = page.members else {
            panic!("the members are not loaded");
        };
        let marks: Vec<(&str, bool)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.pictured))
            .collect();
        assert_eq!(marks, [("Ana", true), ("Bea", false)]);
    }

    #[test]
    fn the_creator_of_an_invite_is_named_by_the_state_and_otherwise_by_the_server() {
        let scene = scene();
        let ana = account("Ana", AccountStatus::Active);
        let creator = |model: &ServerModel| {
            let page = invites(scene.server_id, model, &known(), &AdminState::default());
            page.invites.loaded().unwrap()[0].creator.clone()
        };
        let stated = |name: Option<&str>| Invite {
            created_by_name: name.map(str::to_owned),
            ..invite(ana.id)
        };
        // The state holds the account: its name of now is shown, not the one that the
        // invite carried when it was created.
        let model = scene.model(AdminModel {
            accounts: Collection::Loaded(vec![ana.clone()]),
            invites: Collection::Loaded(vec![stated(Some("Ana before"))]),
            ..AdminModel::default()
        });
        assert_eq!(creator(&model).as_deref(), Some("Ana"));
        // The state does not hold the account: the name that the server states.
        let model = scene.model(with_invites(vec![stated(Some("Ana Maria"))]));
        assert_eq!(creator(&model).as_deref(), Some("Ana Maria"));
        // An invite of an earlier server states none.
        let model = scene.model(with_invites(vec![stated(None)]));
        assert_eq!(creator(&model), None);
        // The name is text of another party.
        let model = scene.model(with_invites(vec![stated(Some("Bo\nb\u{7}"))]));
        assert_eq!(creator(&model).as_deref(), Some("Bob"));
        let model = scene.model(with_invites(vec![stated(Some(" \n "))]));
        assert_eq!(creator(&model), None);
        let long = "n".repeat(200);
        let model = scene.model(with_invites(vec![stated(Some(&long))]));
        assert_eq!(creator(&model).map(|name| name.chars().count()), Some(64));
    }
}
