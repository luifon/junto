//! The pages of the roles in the window against the real server (ADR 0791): a person who
//! manages roles creates, orders, renames, gives, takes and deletes them, and the window
//! of another member follows; a moderator sees why an account with a higher role cannot
//! be banned before trying.

use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use junto_client::state::admin::view::people::{Blocked, MemberList};
use junto_client::state::admin::view::roles::{
    Fixed, Locked, NoAssignment, RoleChoice, RoleRow, RolesPage, Standing,
};
use junto_client::state::admin::view::{Scope, ServerPage};
use junto_client::state::admin::{Ask, AskKind, AskState, RolePermission, RoleRef};
use junto_client::text::admin::FORM_SENT;
use junto_client_core::ServerHandle;
use junto_client_core::api::ErrorCategory;
use junto_client_core::protocol::account::AccountStatus;
use junto_client_core::protocol::ids::{AccountId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::roles::{
    CreateServerRole, CreateWorkspaceRole, ServerRolePosition, WorkspaceRole,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use crate::admin_pages::{
    account, awaited, click_in_page, default_workspace, idle, invite, joined, open_page,
    owner_window_at, sel, try_click, until_model,
};
use crate::harness::{Admin, Window, World, member, named, selector};
use crate::messages::document;
use crate::support::{Fault, FaultProxy, logged_in};

/// The page of the roles that the administration shows.
fn roles_page(window: &mut Window) -> Option<RolesPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Roles(page),
            ..
        }) => Some(page),
        _ => None,
    }
}

/// The rows of the roles page, once the roles are read.
fn role_rows(window: &mut Window) -> Option<Vec<RoleRow>> {
    roles_page(window)?.roles.loaded().cloned()
}

/// The names of the roles in the order of the page.
fn role_names(window: &mut Window) -> Vec<String> {
    role_rows(window)
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.name)
        .collect()
}

fn role_row(window: &mut Window, role: RoleRef) -> Option<RoleRow> {
    role_rows(window)?.into_iter().find(|row| row.role == role)
}

/// The reason that the switch of a permission of a role is drawn with, when it takes no
/// press.
fn permission_held(
    window: &mut Window,
    role: RoleRef,
    permission: RolePermission,
) -> Option<String> {
    let view = window.admin()?;
    let row = role_row(window, role)?;
    window
        .in_shell(|shell, _, _| shell.role_permission_held(&view, &row, permission))
        .map(|reason| reason.to_string())
}

/// The client of the server of a window.
fn handle_of(window: &mut Window) -> ServerHandle {
    window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0)
}

fn workspace_roles(world: &World, admin: &Admin, workspace_id: WorkspaceId) -> Vec<WorkspaceRole> {
    world
        .run(admin.handle.api().workspace_roles(workspace_id))
        .unwrap()
}

/// The position of a role of a workspace on the server.
fn position_on_server(
    world: &World,
    admin: &Admin,
    workspace_id: WorkspaceId,
    name: &str,
) -> Option<i32> {
    workspace_roles(world, admin, workspace_id)
        .into_iter()
        .find(|role| role.name == name)
        .map(|role| role.position)
}

fn create_workspace_role(
    world: &World,
    admin: &Admin,
    workspace_id: WorkspaceId,
    name: &str,
    permissions: &[WorkspacePermission],
) -> WorkspaceRoleId {
    let role = CreateWorkspaceRole {
        name: name.to_owned(),
        permissions: permissions.to_vec(),
    };
    world
        .run(
            admin
                .handle
                .api()
                .create_workspace_role(workspace_id, &role),
        )
        .unwrap()
        .id
}

/// Types a name into the open form of the roles page and sends it.
fn send_role_form(window: &mut Window, name: &str) {
    window.until("the form of the role is drawn", |window| {
        window.shows("role-name")
    });
    window.in_shell(|shell, window, cx| shell.type_role_name(name, window, cx));
    click_in_page(window, "role-form-send");
}

/// Drags the handle of one role onto the row of another, as the pointer does. Returns
/// whether a drag started.
fn drag_role(window: &mut Window, from: &'static str, to: &'static str) -> bool {
    let start = crate::admin_pages::reveal_in_page(window, from).center();
    window.simulate(|cx| cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none()));
    window.simulate(|cx| {
        cx.simulate_mouse_move(
            start + point(px(0.0), px(12.0)),
            MouseButton::Left,
            Modifiers::none(),
        )
    });
    let started = window.cx.update(|_, cx| cx.has_active_drag());
    let end = crate::admin_pages::reveal_in_page(window, to).center();
    window.simulate(|cx| cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none()));
    window.simulate(|cx| cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none()));
    window.run_tasks();
    started
}

/// The roles that the members page offers for one member.
fn member_choices(window: &mut Window, account_id: AccountId) -> Option<Vec<RoleChoice>> {
    let page = match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Members(page),
            ..
        }) => page,
        _ => return None,
    };
    let MemberList::Loaded(rows) = page.members else {
        return None;
    };
    rows.into_iter()
        .find(|row| row.account_id == account_id)?
        .assignable
}

