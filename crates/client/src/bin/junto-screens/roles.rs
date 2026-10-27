//! The screens of the roles (ADR 0791): the roles of a workspace and of the server in
//! their order, for a person below some of them, for the owner and for a person without a
//! role; the form, the question and the permissions of a role; the roles of an account in
//! the lists of people; and an account that role order protects from a moderator.

use std::sync::Arc;

use junto_client::shell::Shell;
use junto_client::shell::admin::roles::Open;
use junto_client::state::admin::{AdminPage, AskKind, RoleLevel, RoleRef};
use junto_client::state::view::ServerSnapshot;
use junto_client_core::model::{Collection, LoadFailure, ServerModel};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{ServerRoleId, WorkspaceRoleId};
use junto_client_core::protocol::roles::{ServerRole, WorkspaceRole};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::admin::{accounts, collections, open_page, refused, servers, tell};
use super::{Screen, Studio, id, person};

const STUDIO: u32 = 10;
const LEADS: u32 = 21;
const ENGINEERS: u32 = 22;
const GUESTS: u32 = 23;
const OWNERS_CIRCLE: u32 = 24;
const MODERATORS: u32 = 50;
const INVITERS: u32 = 51;
const STEWARDS: u32 = 52;
const FOUNDERS: u32 = 53;

fn workspace_role(number: u32) -> RoleRef {
    RoleRef::Workspace(id(STUDIO), id(number))
}

/// "Studio" with the roles "Board" (4), "Leads" (3), "Engineers" (2) and "Guests" (1).
/// Luca holds the roles `held`, and the server states the permissions that they give.
fn with_roles(model: &mut ServerModel, held: &[u32]) {
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    let workspace_id = studio.workspace.id;
    let role =
        |number: u32, name: &str, position, permissions: &[WorkspacePermission]| WorkspaceRole {
            position,
            id: id(number),
            workspace_id,
            name: name.to_owned(),
            permissions: permissions.to_vec(),
            is_everyone: false,
        };
    let everyone = studio.roles[0].clone();
    studio.roles = vec![
        everyone,
        role(
            OWNERS_CIRCLE,
            "Board",
            4,
            &[
                WorkspacePermission::ManageRoles,
                WorkspacePermission::ManageChannel,
                WorkspacePermission::ViewAuditEntries,
            ],
        ),
        role(
            LEADS,
            "Leads",
            3,
            &[
                WorkspacePermission::ManageRoles,
                WorkspacePermission::ManageMessages,
                WorkspacePermission::MentionEveryone,
                WorkspacePermission::RecordCalls,
            ],
        ),
        role(
            ENGINEERS,
            "Engineers",
            2,
            &[
                WorkspacePermission::ManageMessages,
                WorkspacePermission::RecordCalls,
            ],
        ),
        role(GUESTS, "Guests", 1, &[]),
    ];
    studio.own_role_ids = held
        .iter()
        .map(|number| id::<WorkspaceRoleId>(*number))
        .collect();
}

/// "Acme Studio" for Luca as a holder of "Leads".
fn as_lead() -> Vec<ServerSnapshot> {
    servers(|model| with_roles(model, &[LEADS, GUESTS]))
}

fn server_roles() -> Vec<ServerRole> {
    let role = |number: u32, name: &str, position, permissions: &[ServerPermission]| ServerRole {
        position,
        id: id(number),
        name: name.to_owned(),
        permissions: permissions.to_vec(),
    };
    vec![
        role(FOUNDERS, "Founders", 4, &[ServerPermission::Administrator]),
        role(STEWARDS, "Stewards", 3, &[ServerPermission::Administrator]),
        role(
            MODERATORS,
            "Moderators",
            2,
            &[
                ServerPermission::BanMembers,
                ServerPermission::ApproveJoinRequests,
            ],
        ),
        role(INVITERS, "Inviters", 1, &[ServerPermission::CreateInvites]),
    ]
}

