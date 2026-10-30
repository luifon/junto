use crate::harness::{World, named, open};
use gpui::TestAppContext;
use junto_client_core::address::ServerAddress;
use junto_client_core::links::{self, AppLink};
use junto_client_core::protocol::ids::MessageId;

#[gpui::test]
fn an_invite_opens_the_join_step_with_the_code_without_spending_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = admin.invite(&world, 1);
    let link = links::format(&AppLink::Invite {
        server: ServerAddress::parse(&server.address()).unwrap(),
        code,
    });
    let (_directory, mut window) = named(cx, "Member");
    assert!(window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx)));
    window.until("the invited join step opens", |window| {
        window.shows("join-dialog")
    });
    assert!(window.view().servers.is_empty());
    window.click("join-with-code");
    window.until_ready();
    assert_eq!(window.view().servers.len(), 1);
    assert!(!window.shows("join-dialog"));
    // Reopening requires confirmation even for an existing account.
    window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx));
    assert!(window.shows("redeem-invite"));
    window.press("escape");
}

#[gpui::test]
fn an_invite_waits_for_the_first_start_name(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let code = world.owner(&server).invite(&world, 1);
    let link = links::format(&AppLink::Invite {
        server: ServerAddress::parse(&server.address()).unwrap(),
        code,
    });
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx));
    assert!(window.shows("welcome-dialog"));
    window.choose_name("Member");
    window.until("the invited join step follows the name", |window| {
        window.shows("join-dialog")
    });
    window.click("join-with-code");
    window.until_ready();
}

#[gpui::test]
fn an_unknown_message_server_prefills_the_address_and_invalid_links_do_nothing(
    cx: &mut TestAppContext,
) {
    let (_directory, mut window) = named(cx, "Member");
    let link = links::format(&AppLink::Message {
        server: ServerAddress::parse("https://chat.example.test:8443").unwrap(),
        id: MessageId::generate(),
    });
    assert!(window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx)));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.typed_address(cx)),
        "https://chat.example.test:8443"
    );
    assert!(!window.in_shell(|shell, window, cx| shell.open_url(
        "junto://invite?server=file%3A%2F%2F%2Ftmp&code=ABCD-EFGH-JKMN-PQRS",
        window,
        cx
    )));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.typed_address(cx)),
        "https://chat.example.test:8443"
    );
    assert!(window.view().servers.is_empty());
}

#[gpui::test]
fn a_message_url_navigates_and_a_missing_message_has_a_dismissible_refusal(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) =
        crate::harness::member(cx, &world, &server.address(), &admin, "Member");
    window.write("linked message");
    let message = window.until_message("linked message").message;
    let other = admin.text_channel(&world, &mut window, "other");
    window.click_channel("other");
    window.until("the other channel opens", |window| {
        window.selected_channel() == other
    });
    let server = ServerAddress::parse(&server.address()).unwrap();
    let link = links::format(&AppLink::Message {
        server: server.clone(),
        id: message.id,
    });
    window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx));
    window.until("the URL targets its message", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message.id && row.target)
    });
    let missing = links::format(&AppLink::Message {
        server,
        id: MessageId::generate(),
    });
    window.in_shell(|shell, window, cx| shell.open_url(&missing, window, cx));
    window.until("a missing message is refused", |window| {
        window.shows("message-link-unavailable")
    });
    window.click("dismiss-message-link");
    assert!(!window.shows("message-link-unavailable"));
}

#[gpui::test]
fn an_existing_account_confirms_an_invite_into_another_workspace(cx: &mut TestAppContext) {
    use junto_client_core::protocol::invite::CreateInvite;
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) =
        crate::harness::member(cx, &world, &server.address(), &admin, "Member");
    let account = window.own_account();
    let workspace = world
        .run(admin.handle.api().create_workspace("Lab".into()))
        .unwrap();
    let code = world
        .run(admin.handle.api().create_invite(&CreateInvite {
            workspace_ids: vec![workspace.id],
            max_uses: Some(1),
            ..CreateInvite::default()
        }))
        .unwrap()
        .code;
    let link = links::format(&AppLink::Invite {
        server: ServerAddress::parse(&server.address()).unwrap(),
        code,
    });
    window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx));
    assert!(
        window.shows("join-dialog"),
        "an existing account must be offered its invitation"
    );
    assert!(window.shows("redeem-invite"));
    assert!(
        !window.view().servers[0]
            .workspaces
            .iter()
            .any(|row| row.id == workspace.id)
    );
    window.press("escape");
    window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx));
    window.click("redeem-invite");
    window.until(
        "the invitation adds Lab to the existing account",
        |window| {
            window.own_account() == account
                && window.view().servers[0]
                    .workspaces
                    .iter()
                    .any(|row| row.id == workspace.id)
        },
    );
    assert!(!window.shows("join-dialog"));
    window.in_shell(|shell, window, cx| shell.open_url(&link, window, cx));
    window.click("redeem-invite");
    window.until("a spent invitation is refused in its form", |window| {
        window.in_shell(|shell, _, _| shell.join_error().is_some())
    });
    assert!(window.shows("join-dialog"));
}
