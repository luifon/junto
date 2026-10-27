//! The settings of one person (ADR 0306, ADR 0761): the workspace permissions that are
//! allowed to or denied to a member, what the member can use in each channel that the
//! person sees, and the server permissions of an account.

use junto_client_core::model::{Collection, ServerModel, WorkspaceModel, outranks};
use junto_client_core::protocol::ids::{AccountId, CategoryId, ChannelId, WorkspaceId};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::Listed;
use super::overrides::{Choice, Locked, PermissionLine, member_lock};
use crate::state::admin::PersonPage;

/// The workspace permissions of one member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberPermissionsPage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    /// In the order of `WorkspacePermission::ALL`.
    pub lines: Listed<Vec<PermissionLine>>,
    pub locked: Option<Locked>,
}

/// What a member can use in one channel that the person sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLine {
    pub channel_id: ChannelId,
    pub name: String,
    pub kind: ChannelKind,
    pub category: Option<String>,
    /// In the order of `WorkspacePermission::ALL`. Empty: the member does not see the
    /// channel.
    pub permissions: Vec<WorkspacePermission>,
}

/// What a member can use in the channels of a workspace that the person sees (ADR 0304).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessPage {
    pub workspace_name: String,
    /// In the order of the channel list: the channels without a category, then each
    /// category.
    pub channels: Listed<Vec<AccessLine>>,
}