#[gpui::test]
fn a_person_who_manages_roles_creates_orders_gives_and_takes_them_and_another_window_follows(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let ada = world.owner(&server);
    let workspace_id = default_workspace(&world, &ada.handle);
    // "Chiefs" stands above "Leads": a new role enters at the bottom.
    let chiefs = create_workspace_role(
        &world,
        &ada,
        workspace_id,
        "Chiefs",
        &[WorkspacePermission::ManageChannel],
    );
    let leads = create_workspace_role(
        &world,
        &ada,
        workspace_id,
        "Leads",
        &[
            WorkspacePermission::ManageRoles,
            WorkspacePermission::ManageMessages,
        ],
    );
    let (_bruno_directory, mut window) = member(cx, &world, &server.address(), &ada, "Bruno");
    let (_carla_directory, mut other) = member(cx, &world, &server.address(), &ada, "Carla");
    let (bruno, carla) = (
        logged_in(&handle_of(&mut window)),
        logged_in(&handle_of(&mut other)),
    );
    world
        .run(
            ada.handle
                .api()
                .assign_workspace_role(workspace_id, bruno, leads),
        )
        .unwrap();
    // A message of the owner in the channel that the window of Carla shows.
    let channel_id = other
        .view()
        .workspace()
        .and_then(|workspace| workspace.channel.as_ref().map(|row| row.id))
        .unwrap();
    let sent = ada.send(&world, channel_id, document("The plan for today"));
    let may_delete = |window: &mut Window| {
        window
            .messages()
            .into_iter()
            .find(|row| row.message.id == sent.id)
            .map(|row| (row.can_delete, row.can_pin))
    };
    other.until("the message is drawn", |window| {
        may_delete(window).is_some()
    });
    assert_eq!(may_delete(&mut other), Some((false, false)));

    let role = |role_id| RoleRef::Workspace(workspace_id, role_id);
    window.open_administration();
    open_page(&mut window, "admin-nav-roles", |window| {
        roles_page(window).is_some()
    });
    window.until("the roles are listed in their order", |window| {
        role_names(window) == ["Chiefs", "Leads", "everyone"]
    });
    let page = roles_page(&mut window).unwrap();
    assert_eq!(page.standing, Some(Standing::Role("Leads".to_owned())));
    assert_eq!(page.create, Ok(()));

    // A role at or above the highest role of the person states it in its row, and its
    // actions take no click.
    let row = role_row(&mut window, role(chiefs)).unwrap();
    assert_eq!(row.locked, Some(Locked::Above));
    assert_eq!(row.up, Err(Fixed::Locked(Locked::Above)));
    let own = role_row(&mut window, role(leads)).unwrap();
    assert_eq!(own.locked, Some(Locked::OwnHighest));
    assert!(window.shows(sel("role-locked", chiefs)) && window.shows(sel("role-locked", leads)));
    for action in ["role-delete", "role-rename", "role-up", "role-down"] {
        click_in_page(&mut window, sel(action, chiefs));
        assert!(
            !window.shows("role-confirm") && !window.shows("role-name"),
            "{action} acts on a role above the person"
        );
    }
    assert!(idle(&mut window));
    assert_eq!(
        position_on_server(&world, &ada, workspace_id, "Chiefs"),
        Some(2)
    );

    // A new role enters at the bottom, above the "everyone" role.
    click_in_page(&mut window, "role-new");
    send_role_form(&mut window, "Helpers");
    window.until("the new role is the lowest one", |window| {
        role_names(window) == ["Chiefs", "Leads", "Helpers", "everyone"] && idle(window)
    });
    let helpers = match role_rows(&mut window).unwrap()[2].role {
        RoleRef::Workspace(_, role_id) => role_id,
        RoleRef::Server(_) => panic!("a server role on the page of a workspace"),
    };
    let positions: Vec<i32> = role_rows(&mut window)
        .unwrap()
        .iter()
        .map(|row| row.position)
        .collect();
    assert_eq!(positions, [3, 2, 1, 0]);
    assert_eq!(
        position_on_server(&world, &ada, workspace_id, "Helpers"),
        Some(1)
    );
    assert!(!window.shows("role-name"));

    // The permissions of the role: one that the person holds is given, and one that the
    // person does not hold is a disabled switch with its reason.
    click_in_page(&mut window, sel("role-permissions", helpers));
    window.until("the permissions are shown", |window| {
        window.shows("role-permission-manage_messages")
    });
    assert!(window.shows("role-ceiling-manage_channel"));
    assert!(!window.shows("role-ceiling-manage_messages"));
    click_in_page(&mut window, "role-permission-manage_channel");
    click_in_page(&mut window, "role-permission-manage_messages");
    let granted = |window: &mut Window| -> Vec<RolePermission> {
        role_row(window, role(helpers))
            .map(|row| {
                row.permissions
                    .iter()
                    .filter(|switch| switch.on)
                    .map(|switch| switch.permission)
                    .collect()
            })
            .unwrap_or_default()
    };
    window.until("the role holds the permission", |window| {
        granted(window)
            == [RolePermission::Workspace(
                WorkspacePermission::ManageMessages,
            )]
            && idle(window)
    });
    let stored = workspace_roles(&world, &ada, workspace_id)
        .into_iter()
        .find(|held| held.id == helpers)
        .unwrap();
    assert_eq!(stored.permissions, [WorkspacePermission::ManageMessages]);

    // Two further permissions are given one directly after the other: each is one request
    // that names its permission, so the second does not wait for the first and neither
    // undoes the other (ADR 0311).
    let free: Vec<WorkspacePermission> = role_row(&mut window, role(helpers))
        .unwrap()
        .permissions
        .iter()
        .filter(|switch| !switch.on && !switch.beyond_own)
        .filter_map(|switch| match switch.permission {
            RolePermission::Workspace(permission) => Some(permission),
            RolePermission::Server(_) => None,
        })
        .take(2)
        .collect();
    assert_eq!(free.len(), 2, "the person holds two further permissions");
    for permission in &free {
        click_in_page(
            &mut window,
            selector(format!("role-permission-{}", permission.name())),
        );
    }
    let mut wanted = vec![WorkspacePermission::ManageMessages];
    wanted.extend(free.iter().copied());
    wanted.sort_by_key(|permission| {
        WorkspacePermission::ALL
            .iter()
            .position(|held| held == permission)
    });
    // The two changes are sent at the same time and their replies can arrive in either
    // order. `idle` follows only the newest request, and a switch whose change the state
    // does not hold yet is drawn held and takes no click, so the clicks below wait until
    // no change is held.
    window.until("the role holds the three permissions", |window| {
        granted(window)
            == wanted
                .iter()
                .copied()
                .map(RolePermission::Workspace)
                .collect::<Vec<_>>()
            && idle(window)
            && window
                .admin()
                .is_some_and(|view| view.changing_roles.is_empty())
    });
    let stored = workspace_roles(&world, &ada, workspace_id)
        .into_iter()
        .find(|held| held.id == helpers)
        .unwrap();
    let mut held = stored.permissions.clone();
    held.sort_by_key(|permission| {
        WorkspacePermission::ALL
            .iter()
            .position(|of| of == permission)
    });
    assert_eq!(held, wanted);
    for permission in &free {
        click_in_page(
            &mut window,
            selector(format!("role-permission-{}", permission.name())),
        );
    }
    window.until("the role holds one permission again", |window| {
        granted(window)
            == [RolePermission::Workspace(
                WorkspacePermission::ManageMessages,
            )]
            && idle(window)
    });
    click_in_page(&mut window, sel("role-permissions", helpers));

    // A second role, and the order: one place up with its button, which is the path of
    // the keyboard, and back by dragging.
    click_in_page(&mut window, "role-new");
    send_role_form(&mut window, "Guests");
    window.until("the second role is the lowest one", |window| {
        role_names(window) == ["Chiefs", "Leads", "Helpers", "Guests", "everyone"] && idle(window)
    });
    let guests = match role_rows(&mut window).unwrap()[3].role {
        RoleRef::Workspace(_, role_id) => role_id,
        RoleRef::Server(_) => panic!("a server role on the page of a workspace"),
    };
    let row = role_row(&mut window, role(guests)).unwrap();
    assert_eq!((row.up, row.down), (Ok(2), Err(Fixed::NoPlaceBelow)));
    // The place above "Helpers" is the highest role of the person: no move leads there.
    let row = role_row(&mut window, role(helpers)).unwrap();
    assert_eq!((row.up, row.down), (Err(Fixed::NoPlaceAbove), Ok(3)));
    click_in_page(&mut window, sel("role-up", guests));
    window.until("the role moved one place up", |window| {
        role_names(window) == ["Chiefs", "Leads", "Guests", "Helpers", "everyone"] && idle(window)
    });
    assert_eq!(
        (
            position_on_server(&world, &ada, workspace_id, "Guests"),
            position_on_server(&world, &ada, workspace_id, "Helpers"),
            position_on_server(&world, &ada, workspace_id, "Leads"),
        ),
        (Some(2), Some(1), Some(3))
    );

    // A drop on a row that the person cannot move a role to changes nothing.
    assert!(drag_role(
        &mut window,
        sel("role-handle", helpers),
        sel("role-row", chiefs)
    ));
    assert!(idle(&mut window));
    assert_eq!(
        role_names(&mut window),
        ["Chiefs", "Leads", "Guests", "Helpers", "everyone"]
    );
    // A role above the person has no handle that starts a drag.
    assert!(!drag_role(
        &mut window,
        sel("role-handle", chiefs),
        sel("role-row", guests)
    ));
    assert!(drag_role(
        &mut window,
        sel("role-handle", helpers),
        sel("role-row", guests)
    ));
    window.until("the dragged role took the place", |window| {
        role_names(window) == ["Chiefs", "Leads", "Helpers", "Guests", "everyone"] && idle(window)
    });
    assert_eq!(
        (
            position_on_server(&world, &ada, workspace_id, "Helpers"),
            position_on_server(&world, &ada, workspace_id, "Guests"),
        ),
        (Some(2), Some(1))
    );

    // A new name.
    click_in_page(&mut window, sel("role-rename", guests));
    send_role_form(&mut window, "Visitors");
    window.until("the role has its new name", |window| {
        role_names(window) == ["Chiefs", "Leads", "Helpers", "Visitors", "everyone"] && idle(window)
    });

    // The role is given to a member on the members page, and the window of that member
    // offers what the role allows, without an act of that person.
    open_page(&mut window, "admin-nav-members", |window| {
        member_choices(window, carla).is_some()
    });
    let choices = member_choices(&mut window, carla).unwrap();
    let names: Vec<&str> = choices.iter().map(|choice| choice.name.as_str()).collect();
    assert_eq!(names, ["Chiefs", "Leads", "Helpers", "Visitors"]);
    assert_eq!(
        choices[0].blocked,
        Some(NoAssignment::Locked(Locked::Above))
    );
    assert_eq!(
        choices[1].blocked,
        Some(NoAssignment::Locked(Locked::OwnHighest))
    );
    assert!(choices[2].blocked.is_none() && !choices[2].held);
    click_in_page(&mut window, sel("member-roles-toggle", carla));
    window.until("the roles of the member are drawn", |window| {
        window.shows(sel("account-roles", carla))
    });
    // A role above the person states why it is not given, and takes no click.
    let above = sel(&format!("account-role-held-{carla}"), chiefs);
    assert!(window.shows(above));
    click_in_page(&mut window, sel(&format!("account-role-{carla}"), chiefs));
    click_in_page(&mut window, sel(&format!("account-role-{carla}"), helpers));
    window.until("the member holds the role", |window| {
        member_choices(window, carla).is_some_and(|choices| choices[2].held && !choices[0].held)
            && idle(window)
    });
    other.until("the window of the member offers the deletion", |window| {
        may_delete(window) == Some((true, true))
    });

    // The role is taken again, and the other window follows.
    window.until("the roles of the member are drawn again", |window| {
        window.shows(sel("account-roles", carla))
    });
    click_in_page(&mut window, sel(&format!("account-role-{carla}"), helpers));
    window.until("the member holds the role no longer", |window| {
        member_choices(window, carla).is_some_and(|choices| !choices[2].held) && idle(window)
    });
    other.until("the window of the member offers it no longer", |window| {
        may_delete(window) == Some((false, false))
    });

    // The deletion asks first.
    open_page(&mut window, "admin-nav-roles", |window| {
        roles_page(window).is_some()
    });
    click_in_page(&mut window, sel("role-delete", guests));
    assert!(window.shows("role-confirm") && window.shows("role-cancel"));
    assert_eq!(
        position_on_server(&world, &ada, workspace_id, "Visitors"),
        Some(1)
    );
    click_in_page(&mut window, "role-confirm");
    window.until("the role is gone", |window| {
        role_names(window) == ["Chiefs", "Leads", "Helpers", "everyone"] && idle(window)
    });
    assert_eq!(
        position_on_server(&world, &ada, workspace_id, "Visitors"),
        None
    );
    // The "everyone" role has no deletion and no new name.
    let everyone = workspace_roles(&world, &ada, workspace_id)
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    assert!(!window.shows(sel("role-delete", everyone)));
    assert!(!window.shows(sel("role-rename", everyone)));
}

