//! The first start, joining a server, a second server, and lists that follow the events.

use gpui::TestAppContext;
use junto_client::state::view::{Members, ServerStatus};
use junto_client::text;
use junto_client_core::book::Membership;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::invite::ApproveJoinRequest;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel, UpdateChannel};

use crate::harness::{World, member, named, open};
use crate::support::FaultProxy;

#[gpui::test]
fn the_first_start_asks_for_a_name_and_the_owner_code_opens_the_server(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());

    // Nothing but the name can be given at the first start.
    assert!(window.shows("welcome-dialog"));
    assert!(!window.shows("dialog-close"));
    window.click("continue");
    assert!(window.shows("name-error"));
    assert!(window.shows("welcome-dialog"));
    window.choose_name("  Luca ");
    assert!(!window.shows("welcome-dialog"));
    let view = window.view();
    assert_eq!(view.person.name, "Luca");
    assert!(view.servers.is_empty() && view.selected.is_none());
    assert!(window.shows("no-server"));

    // The address is typed in the server book; the dialog shows the server that answered.
    window.find_server(&server.address());
    assert!(window.shows("code-field") && window.shows("request-access"));
    assert!(
        window.view().servers.is_empty(),
        "nothing is saved before the person chooses a way in"
    );

    // Escape closes the dialog also while the code field holds the keyboard focus, and
    // nothing was saved. Enter in the address field asks the server again.
    assert!(window.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)));
    window.press("escape");
    assert!(!window.shows("join-dialog"));
    assert!(window.view().servers.is_empty());
    window.in_shell(|shell, window, cx| shell.focus_address(window, cx));
    window.press("enter");
    window.until("the join dialog opens again", |window| {
        window.shows("join-dialog")
    });

    // An empty code is not sent.
    window.click("join-with-code");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.join_error().map(str::to_owned)),
        Some(text::CODE_MISSING.to_owned())
    );
    assert!(window.view().servers.is_empty());

    // The owner code has 20 symbols: the same field takes it, and the device claims the
    // server.
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    assert!(!window.shows("join-dialog"));
    // The server is added: the field of the server book is empty for the next one.
    assert_eq!(window.in_shell(|shell, _, cx| shell.typed_address(cx)), "");
    let view = window.view();
    assert_eq!(view.servers.len(), 1);
    assert_eq!(view.servers[0].name, "Acme Studio");
    assert!(view.servers[0].selected);
    assert_eq!(view.servers[0].address, server.address()["http://".len()..]);
    assert_eq!(view.person.name, "Luca");
    let workspace = view.workspace().unwrap();
    assert_eq!(view.servers[0].workspaces.len(), 1);
    assert!(view.servers[0].workspaces[0].selected);
    let kinds: Vec<ChannelKind> = window.channels().iter().map(|row| row.kind).collect();
    assert_eq!(kinds, [ChannelKind::Text, ChannelKind::Voice]);
    assert_eq!(
        workspace.channel.as_ref().map(|channel| channel.kind),
        Some(ChannelKind::Text)
    );
    assert!(window.shows("feed") && window.shows("composer"));
    assert!(window.shows("channel-title") && window.shows("workspace-title"));

    // The account is the owner, and the server book on the disk names it.
    let handle = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    assert!(matches!(
        handle.entry().membership,
        Membership::Member { .. }
    ));
    assert!(world.run(handle.api().own_account()).unwrap().is_owner);

    // A click on a voice channel shows it with the action that enters its call, and
    // joins nothing (ADR 0735).
    window.click_channel("voice");
    assert!(window.shows("join-call") && window.shows("feed") && window.shows("composer"));
    assert!(!window.shows("call-strip") && window.view().call.is_none());
    assert_eq!(
        window
            .view()
            .workspace()
            .unwrap()
            .channel
            .as_ref()
            .unwrap()
            .kind,
        ChannelKind::Voice
    );

    // A second start on the same data directory logs in without a question.
    window.close();
    let mut again = open(cx, directory.path());
    assert!(!again.shows("welcome-dialog"));
    again.until_ready();
    assert_eq!(again.view().person.name, "Luca");
}