/// The server permissions of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPermissionsPage {
    /// In the order of `ServerPermission::ALL`.
    pub lines: Listed<Vec<PermissionLine<ServerPermission>>>,
    pub locked: Option<Locked>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonPageView {
    Permissions(MemberPermissionsPage),
    Access(AccessPage),
    Server(ServerPermissionsPage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonSettings {
    pub account_id: AccountId,
    pub name: String,
    /// The pages of the settings, in their order.
    pub pages: Vec<PersonPage>,
    pub selected: PersonPage,
    pub page: PersonPageView,
}

impl PersonSettings {
    /// The page of the administration that lists the person: the members of a workspace,
    /// or the people of the server.
    pub const fn listed_on(&self) -> crate::state::admin::AdminPage {
        match self.selected {
            PersonPage::Permissions(_) | PersonPage::Access(_) => {
                crate::state::admin::AdminPage::Members
            }
            PersonPage::Server => crate::state::admin::AdminPage::People,
        }
    }
}

/// The name of a member of a workspace, while its members are read.
fn member_name(workspace: &WorkspaceModel, account_id: AccountId) -> Option<String> {
    workspace
        .members
        .loaded()?
        .iter()
        .find(|member| member.account_id == account_id)
        .map(|member| member.name.clone())
}

fn member_page(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    account_id: AccountId,
) -> MemberPermissionsPage {
    let workspace_id = workspace.workspace.id;
    let lines = Listed::of(
        model.admin.member_permissions(workspace_id, account_id),
        |held| {
            WorkspacePermission::ALL
                .iter()
                .copied()
                .map(|permission| PermissionLine {
                    permission,
                    current: if held.deny.contains(&permission) {
                        Choice::Deny
                    } else if held.allow.contains(&permission) {
                        Choice::Allow
                    } else {
                        Choice::Neutral
                    },
                    // The ceiling in the workspace; the server checks each channel too
                    // (ADR 0306).
                    held: workspace.holds(permission) || model.is_administrator(),
                    owner_denies: false,
                })
                .collect()
        },
    );
    MemberPermissionsPage {
        workspace_id,
        workspace_name: workspace.workspace.name.clone(),
        lines,
        locked: member_lock(model, workspace, account_id, false),
    }
}

fn category_name(workspace: &WorkspaceModel, category_id: Option<CategoryId>) -> Option<String> {
    let category_id = category_id?;
    workspace
        .categories
        .iter()
        .find(|category| category.id == category_id)
        .map(|category| category.name.clone())
}

fn access_page(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    account_id: AccountId,
) -> AccessPage {
    let workspace_id = workspace.workspace.id;
    let channels = Listed::of(
        model.admin.member_access(workspace_id, account_id),
        |listed| {
            // The channels of the state that the answer names: a channel that the state does
            // not hold is not named to the person, whatever the server answered.
            let mut lines: Vec<(Option<i32>, i32, AccessLine)> = workspace
                .channels
                .iter()
                .filter_map(|held| {
                    let entry = listed
                        .iter()
                        .find(|entry| entry.channel_id == held.channel.id)?;
                    let category_position = held.channel.category_id.and_then(|category_id| {
                        workspace
                            .categories
                            .iter()
                            .find(|category| category.id == category_id)
                            .map(|category| category.position)
                    });
                    Some((
                        category_position,
                        held.channel.position,
                        AccessLine {
                            channel_id: held.channel.id,
                            name: held.channel.name.clone(),
                            kind: held.channel.kind,
                            category: category_name(workspace, held.channel.category_id),
                            permissions: WorkspacePermission::ALL
                                .iter()
                                .copied()
                                .filter(|permission| entry.permissions.contains(permission))
                                .collect(),
                        },
                    ))
                })
                .collect();
            lines.sort_by_key(|(category, position, line)| {
                (*category, *position, line.channel_id.as_uuid().as_u128())
            });
            lines.into_iter().map(|(_, _, line)| line).collect()
        },
    );
    AccessPage {
        workspace_name: workspace.workspace.name.clone(),
        channels,
    }
}

/// Whether role order lets the person change the server permissions of an account: the
/// owner takes no rule, and the person must stand above the account (ADR 0306, ADR 0309).
fn server_lock(model: &ServerModel, account_id: AccountId) -> Option<Locked> {
    let account = match &model.admin.accounts {
        Collection::Loaded(accounts) => accounts.iter().find(|account| account.id == account_id),
        Collection::Failed(_) => None,
        Collection::NotLoaded | Collection::Loading => return Some(Locked::OrderNotRead),
    };
    if account.is_some_and(|account| account.is_owner)
        || (account_id == model.account.id && model.account.is_owner)
    {
        return Some(Locked::Owner);
    }
    if account_id == model.account.id {
        return Some(Locked::OwnAccount);
    }
    if model.account.is_owner {
        return None;
    }
    let Some(account) = account else {
        // A read that failed leaves the decision to the server.
        return None;
    };
    match (model.server_rank(), model.server_rank_of(account)) {
        (Some(person), Some(target)) if outranks(person, target) => None,
        (Some(_), Some(_)) => Some(Locked::Outranked),
        _ if matches!(model.admin.server_roles, Collection::Failed(_)) => None,
        _ => Some(Locked::OrderNotRead),
    }
}

fn server_page(model: &ServerModel, account_id: AccountId) -> ServerPermissionsPage {
    let owner = model.account.is_owner;
    let lines = Listed::of(model.admin.server_permissions(account_id), |held| {
        ServerPermission::ALL
            .iter()
            .copied()
            .map(|permission| PermissionLine {
                permission,
                current: if held.deny.contains(&permission) {
                    Choice::Deny
                } else if held.allow.contains(&permission) {
                    Choice::Allow
                } else {
                    Choice::Neutral
                },
                // The page is an administrator's, who holds every server permission.
                held: true,
                owner_denies: permission == ServerPermission::Administrator && !owner,
            })
            .collect()
    });
    ServerPermissionsPage {
        lines,
        locked: server_lock(model, account_id),
    }
}

/// The name of an account: from the list of the accounts, or from the members of a
/// workspace.
fn account_name(model: &ServerModel, account_id: AccountId) -> Option<String> {
    if account_id == model.account.id {
        return Some(model.account.name.clone());
    }
    model
        .admin
        .accounts
        .loaded()
        .and_then(|accounts| accounts.iter().find(|account| account.id == account_id))
        .map(|account| account.name.clone())
        .or_else(|| {
            model
                .workspaces
                .iter()
                .find_map(|workspace| member_name(workspace, account_id))
        })
}

/// The settings of a person. Absent when the workspace of a page of a member is gone, and
/// for a page that the person may not use: the pages of a member need "manage roles" in the
/// workspace, the server permissions the administrator permission.
pub fn settings(
    model: &ServerModel,
    account_id: AccountId,
    selected: PersonPage,
) -> Option<PersonSettings> {
    let name = account_name(model, account_id).unwrap_or_default();
    let (pages, page) = match selected {
        PersonPage::Permissions(workspace_id) | PersonPage::Access(workspace_id) => {
            let workspace = model
                .workspace(workspace_id)
                .filter(|workspace| workspace.holds(WorkspacePermission::ManageRoles))?;
            let page = match selected {
                PersonPage::Access(_) => {
                    PersonPageView::Access(access_page(model, workspace, account_id))
                }
                _ => PersonPageView::Permissions(member_page(model, workspace, account_id)),
            };
            let name = member_name(workspace, account_id).unwrap_or(name.clone());
            return Some(PersonSettings {
                account_id,
                name,
                pages: vec![
                    PersonPage::Permissions(workspace_id),
                    PersonPage::Access(workspace_id),
                ],
                selected,
                page,
            });
        }
        PersonPage::Server => {
            if !model.is_administrator() {
                return None;
            }
            (
                vec![PersonPage::Server],
                PersonPageView::Server(server_page(model, account_id)),
            )
        }
    };
    Some(PersonSettings {
        account_id,
        name,
        pages,
        selected,
        page,
    })
}
