//! The glossary page of a workspace in the window against the server binary (ADR 0616):
//! a member reads it, and a person with "manage glossary" adds, changes, moves and removes
//! terms. A change made from an older list is refused by the server.

use gpui::TestAppContext;
use junto_client::state::admin::view::{Listed, Scope, ServerPage};
use junto_client_core::protocol::WorkspacePermission;

use crate::admin_pages::{click_in_page, open_page, sel};
use crate::harness::{Window, World, member};
use crate::support::FaultProxy;
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
    let under = window.cx.debug_bounds("glossary-row-2").unwrap();
    click_in_page(&mut window, "admin-form-send");
    window.until("the changed term", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax Ltd", "Ysolde Varkonen"]))
            && !window.shows("glossary-term")
    });
    assert_eq!(window.cx.debug_bounds("glossary-row-2").unwrap(), under);

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
