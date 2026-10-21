//! The glossary page of a workspace in the window against the server binary (ADR 0616):
//! a member reads it, and a person with "manage glossary" adds, changes, moves and removes
//! terms. A change made from an older list is refused by the server.

use gpui::TestAppContext;
use junto_client::state::admin::AskKind;
use junto_client::state::admin::view::{Listed, Scope, ServerPage};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::WorkspaceId;
use junto_client_core::protocol::roles::UpdateWorkspaceRole;

use crate::admin_pages::{click_in_page, open_page, reveal_under_form, sel, under_form};
use crate::harness::{Admin, Window, World, member};
use crate::support::{FaultProxy, TestServer};
use crate::transcripts::give_everyone;

/// The glossary page of the shown workspace as the window draws it.
fn glossary_page(
    window: &mut Window,
) -> Option<junto_client::state::admin::view::glossary::GlossaryPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Glossary(page),
            ..
        }) => Some(page),
        _ => None,
    }
}

fn terms(window: &mut Window) -> Option<Vec<String>> {
    match glossary_page(window)?.terms {
        Listed::Loaded(terms) => Some(terms),
        _ => None,
    }
}

fn list(terms: &[&str]) -> Vec<String> {
    terms.iter().map(|term| (*term).to_owned()).collect()
}

#[gpui::test]
fn a_member_reads_the_glossary_and_a_person_who_manages_it_adds_changes_orders_and_removes_terms(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    // The owner works through the API; the window of a member reaches the server
    // through the proxy, so that its event stream can be held.
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let workspace_id = window.view().workspace().unwrap().id;
    let api = admin.handle.api();
    let stored = |world: &World| world.run(api.glossary(workspace_id)).unwrap().terms;
    world
        .run(api.set_glossary(workspace_id, &list(&["Quorvex", "Lumitrax"]), None))
        .unwrap();

    // Every member reads the glossary. Without "manage glossary" nothing changes it.
    window.open_administration();
    open_page(&mut window, "admin-nav-glossary", |window| {
        glossary_page(window).is_some()
    });
    window.until("the terms are read", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax"]))
    });
    assert!(window.shows("glossary-row-0") && window.shows("glossary-row-1"));
    assert!(!glossary_page(&mut window).unwrap().may_manage);
    window.click("glossary-add");
    assert!(!window.shows("glossary-term"));

    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageGlossary,
    );
    window.until("the person may change the glossary", |window| {
        glossary_page(window).is_some_and(|page| page.held().is_none())
            && window.shows("glossary-add")
    });

    // A new term goes to the end of the list.
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Ysolde Varkonen", window, cx));
    click_in_page(&mut window, "admin-form-send");
    window.until("the new term", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax", "Ysolde Varkonen"]))
            && !window.shows("glossary-term")
    });
    assert_eq!(
        stored(&world),
        list(&["Quorvex", "Lumitrax", "Ysolde Varkonen"])
    );

    // A term that the list holds is refused before it is sent.
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Quorvex", window, cx));
    click_in_page(&mut window, "admin-form-send");
    assert!(window.shows("admin-form-invalid"));
    click_in_page(&mut window, "admin-form-cancel");

    // A changed term. The answer closes the form: the row under it keeps its place.
    let settled =
        |window: &mut Window| glossary_page(window).is_some_and(|page| page.held().is_none());
    window.until("the page takes changes", settled);
    click_in_page(&mut window, sel("glossary-change", 1));
    window.in_shell(|shell, window, cx| shell.type_admin_name("Lumitrax Ltd", window, cx));
    // The list draws the rows in view: the button that sends the form and the row under
    // the form are brought into view together. The row of the changed term has the key of
    // its new text.
    let under = reveal_under_form(
        &mut window,
        "admin-form-send",
        "term-Lumitrax",
        "glossary-row-2",
    );
    click_in_page(&mut window, "admin-form-send");
    window.until("the changed term", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax Ltd", "Ysolde Varkonen"]))
            && !window.shows("glossary-term")
    });
    assert_eq!(
        under_form(&mut window, "term-Lumitrax Ltd", "glossary-row-2"),
        under
    );

    // The last term moves up by one place.
    window.until("the page takes changes", settled);
    click_in_page(&mut window, sel("glossary-up", 2));
    window.until("the moved term", |window| {
        terms(window) == Some(list(&["Quorvex", "Ysolde Varkonen", "Lumitrax Ltd"]))
    });

    // The first term is removed after a question.
    window.until("the page takes changes", settled);
    click_in_page(&mut window, sel("glossary-remove", 0));
    click_in_page(&mut window, "glossary-remove-cancel");
    assert_eq!(terms(&mut window).unwrap().len(), 3);
    click_in_page(&mut window, sel("glossary-remove", 0));
    click_in_page(&mut window, "glossary-remove-confirm");
    window.until("the removed term", |window| {
        terms(window) == Some(list(&["Ysolde Varkonen", "Lumitrax Ltd"]))
    });
    assert_eq!(stored(&world), list(&["Ysolde Varkonen", "Lumitrax Ltd"]));

    // Another person changes the glossary while the window does not follow the server: a
    // change made from the list of the window is refused, and changes nothing.
    window.until("the page takes changes", settled);
    proxy.hold_events();
    world
        .run(api.set_glossary(workspace_id, &list(&["Kestrel"]), None))
        .unwrap();
    click_in_page(&mut window, sel("glossary-down", 0));
    window.until("the refusal of the change", |window| {
        window.shows("glossary-refused")
    });
    assert_eq!(stored(&world), list(&["Kestrel"]));
    proxy.release_events();
    window.until("the glossary of the other person", |window| {
        terms(window) == Some(list(&["Kestrel"]))
    });
}

