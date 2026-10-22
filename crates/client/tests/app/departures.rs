//! Leaving a workspace, leaving the server and deleting the account in the window, against
//! the real server (ADR 0121). The other members keep the messages of the person, with
//! the name marked.

use gpui::TestAppContext;
use junto_client::state::account::{AccountPage, PageView};
use junto_client::state::feed::plain_text;
use junto_client::state::view::ServerStatus;
use junto_client::text;
use junto_client_core::protocol::account::AccountStatus;

use crate::harness::{Window, World, member};

impl Window {
    /// The workspaces that the server book lists for the selected server.
    fn listed_workspaces(&mut self) -> Vec<String> {
        self.view()
            .servers
            .iter()
            .find(|row| row.selected)
            .map(|row| row.workspaces.iter().map(|row| row.name.clone()).collect())
            .unwrap_or_default()
    }

    /// The authors of the drawn messages whose text is `text`.
    fn authors_of(&mut self, text: &str) -> Vec<String> {
        let Some(feed) = self.feed() else {
            return Vec::new();
        };
        self.messages()
            .into_iter()
            .filter(|row| {
                row.message
                    .document
                    .as_ref()
                    .is_some_and(|document| plain_text(document, &feed.names, 200) == text)
            })
            .map(|row| row.author)
            .collect()
    }
}

#[gpui::test]
fn a_member_leaves_a_workspace_then_the_server_and_returns_with_a_code(
    cx: &mut TestAppContext,
    other_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let lab = world
        .run(admin.handle.api().create_workspace("Lab".to_owned()))
        .unwrap();
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_other_directory, mut other) =
        member(other_cx, &world, &server.address(), &admin, "Bruno");
    let luca = window.own_account();
    world
        .run(admin.handle.api().add_workspace_member(lab.id, luca))
        .unwrap();
    window.until("the window lists both workspaces", |window| {
        window.listed_workspaces().len() == 2
    });
    window.write("Kept words");
    window.until_message("Kept words");
    other.until_message("Kept words");
    assert_eq!(other.authors_of("Kept words"), ["Luca"]);

    // The menu of the server opens the page of the account layer with the question for
    // the shown workspace. It is not the last one: the question says what stays and that
    // the account stays.
    let shown = window.view().workspace().unwrap().id;
    window.click("server-menu-button");
    window.click("menu-leave-workspace");
    let view = window.account().unwrap();
    assert_eq!(view.selected, AccountPage::Departure);
    let PageView::Departure(page) = &view.page else {
        panic!("the departure page is not shown");
    };
    assert_eq!(page.workspaces.len(), 2);
    assert!(!page.leaves_the_server(shown));
    assert!(window.shows("account-leave-question"));
    assert!(text::account::leave_question(false).contains("What stays"));
    window.click("account-leave-confirm");
    window.until("the left workspace is no longer listed", |window| {
        window.listed_workspaces() == ["Lab"]
    });
    assert_eq!(window.status(), Some(ServerStatus::Ready));

    // The last workspace: the account becomes inactive, and the window says how to return.
    window.open_account();
    window.click("account-nav-departure");
    window.click("account-leave");
    assert!(window.shows("account-leave-question"));
    window.click("account-leave-confirm");
    window.until("the server shows that the person left", |window| {
        window.status() == Some(ServerStatus::Inactive)
    });
    assert!(window.shows("inactive") && window.shows("return-to-server"));

    // The other member keeps the message, under the name with its mark.
    other.until("the author is marked as inactive", |other| {
        other.authors_of("Kept words") == ["Luca (inactive)"]
    });
    let accounts = world.run(admin.handle.api().accounts()).unwrap();
    let held = accounts.iter().find(|account| account.id == luca).unwrap();
    assert_eq!(held.status, AccountStatus::Inactive);

    // An invite code returns the same account, with its name.
    let code = admin.invite(&world, 1);
    window.click("return-to-server");
    window.join_with_code(code.expose());
    window.until_ready();
    assert_eq!(window.own_account(), luca);
    assert_eq!(window.view().person.name, "Luca");
    other.until("the mark is gone after the return", |other| {
        other.authors_of("Kept words") == ["Luca"]
    });
}

#[gpui::test]
fn deleting_the_account_removes_the_server_and_others_see_a_deleted_author(
    cx: &mut TestAppContext,
    other_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_other_directory, mut other) =
        member(other_cx, &world, &server.address(), &admin, "Bruno");
    let luca = window.own_account();
    window.write("Kept words");
    window.until_message("Kept words");
    other.until_message("Kept words");

    window.open_account();
    window.click("account-nav-departure");
    window.click("account-delete");
    assert!(window.shows("account-delete-question"));
    // Cancel leaves everything as it was.
    window.click("account-delete-cancel");
    assert!(!window.shows("account-delete-question"));
    window.click("account-delete");
    window.click("account-delete-confirm");
    window.until("the server leaves the list of this device", |window| {
        window.view().servers.is_empty()
    });

    other.until("the author is marked as deleted", |other| {
        other.authors_of("Kept words") == ["Luca (deleted account)"]
    });
    let accounts = world.run(admin.handle.api().accounts()).unwrap();
    let held = accounts.iter().find(|account| account.id == luca).unwrap();
    assert_eq!(held.status, AccountStatus::Deleted);
    assert_eq!(held.name, "Luca");
}