#[gpui::test]
fn a_code_that_the_server_refuses_is_told_and_an_invite_code_creates_the_account(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = admin.invite(&world, 1);
    let (_directory, mut window) = named(cx, "Luca");

    window.find_server(&server.address());
    window.join_with_code("AAAA-BBBB-CCCC-DDDD");
    window.until("the refusal is shown", |window| window.shows("join-error"));
    assert_eq!(
        window.in_shell(|shell, _, _| shell.join_error().map(str::to_owned)),
        Some(text::refusal(ErrorCode::InviteNotFound))
    );
    // The server is in the server book from the moment the person chose a way in, and
    // this device has no account on it.
    let view = window.view();
    assert_eq!(view.servers.len(), 1);
    assert_eq!(view.servers[0].status, ServerStatus::NoAccount(None));
    assert!(window.shows("join-dialog"));

    // The owner code of a server that has an owner is refused with its own sentence.
    window.join_with_code("AAAA-BBBB-CCCC-DDDD-EEEE");
    window.until("the second refusal is shown", |window| {
        window.in_shell(|shell, _, _| shell.join_error().map(str::to_owned))
            == Some(text::refusal(ErrorCode::OwnerAlreadyClaimed))
    });

    // Closing the dialog leaves the state of the server in the main area, with the way
    // back into the dialog.
    window.click("dialog-close");
    assert!(!window.shows("join-dialog"));
    assert!(window.shows("no-account"));
    window.click("join-server");
    assert!(window.shows("join-dialog"));

    window.join_with_code(code.expose());
    window.until_ready();
    assert!(!window.shows("join-dialog"));
    assert_eq!(window.view().person.name, "Luca");
    assert_eq!(window.channel_names().len(), 2);
}

#[gpui::test]
fn a_request_for_access_waits_and_opens_when_an_administrator_approves_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (directory, mut window) = named(cx, "Luca");

    window.find_server(&server.address());
    window.click("request-access");
    window.until("the request waits", |window| {
        window.status()
            == Some(ServerStatus::WaitingForApproval {
                name: "Luca".to_owned(),
            })
    });
    // The dialog tells that the request was sent, and can be closed. It does so when the
    // action of the dialog has ended, which is after the state of the server changed.
    window.until("the dialog tells that the request waits", |window| {
        window.shows("cancel-request")
    });
    assert!(window.shows("join-dialog"));
    assert!(!window.shows("request-access"));
    window.click("close-join");
    assert!(!window.shows("join-dialog"));
    assert!(window.shows("waiting-for-approval"));

    // The wait is in the server book on the disk: a second start shows it again.
    window.close();
    let mut window = open(cx, directory.path());
    window.until("the request still waits", |window| {
        matches!(
            window.status(),
            Some(ServerStatus::WaitingForApproval { .. })
        )
    });

    // The administrator approves. The window opens the server without a click.
    let requests = world.run(admin.handle.api().join_requests()).unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].name, "Luca");
    world
        .run(
            admin
                .handle
                .api()
                .approve_join_request(requests[0].id, &ApproveJoinRequest::default()),
        )
        .unwrap();
    window.until_ready();
    assert_eq!(window.channel_names().len(), 2);
    assert!(!window.shows("waiting-for-approval"));
}

#[gpui::test]
fn a_cancelled_request_leaves_no_server_in_the_server_book(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (directory, mut window) = named(cx, "Luca");

    window.find_server(&server.address());
    let results = window.in_shell(|shell, _, _| shell.join_results());
    window.click("request-access");
    window.until("the request waits", |window| {
        window.shows("cancel-request")
            && window.in_shell(|shell, _, _| shell.join_results()) == results + 1
    });
    window.click("cancel-request");
    window.until("the server is removed", |window| {
        window.view().servers.is_empty()
    });
    assert!(!window.shows("join-dialog"));
    assert!(window.shows("no-server"));
    window.close();
    let mut window = open(cx, directory.path());
    assert!(window.view().servers.is_empty());

    // The cancellation withdrew the request on the server, so no approval can make an
    // account for the key that the device discarded.
    assert!(
        world
            .run(admin.handle.api().join_requests())
            .unwrap()
            .is_empty()
    );

    // The same address can be entered again, and the request of before does not hold
    // the name. The server holds one request: the new one.
    window.find_server(&server.address());
    window.click("request-access");
    window.until("the second request waits", |window| {
        matches!(
            window.status(),
            Some(ServerStatus::WaitingForApproval { .. })
        )
    });
    assert_eq!(
        world.run(admin.handle.api().join_requests()).unwrap().len(),
        1
    );
}

#[gpui::test]
fn two_presses_of_the_request_make_one_request(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = named(cx, "Luca");

    window.find_server(&server.address());
    window.press_as_drawn("request-access");
    window.press_as_drawn("request-access");
    window.until("the request waits", |window| {
        window.shows("cancel-request")
            && window.status()
                == Some(ServerStatus::WaitingForApproval {
                    name: "Luca".to_owned(),
                })
    });
    assert!(!window.shows("request-access"));
    assert!(window.in_shell(|shell, _, _| shell.join_error().is_none()));
    assert_eq!(
        world.run(admin.handle.api().join_requests()).unwrap().len(),
        1
    );
}