/// A member window on the glossary page of the default workspace, through a proxy, with
/// the terms given and "manage glossary" given to every member.
struct Page {
    world: World,
    _server: TestServer,
    proxy: FaultProxy,
    admin: Admin,
    _directory: tempfile::TempDir,
    window: Window,
    workspace_id: WorkspaceId,
}

fn manager_on_the_page(cx: &mut TestAppContext, given: &[&str]) -> Page {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let workspace_id = window.view().workspace().unwrap().id;
    world
        .run(
            admin
                .handle
                .api()
                .set_glossary(workspace_id, &list(given), None),
        )
        .unwrap();
    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageGlossary,
    );
    window.open_administration();
    open_page(&mut window, "admin-nav-glossary", |window| {
        glossary_page(window).is_some()
    });
    window.until("the page takes changes", |window| {
        terms(window) == Some(list(given))
            && glossary_page(window).is_some_and(|page| page.held().is_none())
    });
    Page {
        world,
        _server: server,
        proxy,
        admin,
        _directory: directory,
        window,
        workspace_id,
    }
}

/// Takes a permission from the role "everyone" of the workspace, as the owner does.
fn take_from_everyone(
    world: &World,
    admin: &Admin,
    workspace_id: WorkspaceId,
    permission: WorkspacePermission,
) {
    let api = admin.handle.api();
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap();
    let permissions = everyone
        .permissions
        .iter()
        .copied()
        .filter(|held| *held != permission)
        .collect();
    let update = UpdateWorkspaceRole {
        name: None,
        permissions: Some(permissions),
    };
    world
        .run(api.update_workspace_role(workspace_id, everyone.id, &update))
        .unwrap();
}

/// Another person changes the term of an open form: the form closes, and its row keeps the
/// height of the form until the person acts, so the rows under it do not move (ADR 0730).
#[gpui::test]
fn a_change_of_another_person_closes_a_form_and_moves_no_row_under_it(cx: &mut TestAppContext) {
    let Page {
        world,
        admin,
        mut window,
        workspace_id,
        _server,
        _directory,
        proxy: _proxy,
    } = manager_on_the_page(cx, &["Quorvex", "Lumitrax", "Ysolde Varkonen"]);
    click_in_page(&mut window, sel("glossary-change", 1));
    assert!(window.shows("glossary-term"));
    let under = reveal_under_form(
        &mut window,
        "admin-form-send",
        "term-Lumitrax",
        "glossary-row-2",
    );
    let changed = list(&["Quorvex", "Lumitrax Ltd", "Ysolde Varkonen"]);
    world
        .run(
            admin
                .handle
                .api()
                .set_glossary(workspace_id, &changed, None),
        )
        .unwrap();
    window.until("the form of the changed term is closed", |window| {
        terms(window) == Some(changed.clone()) && !window.shows("glossary-term")
    });
    assert_eq!(
        under_form(&mut window, "term-Lumitrax Ltd", "glossary-row-2"),
        under
    );
}

/// The permission is taken while the form of a new term is open: the form takes no send,
/// says why, and an Enter in its field sends nothing.
#[gpui::test]
fn a_form_that_the_permission_was_taken_from_sends_nothing(cx: &mut TestAppContext) {
    let Page {
        world,
        admin,
        mut window,
        workspace_id,
        _server,
        _directory,
        proxy: _proxy,
    } = manager_on_the_page(cx, &["Quorvex"]);
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Kestrel", window, cx));
    assert!(!window.shows("admin-form-held"));
    take_from_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageGlossary,
    );
    window.until("the form states that it takes no send", |window| {
        glossary_page(window).is_some_and(|page| !page.may_manage)
            && window.shows("admin-form-held")
    });
    window.in_shell(|shell, window, cx| shell.submit_admin_form(window, cx));
    assert!(window.shows("admin-form-invalid"));
    let stored = world
        .run(admin.handle.api().glossary(workspace_id))
        .unwrap()
        .terms;
    assert_eq!(stored, list(&["Quorvex"]));
}

