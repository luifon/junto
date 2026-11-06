//! A member of a server joins further workspaces of the same server with an invite of that
//! server (ADR 0115): from the menus of the server, and from "Add server" with the address
//! or the invite link of a server of the server book. The dialog names the workspaces that
//! the invite added.

use gpui::TestAppContext;
use junto_client::shell::Open;
use junto_client_core::address::ServerAddress;
use junto_client_core::links::{self, AppLink};
use junto_client_core::protocol::ids::WorkspaceId;
use junto_client_core::protocol::invite::CreateInvite;
use junto_client_core::protocol::secret::InviteCode;

use crate::harness::{Admin, Window, World, member};

/// An invite of the server for these workspaces.
fn invite_for(world: &World, admin: &Admin, workspace_ids: Vec<WorkspaceId>) -> InviteCode {
    world
        .run(admin.handle.api().create_invite(&CreateInvite {
            workspace_ids,
            max_uses: Some(1),
            ..CreateInvite::default()
        }))
        .unwrap()
        .code
}

/// True when the join dialog states, after a redemption, a sentence that holds this text.
fn says(window: &mut Window, part: &str) -> bool {
    window.shows("redeemed-workspaces")
        && window
            .in_shell(|shell, _, cx| shell.join_redeemed(cx))
            .is_some_and(|sentence| sentence.contains(part))
}

fn holds(window: &mut Window, workspace_id: WorkspaceId) -> bool {
    window.view().servers[0]
        .workspaces
        .iter()
        .any(|row| row.id == workspace_id)
}

/// Types the code into the open redemption and sends it.
fn redeem(window: &mut Window, code: &InviteCode) {
    assert!(
        window.shows("redeem-invite"),
        "the redemption of the account is open"
    );
    window.in_shell(|shell, window, cx| shell.type_code(code.expose(), window, cx));
    window.click("redeem-invite");
}

#[gpui::test]
fn a_member_joins_further_workspaces_from_the_menus_of_the_server(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Mira");
    let account = window.own_account();
    let lab = world
        .run(admin.handle.api().create_workspace("Lab".into()))
        .unwrap();
    let studio = world
        .run(admin.handle.api().create_workspace("Studio".into()))
        .unwrap();
    let code = invite_for(&world, &admin, vec![lab.id, studio.id]);

    // The menu of the selected server.
    window.click("server-menu-button");
    window.click("menu-join-with-invite");
    redeem(&mut window, &code);
    window.until("the dialog names the two added workspaces", |window| {
        window.shows("redeemed-workspaces")
            && says(window, "“Lab”")
            && says(window, "“Studio”")
            && says(window, "Your account is now a member of")
    });
    assert!(holds(&mut window, lab.id) && holds(&mut window, studio.id));
    assert_eq!(window.own_account(), account);
    window.click("redeem-done");
    assert!(!window.shows("join-dialog"));

    // The hover menu of the server in the collapsed server book. An invite of workspaces of which
    // the account is a member already adds none: the server refuses it, and the dialog
    // says why.
    let again = invite_for(&world, &admin, vec![lab.id]);
    window.click("collapse-server-book");
    window.until("the server book is collapsed", |window| {
        window.shows("expand-server-book")
    });
    window.in_shell(|shell, window, cx| shell.open(Open::ServerHoverMenu, window, cx));
    window.click("hover-join-with-invite");
    assert!(!window.shows("server-hover-menu"));
    redeem(&mut window, &again);
    window.until(
        "the dialog says that the account holds every workspace",
        |window| {
            window
                .in_shell(|shell, _, _| shell.join_error().map(str::to_owned))
                .as_deref()
                == Some(junto_client::text::REDEEM_ALREADY_MEMBER)
        },
    );
    assert!(window.shows("join-dialog") && !window.shows("redeemed-workspaces"));
    window.press("escape");
    assert!(!window.shows("join-dialog"));
}

#[gpui::test]
fn add_server_with_a_saved_server_redeems_into_its_account(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Mira");
    let account = window.own_account();
    let lab = world
        .run(admin.handle.api().create_workspace("Lab".into()))
        .unwrap();
    let studio = world
        .run(admin.handle.api().create_workspace("Studio".into()))
        .unwrap();

    // The address of the server of the server book opens its redemption, not a refusal.
    window.enter_address(&server.address());
    window.until("the redemption of the saved server opens", |window| {
        window.shows("redeem-invite")
    });
    assert_eq!(window.view().servers.len(), 1);
    let code = invite_for(&world, &admin, vec![lab.id]);
    redeem(&mut window, &code);
    window.until("Lab is added to the account", |window| {
        holds(window, lab.id) && says(window, "“Lab”")
    });
    window.click("redeem-done");

    // The invite link of the server, pasted into "Add server", opens the same redemption
    // with its code.
    let code = invite_for(&world, &admin, vec![studio.id]);
    let link = links::format(&AppLink::Invite {
        server: ServerAddress::parse(&server.address()).unwrap(),
        code,
    });
    window.enter_address(&link);
    window.until("the redemption of the link opens", |window| {
        window.shows("redeem-invite")
    });
    window.click("redeem-invite");
    window.until("Studio is added to the same account", |window| {
        holds(window, studio.id) && says(window, "“Studio”")
    });
    assert_eq!(window.own_account(), account);
    assert_eq!(window.view().servers.len(), 1);
}