#[gpui::test]
fn removing_a_waiting_server_withdraws_its_request(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = named(cx, "Luca");

    window.find_server(&server.address());
    window.click("request-access");
    window.until("the request waits", |window| window.shows("cancel-request"));
    window.click("close-join");
    window.click("server-menu-button");
    window.click("menu-remove-server");
    window.click("confirm");
    window.until("the server is removed", |window| {
        window.view().servers.is_empty()
    });
    assert!(
        world
            .run(admin.handle.api().join_requests())
            .unwrap()
            .is_empty()
    );

    // Asking again after the removal makes the one request that waits.
    window.find_server(&server.address());
    window.click("request-access");
    window.until("the second request waits", |window| {
        window.shows("cancel-request")
    });
    assert_eq!(
        world.run(admin.handle.api().join_requests()).unwrap().len(),
        1
    );
}

/// A removal that meets an approval which the window has not seen yet: the withdrawal is
/// refused because the key has an account, the server stays without a failure sentence, and
/// the window opens the account (ADR 0712).
#[gpui::test]
fn a_removal_that_meets_an_approval_keeps_the_server_and_tells_no_failure(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = named(cx, "Luca");

    window.find_server(&proxy.address());
    window.click("request-access");
    window.until("the request waits", |window| window.shows("cancel-request"));
    window.click("close-join");

    // The withdrawal of the removal waits at the proxy while the request is approved.
    proxy.hold_request("POST", "/join-requests/withdrawal");
    let server_id = window.view().servers[0].id;
    window.click("server-menu-button");
    window.click("menu-remove-server");
    window.click("confirm");
    window.until("the withdrawal waits", |_| {
        proxy
            .seen("/api/v1/join-requests/withdrawal")
            .iter()
            .any(|seen| seen.answer.is_none())
    });
    let requests = world.run(admin.handle.api().join_requests()).unwrap();
    world
        .run(
            admin
                .handle
                .api()
                .approve_join_request(requests[0].id, &ApproveJoinRequest::default()),
        )
        .unwrap();
    proxy.release_requests();

    window.until("the removal has its answer", |window| {
        window.state.read_with(&window.cx, |state, _| {
            !state.pending().removing.contains(&server_id)
        })
    });
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.pending().remove_failed.is_empty()
    }));
    assert_eq!(window.view().servers.len(), 1);
    window.until_ready();
}

#[gpui::test]
fn a_second_server_has_its_own_row_its_own_state_and_can_be_removed(cx: &mut TestAppContext) {
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let admin = world.owner(&second);
    let (directory, mut window) = named(cx, "Luca");

    window.find_server(&first.address());
    window.join_with_code(&first.owner_code().unwrap());
    window.until_ready();
    let code = admin.invite(&world, 1);
    window.find_server(&second.address());
    // A text that the person types while the server is added stays in the field.
    window.in_shell(|shell, window, cx| shell.type_address("next.example.test", window, cx));
    window.join_with_code(code.expose());
    window.until("the second server is shown", |window| {
        let view = window.view();
        view.servers.len() == 2
            && view.servers[1].selected
            && view.servers[1].status == ServerStatus::Ready
            && view.workspace().is_some()
    });

    // Both servers keep their state: the one that is not selected stays logged in and
    // lists its workspaces.
    let view = window.view();
    assert_eq!(
        [view.servers[0].name.as_str(), view.servers[1].name.as_str()],
        ["Acme Studio", "Harbor"]
    );
    window.until("the first server stays ready", |window| {
        let view = window.view();
        view.servers[0].status == ServerStatus::Ready && view.servers[0].workspaces.len() == 1
    });
    assert!(!window.view().servers[0].workspaces[0].selected);
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.typed_address(cx)),
        "next.example.test"
    );

    // The address of a saved server is not added twice.
    window.enter_address(&first.address());
    window.until("the address is refused", |window| {
        window.shows("address-help")
            && window.in_shell(|shell, _, _| shell.address_error().is_some())
    });
    let refused = window.in_shell(|shell, _, _| shell.address_error().map(str::to_owned));
    assert_eq!(
        refused.as_deref(),
        Some("This server is already in your server book.")
    );
    assert_eq!(window.view().servers.len(), 2);

    // A click on the first server selects it, and its lists are shown.
    window.click("server-0");
    window.until_ready();
    let view = window.view();
    assert!(view.servers[0].selected && !view.servers[1].selected);
    assert_eq!(view.selected.as_ref().unwrap().name, "Acme Studio");

    // The menu of the server removes it from this device after a question.
    window.click("server-menu-button");
    assert!(window.shows("server-menu"));
    window.click("menu-remove-server");
    assert!(window.shows("confirm-remove"));
    window.click("cancel");
    assert!(!window.shows("confirm-remove"));
    assert_eq!(window.view().servers.len(), 2);
    window.click("server-menu-button");
    window.click("menu-remove-server");
    window.click("confirm");
    window.until("one server is left", |window| {
        let view = window.view();
        view.servers.len() == 1 && view.servers[0].name == "Harbor"
    });
    window.until_ready();

    // The removal is on the disk.
    window.close();
    let mut window = open(cx, directory.path());
    assert_eq!(window.view().servers.len(), 1);
    window.until_ready();
    assert_eq!(window.view().selected.unwrap().name, "Harbor");
}