/// Luca holds the server roles `held`, with the permissions that they give. Ana owns the
/// server.
fn with_server_roles(model: &mut ServerModel, held: &[u32], roles: Collection<Vec<ServerRole>>) {
    let all = server_roles();
    let held: Vec<ServerRoleId> = held.iter().map(|number| id(*number)).collect();
    let mut permissions: Vec<ServerPermission> = all
        .iter()
        .filter(|role| held.contains(&role.id))
        .flat_map(|role| role.permissions.iter().copied())
        .collect();
    permissions.sort_unstable();
    permissions.dedup();
    model.account.server_role_ids = held;
    model.account.server_permissions = permissions;
    let admin = collections(model);
    admin.server_roles = roles;
    admin.accounts = Collection::Loaded(
        accounts("Ana")
            .into_iter()
            .map(|mut account| {
                account.server_role_ids = match account.name.as_str() {
                    "Bruno" => vec![id(MODERATORS), id(INVITERS)],
                    "Carla" => vec![id(STEWARDS)],
                    "Marta" => vec![id(INVITERS)],
                    _ => Vec::new(),
                };
                account
            })
            .collect(),
    );
}

fn as_steward() -> Vec<ServerSnapshot> {
    servers(|model| {
        with_server_roles(model, &[STEWARDS], Collection::Loaded(server_roles()));
    })
}