fn create_server_role(
    world: &World,
    admin: &Admin,
    name: &str,
    permissions: &[ServerPermission],
) -> ServerRoleId {
    let role = CreateServerRole {
        name: name.to_owned(),
        permissions: permissions.to_vec(),
    };
    world
        .run(admin.handle.api().create_server_role(&role))
        .unwrap()
        .id
}

#[gpui::test]
fn a_moderator_sees_why_an_account_with_a_higher_role_cannot_be_banned_before_trying(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let ada = world.owner(&server);
    // "Stewards" stands above "Wardens"; both give "ban members".
    let stewards = create_server_role(&world, &ada, "Stewards", &[ServerPermission::BanMembers]);
    let wardens = create_server_role(&world, &ada, "Wardens", &[ServerPermission::BanMembers]);
    let (_directory, mut window) = member(cx, &world, &server.address(), &ada, "Bruno");
    let moderator = handle_of(&mut window);
    let bruno = logged_in(&moderator);
    let code = invite(&world, &ada.handle, Vec::new());
    let (_carla_device, carla_handle) = joined(&world, &server, &code, "Carla");
    let code = invite(&world, &ada.handle, Vec::new());
    let (_dario_device, dario_handle) = joined(&world, &server, &code, "Dario");
    let (carla, dario) = (logged_in(&carla_handle), logged_in(&dario_handle));
    let api = ada.handle.api();
    world.run(api.assign_server_role(bruno, wardens)).unwrap();
    world.run(api.assign_server_role(carla, stewards)).unwrap();
    until_model(
        &world,
        &moderator,
        "the permission of the moderator",
        |model| {
            model
                .account
                .server_permissions
                .contains(&ServerPermission::BanMembers)
        },
    );

    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        account(window, carla).is_some()
    });
    // The account with the higher role: the row states the reason, and its controls
    // take no click. The order of the roles is read first.
    let higher = awaited(&mut window, "the order of the roles decides", |window| {
        account(window, carla).filter(|row| row.ban == Err(Blocked::Outranked))
    });
    assert_eq!(higher.remove, Err(Blocked::Outranked));
    assert!(higher.outranked());
    assert!(window.shows(sel("account-outranked", carla)));
    let lower = account(&mut window, dario).unwrap();
    assert_eq!((lower.remove, lower.ban), (Ok(()), Ok(())));
    assert!(!lower.outranked() && !window.shows(sel("account-outranked", dario)));
    // The owner stands above everyone, and an account does not act on itself.
    let owner = account(&mut window, logged_in(&ada.handle)).unwrap();
    assert_eq!(owner.ban, Err(Blocked::Owner));
    assert_eq!(
        account(&mut window, bruno).unwrap().ban,
        Err(Blocked::OwnAccount)
    );
    // The page has no control that gives or takes a server role for a person who is no
    // administrator.
    assert!(higher.assignable.is_none() && lower.assignable.is_none());
    for action in ["account-ban", "account-remove"] {
        click_in_page(&mut window, sel(action, carla));
        assert!(
            !window.shows("account-confirm"),
            "{action} asks about an account above the moderator"
        );
    }
    assert!(idle(&mut window));
    // The server decides the same: the state of the row is no guess of the window alone.
    let refused = world
        .run(moderator.api().ban_account(carla))
        .expect_err("the server bans an account above the moderator");
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));

    // The owner changes the order of the two roles: the row follows without an act of
    // the moderator, and the ban is possible.
    world
        .run(api.reorder_server_roles(vec![
            ServerRolePosition {
                role_id: wardens,
                position: 2,
            },
            ServerRolePosition {
                role_id: stewards,
                position: 1,
            },
        ]))
        .unwrap();
    awaited(&mut window, "the row follows the new order", |window| {
        account(window, carla).filter(|row| row.ban == Ok(()) && row.remove == Ok(()))
    });
    window.until("the reason is gone", |window| {
        !window.shows(sel("account-outranked", carla))
    });
    // The state is replaced after a change of the order, and the page reads the order
    // again: for that moment the control is drawn as held, without the reason of role
    // order, and takes no click. The click is repeated until the question is drawn.
    window.until("the ban asks first", |window| {
        if window.shows("account-confirm") {
            return true;
        }
        try_click(window, sel("account-ban", carla));
        false
    });
    // The order changes back while the question is open: the question is put away, and
    // the row states the reason again. Nothing was sent.
    let reorder = |first: ServerRoleId, second: ServerRoleId| {
        world
            .run(api.reorder_server_roles(vec![
                ServerRolePosition {
                    role_id: first,
                    position: 2,
                },
                ServerRolePosition {
                    role_id: second,
                    position: 1,
                },
            ]))
            .unwrap();
    };
    reorder(stewards, wardens);
    window.until("the open question is put away", |window| {
        !window.shows("account-confirm") && window.shows(sel("account-outranked", carla))
    });
    let active = world
        .run(api.accounts())
        .unwrap()
        .into_iter()
        .find(|held| held.id == carla)
        .map(|held| held.status);
    assert_eq!(active, Some(AccountStatus::Active));
    reorder(wardens, stewards);
    awaited(&mut window, "the row follows the order again", |window| {
        account(window, carla).filter(|row| row.ban == Ok(()))
    });
    window.until("the reason is gone again", |window| {
        !window.shows(sel("account-outranked", carla))
    });
    // As above: the control is held while the order is read again after the reload.
    window.until("the ban asks again", |window| {
        if window.shows("account-confirm") {
            return true;
        }
        try_click(window, sel("account-ban", carla));
        false
    });
    click_in_page(&mut window, "account-confirm");
    // The event of the ban marks the account banned and makes the unban statements of the
    // accounts stale until they are read again (ADR 0311): for that time the unban waits
    // for the read.
    let banned = awaited(
        &mut window,
        "the account is banned and its unban is read",
        |window| {
            account(window, carla).filter(|row| {
                row.status == AccountStatus::Banned && row.unban != Err(Blocked::OrderNotRead)
            })
        },
    );
    assert_eq!(banned.unban, Ok(()));

    // An unban restores the roles of the account, so it compares them too (ADR 0309):
    // with the old order the banned account stands above the moderator again.
    world
        .run(api.reorder_server_roles(vec![
            ServerRolePosition {
                role_id: stewards,
                position: 2,
            },
            ServerRolePosition {
                role_id: wardens,
                position: 1,
            },
        ]))
        .unwrap();
    let held = awaited(&mut window, "the unban is held by the order", |window| {
        account(window, carla).filter(|row| row.unban == Err(Blocked::Outranked))
    });
    assert_eq!(held.status, AccountStatus::Banned);
    window.until("the reason is drawn", |window| {
        window.shows(sel("account-outranked", carla))
    });
    click_in_page(&mut window, sel("account-unban", carla));
    assert!(idle(&mut window));
    let refused = world
        .run(moderator.api().unban_account(carla))
        .expect_err("the server lifts the ban of an account above the moderator");
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
}

