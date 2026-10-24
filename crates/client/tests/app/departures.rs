//! Leaving a workspace, leaving the server and deleting the account in the window, against
//! the real server (ADR 0121). The other members keep the messages of the person, with
//! the name marked.

use gpui::TestAppContext;
use junto_client::state::account::{AccountPage, PageView};
use junto_client::state::feed::plain_text;
use junto_client::state::view::ServerStatus;
use junto_client::text;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::account::{AccountStatus, Departure};
use junto_client_core::protocol::message::SendMessage;

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

    // The last workspace, on the same page: the account becomes inactive, and the window
    // says how to return.
    window.until("the page lists the one workspace that is left", |window| {
        matches!(window.account().map(|view| view.page),
            Some(PageView::Departure(page)) if page.workspaces.len() == 1)
    });
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
    // A direct conversation of the two.
    let bruno = other.own_account();
    let api = window.handle().api();
    let conversation = world.run(api.open_conversation(bruno)).unwrap();
    let line = SendMessage {
        document: crate::messages::document("A direct line"),
        reply_to: None,
        attachments: Vec::new(),
    };
    world
        .run(api.send_message(PlaceId::Direct(conversation.id), &line))
        .unwrap();

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

    // The direct conversation stays listed and readable, under the marked name, and takes
    // no message: a statement stands in place of the composer.
    let row = crate::harness::selector(format!("direct-row:{}", conversation.id));
    other.until("the conversation is listed", |other| other.shows(row));
    other.click(row);
    other.until_message("A direct line");
    assert_eq!(
        other.authors_of("A direct line"),
        ["Luca (deleted account)"]
    );
    other.until("the composer gives way to the statement", |other| {
        other.shows("departed-bar")
    });
    assert!(!other.shows("send-message"));
    let shown = other
        .view()
        .lists()
        .and_then(|lists| lists.conversation.clone())
        .unwrap();
    assert_eq!(shown.departure, Some(Departure::Deleted));
    assert_eq!(shown.name.as_deref(), Some("Luca (deleted account)"));
    let accounts = world.run(admin.handle.api().accounts()).unwrap();
    let held = accounts.iter().find(|account| account.id == luca).unwrap();
    assert_eq!(held.status, AccountStatus::Deleted);
    assert_eq!(held.name, "Luca");
}

#[gpui::test]
fn deleting_the_account_waits_for_a_session_renewal_before_removing_the_server(
    cx: &mut TestAppContext,
) {
    for close_page in [false, true] {
        deletion_during_renewal(cx, close_page);
    }
}

fn deletion_during_renewal(cx: &mut TestAppContext, close_page: bool) {
    use crate::support::FaultProxy;
    use junto_client_core::flow::access::ActionKind;

    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let handle = window.handle();
    let server_id = handle.id();
    window.open_account();
    window.click("account-nav-departure");
    window.click("account-delete");
    proxy.hold_answer_ending("DELETE", "/account");
    proxy.hold_request("POST", "/auth/challenges");
    window.click("account-delete-confirm");
    window.until("deletion is done while its session renewal waits", |_| {
        proxy.answers_waiting() == 1
            && handle.access().in_progress == Some(ActionKind::LogIn)
            && proxy
                .seen("/api/v1/auth/challenges")
                .iter()
                .any(|seen| seen.answer.is_none())
    });
    if close_page {
        window.click("account-close");
        assert!(window.account().is_none());
    }
    // Release the deletion answer, but keep the renewal holding the flow lock.
    proxy.release_answers();
    window.until("the removal was requested", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.pending().removing.contains(&server_id)
                || state.pending().remove_failed.contains_key(&server_id)
        })
    });
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.pending().remove_failed.is_empty()
    }));
    proxy.release_requests();
    window.until("the deleted server leaves the list", |window| {
        window.view().servers.is_empty()
    });
    assert_eq!(
        proxy
            .seen("/api/v1/account")
            .iter()
            .filter(|seen| seen.method == "DELETE")
            .count(),
        1
    );
    assert!(handle.device_public_key().is_err());
    assert!(
        !directory
            .path()
            .join("junto/keys")
            .join(format!("{server_id}.key"))
            .exists()
    );
    window.close();
    let reopened = junto_client_core::Client::open(junto_client_core::ClientOptions::new(
        directory.path().join("junto"),
    ))
    .unwrap();
    assert!(reopened.servers().is_empty());
}

#[gpui::test]
fn ordinary_server_removal_waits_for_session_renewal(cx: &mut TestAppContext) {
    use crate::support::FaultProxy;
    use junto_client_core::flow::access::ActionKind;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let handle = window.handle();
    let server_id = handle.id();
    proxy.hold_request("POST", "/auth/challenges");
    let workspace = world.run(handle.api().own_account()).unwrap().workspace_ids[0];
    world.run(handle.api().leave_workspace(workspace)).unwrap();
    window.until("renewal holds the action mutex", |_| {
        handle.access().in_progress == Some(ActionKind::LogIn)
            && proxy
                .seen("/api/v1/auth/challenges")
                .iter()
                .any(|seen| seen.answer.is_none())
    });
    window.click("server-menu-button");
    window.click("menu-remove-server");
    window.click("confirm");
    window.until("the ordinary removal was requested", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.pending().removing.contains(&server_id)
                || state.pending().remove_failed.contains_key(&server_id)
        })
    });
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.pending().remove_failed.is_empty()
    }));
    proxy.release_requests();
    window.until("the removed server leaves the list", |window| {
        window.view().servers.is_empty()
    });
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.client().selected().is_none())
    );
    assert!(
        !directory
            .path()
            .join("junto/keys")
            .join(format!("{server_id}.key"))
            .exists()
    );
}

#[gpui::test]
fn a_late_deletion_answer_preserves_the_replacement_account_page(cx: &mut TestAppContext) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let old = window.handle();
    let client = window
        .state
        .read_with(&window.cx, |state, _| state.client().clone());
    window.open_account();
    window.click("account-nav-departure");
    window.click("account-delete");
    proxy.hold_answer_ending("DELETE", "/account");
    window.click("account-delete-confirm");
    window.until("the server deleted the original account", |_| {
        proxy.answers_waiting() == 1
    });
    world.run(client.remove_server_when_free(old.id())).unwrap();
    window.until("the original entry is gone", |window| {
        window.view().servers.is_empty()
    });
    let code = admin.invite(&world, 1);
    window.find_server(&proxy.address());
    window.join_with_code(code.expose());
    window.until_ready();
    let replacement = window.handle();
    let owner = replacement.owner().unwrap();
    let key = replacement.device_public_key().unwrap();
    assert_ne!(replacement.entry().key(), old.entry().key());
    window.open_account();
    proxy.release_answers();
    window.until("the old deletion callback has completed", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.pending().deleting.is_empty())
    });
    assert!(window.account().is_some());
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.pending().remove_failed.is_empty()
    }));
    assert_eq!(replacement.owner(), Some(owner));
    assert_eq!(replacement.device_public_key().unwrap(), key);
    assert_eq!(client.selected(), Some(replacement.id()));
}