fn roles_page(shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>) {
    open_page(shell, AdminPage::Roles, window, cx);
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    // The roles of a workspace for a person who holds "Leads": "Board" and "Leads" are
    // locked, "Engineers" and "Guests" can be changed.
    studio.draw(Screen::new("roles-workspace", as_lead()), roles_page)?;
    studio.draw(
        Screen::new("roles-workspace-permissions", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            shell.show_role_permissions(Some(workspace_role(ENGINEERS)), cx);
        },
    )?;
    studio.draw(
        Screen::new("roles-workspace-permissions-locked", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            shell.show_role_permissions(Some(workspace_role(OWNERS_CIRCLE)), cx);
        },
    )?;
    studio.draw(
        Screen::new("roles-workspace-new", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            shell.set_roles_open(Some(Open::Create), "Reviewers", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("roles-workspace-new-empty", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            shell.set_roles_open(Some(Open::Create), "", window, cx);
            shell.submit_role_form(window, cx);
        },
    )?;
    studio.draw(
        Screen::new("roles-workspace-rename", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            let open = Open::Rename(workspace_role(ENGINEERS));
            shell.set_roles_open(Some(open), "Engineers", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("roles-workspace-delete-question", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            let open = Open::Delete(workspace_role(GUESTS));
            shell.set_roles_open(Some(open), "", window, cx);
        },
    )?;
    // An override of the role denies a permission that the person does not hold in a
    // channel: the server refuses the deletion (ADR 0308).
    studio.draw(
        Screen::new("roles-workspace-delete-refused", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            let role = workspace_role(GUESTS);
            shell.set_roles_open(Some(Open::Delete(role)), "", window, cx);
            let what = AskKind::DeleteRole(role);
            tell(
                shell,
                refused(what, ErrorCode::ExceedsOwnPermissions),
                None,
                cx,
            );
        },
    )?;
    // Another person changed the order while the page was open.
    studio.draw(
        Screen::new("roles-workspace-move-refused", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            let what = AskKind::MoveRole(workspace_role(GUESTS));
            tell(shell, refused(what, ErrorCode::Forbidden), None, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "roles-workspace-owner",
            servers(|model| {
                with_roles(model, &[]);
                model.account.is_owner = true;
            }),
        ),
        roles_page,
    )?;
    // A person who manages roles by the "everyone" role and holds no role: nothing is
    // below the person.
    studio.draw(
        Screen::new(
            "roles-workspace-no-role",
            servers(|model| {
                with_roles(model, &[]);
                let studio = Arc::make_mut(&mut model.workspaces[0]);
                studio.roles[0]
                    .permissions
                    .push(WorkspacePermission::ManageRoles);
            }),
        ),
        roles_page,
    )?;
    // An administrator stands at a server role whose position is on its way.
    for (name, roles) in [
        ("roles-workspace-reading", Collection::Loading),
        (
            "roles-workspace-failed",
            Collection::Failed(LoadFailure::NoAnswer),
        ),
        (
            "roles-workspace-administrator",
            Collection::Loaded(server_roles()),
        ),
    ] {
        studio.draw(
            Screen::new(
                name,
                servers(move |model| {
                    with_roles(model, &[]);
                    with_server_roles(model, &[STEWARDS], roles);
                }),
            ),
            roles_page,
        )?;
    }

    // The server roles for an administrator who holds "Stewards".
    let server_page =
        |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            open_page(shell, AdminPage::ServerRoles, window, cx);
        };
    studio.draw(Screen::new("roles-server", as_steward()), server_page)?;
    studio.draw(
        Screen::new("roles-server-permissions", as_steward()),
        |shell, window, cx| {
            open_page(shell, AdminPage::ServerRoles, window, cx);
            shell.show_role_permissions(Some(RoleRef::Server(id(MODERATORS))), cx);
        },
    )?;
    // The pages of the roles after an action: a role created, renamed and deleted, and the
    // server roles of a server that has none.
    let answer = |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
        shell.answer_open_admin_form(cx);
    };
    studio.draw_answered(
        Screen::new("roles-workspace-created", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            shell.set_roles_open(Some(Open::Create), "Reviewers", window, cx);
        },
        answer,
    )?;
    studio.draw_answered(
        Screen::new("roles-workspace-renamed", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            let open = Open::Rename(workspace_role(ENGINEERS));
            shell.set_roles_open(Some(open), "Engineers", window, cx);
        },
        answer,
    )?;
    studio.draw_answered(
        Screen::new("roles-workspace-deleted", as_lead()),
        |shell, window, cx| {
            roles_page(shell, window, cx);
            let open = Open::Delete(workspace_role(GUESTS));
            shell.set_roles_open(Some(open), "", window, cx);
        },
        answer,
    )?;
    studio.draw_answered(
        Screen::new("roles-server-created", as_steward()),
        |shell, window, cx| {
            open_page(shell, AdminPage::ServerRoles, window, cx);
            shell.set_roles_open(Some(Open::Create), "Recruiters", window, cx);
        },
        answer,
    )?;
    studio.draw(
        Screen::new(
            "roles-server-empty",
            servers(|model| {
                with_server_roles(model, &[STEWARDS], Collection::Loaded(Vec::new()));
            }),
        ),
        server_page,
    )?;
    studio.draw(
        Screen::new("roles-server-new", as_steward()),
        |shell, window, cx| {
            open_page(shell, AdminPage::ServerRoles, window, cx);
            shell.set_roles_open(Some(Open::Create), "Recruiters", window, cx);
            let what = AskKind::CreateRole(RoleLevel::Server);
            tell(shell, refused(what, ErrorCode::LimitReached), None, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "roles-server-reading",
            servers(|model| with_server_roles(model, &[STEWARDS], Collection::Loading)),
        ),
        server_page,
    )?;
    studio.draw(
        Screen::new(
            "roles-server-owner",
            servers(|model| {
                with_server_roles(model, &[], Collection::Loaded(server_roles()));
                model.account.is_owner = true;
                // Luca owns the server on this screen: no account of the list does.
                if let Collection::Loaded(accounts) = &mut collections(model).accounts {
                    for account in accounts {
                        account.is_owner = false;
                    }
                }
            }),
        ),
        server_page,
    )?;

    // The people of the server for a moderator: an account at or above the moderator
    // states why it takes no removal and no ban.
    let as_moderator = || {
        servers(|model| {
            with_server_roles(model, &[MODERATORS], Collection::Loaded(server_roles()));
        })
    };
    studio.draw(
        Screen::new("roles-people-outranked", as_moderator()),
        |shell, window, cx| open_page(shell, AdminPage::People, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "roles-people-order-reading",
            servers(|model| with_server_roles(model, &[MODERATORS], Collection::Loading)),
        ),
        |shell, window, cx| open_page(shell, AdminPage::People, window, cx),
    )?;
    // The server roles of an account, for an administrator.
    studio.draw(
        Screen::new("roles-people-roles", as_steward()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            shell.show_account_roles(Some(person("Bruno")), cx);
        },
    )?;
    studio.draw(
        Screen::new("roles-people-roles-refused", as_steward()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            shell.show_account_roles(Some(person("Marta")), cx);
            let what = AskKind::AssignRole(RoleRef::Server(id(MODERATORS)), person("Marta"));
            tell(shell, refused(what, ErrorCode::LimitReached), None, cx);
        },
    )?;
    // The roles of a member of the workspace, for a person who manages roles.
    studio.draw(
        Screen::new("roles-members-roles", as_lead()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Members, window, cx);
            shell.show_account_roles(Some(person("Bruno")), cx);
        },
    )
}