/// The change of the person is answered, and another person puts the list back before the
/// window has read it: the page shows the list of the server and takes changes again.
#[gpui::test]
fn a_change_that_another_person_undid_leaves_the_page_taking_changes(cx: &mut TestAppContext) {
    let Page {
        world,
        proxy,
        admin,
        mut window,
        workspace_id,
        _server,
        _directory,
    } = manager_on_the_page(cx, &["Quorvex", "Lumitrax"]);
    proxy.hold_events();
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Kestrel", window, cx));
    click_in_page(&mut window, "admin-form-send");
    // The answer carries the new list, which the page shows before the state has it.
    window.until("the answered list", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax", "Kestrel"]))
            && !window.shows("glossary-term")
    });
    world
        .run(
            admin
                .handle
                .api()
                .set_glossary(workspace_id, &list(&["Quorvex", "Lumitrax"]), None),
        )
        .unwrap();
    proxy.release_events();
    window.until(
        "the list of the server, and the page takes changes",
        |window| {
            terms(window) == Some(list(&["Quorvex", "Lumitrax"]))
                && glossary_page(window).is_some_and(|page| page.held().is_none())
        },
    );
    click_in_page(&mut window, "glossary-add");
    assert!(window.shows("glossary-term"));
}

/// A permission given or taken while the page is open moves no row: the sentence that tells
/// that the roles do not hold "manage glossary" stands below the list (ADR 0730).
#[gpui::test]
fn a_permission_given_or_taken_moves_no_row_of_the_page(cx: &mut TestAppContext) {
    let Page {
        world,
        admin,
        mut window,
        workspace_id,
        _server,
        _directory,
        proxy: _proxy,
    } = manager_on_the_page(cx, &["Quorvex", "Lumitrax"]);
    let rows = |window: &mut Window| {
        (
            window.cx.debug_bounds("glossary-row-0").unwrap(),
            window.cx.debug_bounds("glossary-row-1").unwrap(),
        )
    };
    let shown = rows(&mut window);
    assert!(!window.shows("glossary-manage-needs"));
    take_from_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageGlossary,
    );
    // The change of the permission replaces the state, and the glossary is read again.
    window.until("the page takes no change, with the list read", |window| {
        glossary_page(window).is_some_and(|page| !page.may_manage && page.terms.loaded().is_some())
    });
    assert_eq!(rows(&mut window), shown);
    assert!(window.shows("glossary-manage-needs"));
    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageGlossary,
    );
    window.until("the page takes changes", |window| {
        glossary_page(window).is_some_and(|page| page.held().is_none())
    });
    assert_eq!(rows(&mut window), shown);
    assert!(!window.shows("glossary-manage-needs"));
}

/// The answer to a change of the person arrives late: the event of the change, a change of
/// another person and their reads all came before it. The page shows the list of the server,
/// with the change of the other person, and no older list (ADR 0616).
#[gpui::test]
fn a_late_answer_does_not_hide_a_later_change_of_another_person(cx: &mut TestAppContext) {
    let Page {
        world,
        proxy,
        admin,
        mut window,
        workspace_id,
        _server,
        _directory,
    } = manager_on_the_page(cx, &["Quorvex", "Lumitrax"]);
    proxy.hold_answer_ending("PUT", "/glossary");
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Kestrel", window, cx));
    click_in_page(&mut window, "admin-form-send");
    window.until("the server did the change", |window| {
        proxy.answers_waiting() == 1
            && terms(window) == Some(list(&["Quorvex", "Lumitrax", "Kestrel"]))
    });
    let later = list(&["Quorvex", "Lumitrax", "Kestrel", "Tern"]);
    world
        .run(admin.handle.api().set_glossary(workspace_id, &later, None))
        .unwrap();
    window.until("the change of the other person", |window| {
        terms(window) == Some(later.clone())
    });
    proxy.release();
    window.until("the answer of the change", |window| {
        window
            .admin()
            .is_some_and(|view| !view.sending(AskKind::AddTerm))
    });
    window.until(
        "the list of the server, and the page takes changes",
        |window| {
            terms(window) == Some(later.clone())
                && glossary_page(window).is_some_and(|page| page.held().is_none())
        },
    );
}