/// The answer to the form of a new role closes the form, and the rows of the roles under
/// it keep their places (ADR 0730).
#[gpui::test]
fn an_answer_to_the_form_of_a_new_role_moves_no_row_of_a_role(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = named(cx, "Ada");
    window.find_server(&proxy.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    window.open_administration();
    open_page(&mut window, "admin-nav-roles", |window| {
        role_rows(window).is_some()
    });
    let everyone = role_rows(&mut window)
        .unwrap()
        .into_iter()
        .find(|row| row.is_everyone)
        .unwrap()
        .role;
    let row = sel("role-row", everyone.written());

    // The role is made on the server and its row is drawn while the answer waits at the
    // proxy.
    click_in_page(&mut window, "role-new");
    proxy.hold_answer_ending("POST", "/roles");
    send_role_form(&mut window, "Helpers");
    window.until("the answer waits and the new role is drawn", |window| {
        proxy.answers_waiting() == 1 && role_names(window).contains(&"Helpers".to_owned())
    });
    let before =
        crate::admin_pages::reveal_under_form(&mut window, "role-name", "roles-header", row);
    proxy.release();
    window.until("the form is closed", |window| {
        idle(window) && !window.shows("role-name")
    });
    assert_eq!(
        crate::admin_pages::under_form(&mut window, "roles-header", row),
        before
    );
}

/// The row of a role tells one sentence for the change of its permissions, its deletion and
/// its move: after a refused change of a permission and a refused move, the move.
#[gpui::test]
fn the_row_of_a_role_tells_the_refusal_of_its_newest_request(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let workspace_id = default_workspace(&world, &owner);
    let create = |name: &str| {
        let role = CreateWorkspaceRole {
            name: name.to_owned(),
            permissions: Vec::new(),
        };
        world
            .run(owner.api().create_workspace_role(workspace_id, &role))
            .unwrap()
            .id
    };
    let chiefs = create("Chiefs");
    create("Leads");
    let role = RoleRef::Workspace(workspace_id, chiefs);
    window.open_administration();
    open_page(&mut window, "admin-nav-roles", |window| {
        roles_page(window).is_some()
    });
    window.until("the roles are listed in their order", |window| {
        role_names(window) == ["Chiefs", "Leads", "everyone"]
    });
    let asks = |window: &mut Window| window.admin().map(|view| view.asks).unwrap_or_default();
    let refused = |what: AskKind| {
        move |window: &mut Window| {
            matches!(
                asks(window).as_slice(),
                [Ask {
                    what: held,
                    state: AskState::Refused(_),
                }] if *held == what
            )
        }
    };

    // The client sends a request that gets `busy` again, up to its attempts.
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    let busy = || std::iter::repeat_n(Fault::Busy, attempts);
    proxy.fail_ending(&format!("/roles/{chiefs}/permissions"), busy());
    click_in_page(&mut window, sel("role-permissions", chiefs));
    window.until("the permissions of the role", |window| {
        window.shows("role-permission-manage_messages")
    });
    click_in_page(&mut window, "role-permission-manage_messages");
    window.until("the refused change", refused(AskKind::UpdateRole(role)));
    assert!(window.shows("role-refused"));

    proxy.fail_ending("/roles/positions", busy());
    click_in_page(&mut window, sel("role-down", chiefs));
    window.until(
        "only the refused move is told",
        refused(AskKind::MoveRole(role)),
    );
    assert!(window.shows("role-refused"));
}

/// While the new name of a role is on its way, the switches of its permissions send
/// nothing: the row and its form tell the answer to the name, and no later request at the
/// row replaces it (ADR 0761).
#[gpui::test]
fn the_permissions_of_a_role_wait_for_the_answer_to_its_new_name(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let workspace_id = default_workspace(&world, &owner);
    let chiefs = world
        .run(owner.api().create_workspace_role(
            workspace_id,
            &CreateWorkspaceRole {
                name: "Chiefs".to_owned(),
                permissions: Vec::new(),
            },
        ))
        .unwrap()
        .id;
    window.open_administration();
    open_page(&mut window, "admin-nav-roles", |window| {
        roles_page(window).is_some()
    });
    window.until("the role is listed", |window| {
        role_names(window) == ["Chiefs", "everyone"]
    });
    click_in_page(&mut window, sel("role-permissions", chiefs));
    window.until("the permissions of the role", |window| {
        window.shows("role-permission-manage_messages")
    });
    let renamed = format!("/api/v1/workspaces/{workspace_id}/roles/{chiefs}");
    let changed = format!("{renamed}/permissions");

    let role = RoleRef::Workspace(workspace_id, chiefs);
    let waits = |window: &mut Window| {
        window.in_shell(|shell, _, _| shell.admin_form_waits(AskKind::UpdateRole(role)))
    };
    let granted = |window: &mut Window| {
        role_row(window, role)
            .unwrap()
            .permissions
            .iter()
            .any(|switch| switch.on)
    };

    click_in_page(&mut window, sel("role-rename", chiefs));
    proxy.hold_request("PATCH", &format!("/roles/{chiefs}"));
    send_role_form(&mut window, "Heads");
    window.until("the new name is on its way", |_| {
        !proxy.seen(&renamed).is_empty()
    });
    assert!(waits(&mut window));
    let permission = RolePermission::Workspace(WorkspacePermission::ManageMessages);
    assert_eq!(
        permission_held(&mut window, role, permission).as_deref(),
        Some(FORM_SENT)
    );
    click_in_page(&mut window, "role-permission-manage_messages");
    window.settle();
    assert!(waits(&mut window));
    assert!(window.shows("role-name"));
    assert_eq!(
        window
            .admin()
            .and_then(|view| view.permission_asked(role, permission)),
        None,
        "the press while the name was on its way asked for a permission"
    );
    assert!(!granted(&mut window));

    proxy.release();
    window.until(
        "the role has its new name and the form is closed",
        |window| role_names(window) == ["Heads", "everyone"] && !window.shows("role-name"),
    );
    assert!(!waits(&mut window));
    assert!(!granted(&mut window));
    // The press while the name was on its way sent nothing: after the next press, the
    // server has seen one change of the permissions.
    click_in_page(&mut window, "role-permission-manage_messages");
    window.until("the role holds the permission", |window| granted(window));
    assert_eq!(
        proxy.seen(&changed).len(),
        1,
        "a permission was sent while the new name was on its way"
    );
}

/// A Cancel of the rename of a role while its new name is on its way, and the form of
/// another role opened after it, do not end the wait: the permissions of the role send
/// nothing until the answer arrives, and the refusal of the name is told in its row
/// (ADR 0761).
#[gpui::test]
fn a_cancelled_rename_of_a_role_keeps_its_permissions_waiting_and_tells_the_refusal(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let workspace_id = default_workspace(&world, &owner);
    let create = |name: &str| {
        let role = CreateWorkspaceRole {
            name: name.to_owned(),
            permissions: Vec::new(),
        };
        world
            .run(owner.api().create_workspace_role(workspace_id, &role))
            .unwrap()
            .id
    };
    let chiefs = create("Chiefs");
    let leads = create("Leads");
    let role = RoleRef::Workspace(workspace_id, chiefs);
    window.open_administration();
    open_page(&mut window, "admin-nav-roles", |window| {
        roles_page(window).is_some()
    });
    window.until("the roles are listed", |window| {
        role_names(window) == ["Chiefs", "Leads", "everyone"]
    });
    click_in_page(&mut window, sel("role-permissions", chiefs));
    window.until("the permissions of the role", |window| {
        window.shows("role-permission-manage_messages")
    });
    let renamed = format!("/api/v1/workspaces/{workspace_id}/roles/{chiefs}");
    let changed = format!("{renamed}/permissions");
    let waits = |window: &mut Window| {
        window.in_shell(|shell, _, _| shell.admin_form_waits(AskKind::UpdateRole(role)))
    };

    // Every try of the new name is answered `busy`, so the name is refused.
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    proxy.fail(&renamed, std::iter::repeat_n(Fault::Busy, attempts));
    proxy.hold_request("PATCH", &format!("/roles/{chiefs}"));
    click_in_page(&mut window, sel("role-rename", chiefs));
    send_role_form(&mut window, "Heads");
    window.until("the new name is on its way", |_| {
        !proxy.seen(&renamed).is_empty()
    });
    click_in_page(&mut window, "role-form-cancel");
    window.until("the form is put away", |window| !window.shows("role-name"));
    click_in_page(&mut window, sel("role-rename", leads));
    window.until("the form of the other role", |window| {
        window.shows("role-name")
    });
    assert!(waits(&mut window));
    let permission = RolePermission::Workspace(WorkspacePermission::ManageMessages);
    assert_eq!(
        permission_held(&mut window, role, permission).as_deref(),
        Some(FORM_SENT)
    );
    click_in_page(&mut window, "role-permission-manage_messages");
    // The move and the deletion of the role send nothing either: each would replace the
    // state of the new name at the place of the role.
    click_in_page(&mut window, sel("role-down", chiefs));
    click_in_page(&mut window, sel("role-delete", chiefs));
    window.settle();
    let unchanged = |window: &mut Window| {
        window.admin().is_some_and(|view| {
            view.sending(AskKind::UpdateRole(role))
                && view.permission_asked(role, permission).is_none()
        })
    };
    assert!(unchanged(&mut window));
    assert!(!window.shows("role-confirm"));
    assert!(!drag_role(
        &mut window,
        sel("role-handle", chiefs),
        sel("role-row", leads)
    ));
    assert!(waits(&mut window));
    assert!(unchanged(&mut window));

    proxy.release();
    window.until("the refusal of the new name is told", |window| {
        window.shows("role-refused")
            && window.admin().is_some_and(|view| {
                view.asks.iter().any(|ask| {
                    ask.what == AskKind::UpdateRole(role)
                        && matches!(ask.state, AskState::Refused(_))
                })
            })
    });
    assert!(!waits(&mut window));
    assert_eq!(proxy.seen(&renamed).len(), attempts);
    assert!(
        proxy.seen(&changed).is_empty(),
        "a permission was sent while the new name was on its way"
    );
    assert_eq!(role_names(&mut window), ["Chiefs", "Leads", "everyone"]);
}

/// One permission of a role is given, taken and given again against the real server. The
/// event of the second change is held back: until the state holds it, the switch shows the
/// asked state, takes no click and sends nothing, so the third change starts from the state
/// that the server holds (ADR 0311, ADR 0763, ADR 0791).
#[gpui::test]
fn a_permission_given_taken_and_given_again_ends_as_the_last_change(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let workspace_id = default_workspace(&world, &owner);
    let role = CreateWorkspaceRole {
        name: "Helpers".to_owned(),
        permissions: Vec::new(),
    };
    let helpers = world
        .run(owner.api().create_workspace_role(workspace_id, &role))
        .unwrap()
        .id;
    let stored = || -> Vec<WorkspacePermission> {
        world
            .run(owner.api().workspace_roles(workspace_id))
            .unwrap()
            .into_iter()
            .find(|role| role.id == helpers)
            .unwrap()
            .permissions
    };
    let changes = || {
        let path = format!("/roles/{helpers}/permissions");
        proxy
            .all()
            .iter()
            .filter(|seen| seen.path.ends_with(&path))
            .count()
    };
    let permission = WorkspacePermission::ManageMessages;
    // The state that the switch shows: the asked state while a change waits.
    let on = |window: &mut Window| -> Option<bool> {
        let role = RoleRef::Workspace(workspace_id, helpers);
        let asked = window
            .admin()?
            .permission_asked(role, RolePermission::Workspace(permission));
        let held = role_row(window, role)?
            .permissions
            .iter()
            .find(|switch| switch.permission == RolePermission::Workspace(permission))
            .map(|switch| switch.on)?;
        Some(asked.unwrap_or(held))
    };
    let settled = |window: &mut Window| {
        idle(window)
            && window
                .admin()
                .is_some_and(|view| view.changing_roles.is_empty())
    };
    window.open_administration();
    open_page(&mut window, "admin-nav-roles", |window| {
        roles_page(window).is_some()
    });
    click_in_page(&mut window, sel("role-permissions", helpers));
    window.until("the permissions are shown", |window| {
        window.shows("role-permission-manage_messages")
    });

    // A: the permission is given.
    click_in_page(&mut window, "role-permission-manage_messages");
    window.until("the role holds the permission", |window| {
        on(window) == Some(true) && settled(window)
    });
    assert_eq!(stored(), [permission]);
    assert_eq!(changes(), 1);

    // B: it is taken, and the event of the change waits at the proxy. The server holds the
    // change; the switch shows the asked state and is held.
    proxy.hold_events();
    click_in_page(&mut window, "role-permission-manage_messages");
    window.until("the server answered the change", |window| {
        changes() == 2 && idle(window)
    });
    assert_eq!(stored(), Vec::<WorkspacePermission>::new());
    assert_eq!(on(&mut window), Some(false));
    assert!(
        window
            .admin()
            .is_some_and(|view| !view.changing_roles.is_empty())
    );
    // A click on the held switch sends nothing.
    assert!(try_click(&mut window, "role-permission-manage_messages"));
    window.settle();
    assert_eq!(changes(), 2);
    assert_eq!(stored(), Vec::<WorkspacePermission>::new());

    // The event arrives: the switch stays off and takes clicks again.
    proxy.release();
    window.until("the state holds the second change", |window| {
        on(window) == Some(false) && settled(window)
    });

    // A again: the permission is given once more, and the role ends with it.
    click_in_page(&mut window, "role-permission-manage_messages");
    window.until("the role holds the permission again", |window| {
        on(window) == Some(true) && settled(window)
    });
    assert_eq!(changes(), 3);
    assert_eq!(stored(), [permission]);
}