#[gpui::test]
fn the_lists_follow_the_events_of_the_server_without_a_reload(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let workspace_id = window.view().workspace().unwrap().id;
    let before = window.channel_names();
    assert_eq!(before.len(), 2);

    // A channel that the administrator creates appears, is renamed and goes.
    let api = admin.handle.api();
    let created = world
        .run(api.create_channel(
            workspace_id,
            &CreateChannel {
                name: "plans".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    window.until("the new channel is listed", |window| {
        window.channel_names().contains(&"plans".to_owned())
    });
    assert!(window.shows_channel("plans"));

    // The new channel can be selected, and a rename reaches its title.
    window.click_channel("plans");
    assert_eq!(
        window
            .view()
            .workspace()
            .unwrap()
            .channel
            .as_ref()
            .unwrap()
            .id,
        created.id
    );
    world
        .run(api.update_channel(
            created.id,
            &UpdateChannel {
                name: Some("roadmap".to_owned()),
                ..UpdateChannel::default()
            },
        ))
        .unwrap();
    window.until("the channel is renamed", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.channel.clone())
            .is_some_and(|channel| channel.name == "roadmap" && channel.id == created.id)
    });
    assert!(window.shows_channel("roadmap") && !window.shows_channel("plans"));

    // A deleted channel leaves the list, and the selection returns to the first text
    // channel.
    world.run(api.delete_channel(created.id)).unwrap();
    window.until("the channel is gone", |window| {
        window.channel_names() == before
    });
    assert_eq!(
        window
            .view()
            .workspace()
            .unwrap()
            .channel
            .as_ref()
            .unwrap()
            .kind,
        ChannelKind::Text
    );

    // The member list is read when the panel opens, and follows a new name.
    assert!(!window.shows("side-panel"));
    window.click("members-button");
    assert!(window.shows("side-panel"));
    window.until("the members are listed", |window| {
        matches!(
            &window.view().workspace().unwrap().members,
            Members::Loaded(members) if members.len() == 2
        )
    });
    assert!(window.shows_member("Ada") && window.shows_member("Luca"));
    world.run(api.update_profile("Ada Lovelace")).unwrap();
    window.until("the new name is listed", |window| {
        window.shows_member("Ada Lovelace")
    });
    window.click("close-members");
    assert!(!window.shows("side-panel"));
}

#[gpui::test]
fn a_join_that_ends_after_its_dialog_closed_changes_no_other_dialog(cx: &mut TestAppContext) {
    const JOIN: &str = "/api/v1/join";
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let _first_owner = world.owner(&first);
    let _second_owner = world.owner(&second);
    let proxy = world.run(FaultProxy::start(&first));
    let (_directory, mut window) = named(cx, "Luca");

    // The second server is in the server book without an account.
    window.find_server(&second.address());
    window.join_with_code("AAAA-BBBB-CCCC-DDDD");
    window.until("the refusal is shown", |window| window.shows("join-error"));
    window.click("dialog-close");
    let harbor = window.view().servers[0].id;

    // A join on the first server waits at the proxy, so it runs for as long as the test
    // says.
    window.find_server(&proxy.address());
    proxy.hold(JOIN);
    let results = window.in_shell(|shell, _, _| shell.join_results());
    window.join_with_code("AAAA-BBBB-CCCC-DDDD");
    window.until("the join has reached the proxy", |_| {
        !proxy.seen(JOIN).is_empty()
    });
    // The person closes that dialog while the join runs and opens the dialog of the
    // second server.
    window.press("escape");
    assert!(!window.shows("join-dialog"));
    window.in_shell(|shell, window, cx| shell.open_join_for(harbor, window, cx));
    assert_eq!(
        window.in_shell(|shell, _, _| shell.join_target()),
        Some(harbor)
    );
    assert_eq!(
        window.in_shell(|shell, _, _| shell.join_results()),
        results,
        "the join ended before the other dialog was open"
    );

    // The join of the first server ends with its refusal, and the window receives it.
    proxy.release();
    window.until("the window has the result of the join", |window| {
        window.in_shell(|shell, _, _| shell.join_results()) == results + 1
    });
    // The dialog of the second server is still about the second server and shows no
    // failure of the first.
    assert!(window.shows("join-dialog"));
    assert_eq!(
        window.in_shell(|shell, _, _| shell.join_target()),
        Some(harbor)
    );
    assert_eq!(
        window.in_shell(|shell, _, _| shell.join_error().map(str::to_owned)),
        None
    );
    assert!(!window.shows("join-error"));
}
