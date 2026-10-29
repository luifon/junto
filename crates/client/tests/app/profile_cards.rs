use crate::harness::{Window, World, member};
use gpui::TestAppContext;
use junto_client::state::profile::{ProfileRead, ProfileView};
use junto_client_core::protocol::ids::AccountId;

fn profile(window: &mut Window) -> Option<ProfileView> {
    let entity = window.shell.entity_id();
    window
        .state
        .read_with(&window.cx, |state, _| state.profile_view(entity))
}
fn ready(window: &mut Window, account: AccountId) -> bool {
    profile(window)
        .is_some_and(|view| view.account == account && matches!(view.read, ProfileRead::Shown(_)))
}
fn open_avatar(window: &mut Window, account: AccountId) {
    window.click_named(format!("presence:{}", account.as_uuid().as_u128()));
}

#[gpui::test]
fn pressing_a_message_avatar_opens_the_persons_profile_card(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let other = crate::support::logged_in(&admin.handle);
    let channel = window.selected_channel();
    let message = admin.send(
        &world,
        channel,
        crate::messages::document("A profile example"),
    );
    window.until("the message and its avatar are drawn", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message.id)
    });
    open_avatar(&mut window, other);
    assert!(window.shows("profile-card"));
    assert!(window.cx.debug_bounds("profile-card").unwrap().size.width > gpui::px(200.));
    window.until("the profile has loaded", |window| ready(window, other));
    assert_eq!(window.selected_channel(), channel);
    assert!(window.view().call.is_none());
    window.click("profile-message");
    window.until("Message opens this person's conversation", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.conversation.as_ref())
            .is_some_and(|person| person.account_id == other)
    });
    assert!(!window.shows("profile-card"));
}

#[gpui::test]
fn the_own_card_shows_roles_and_has_no_self_message_or_call(cx: &mut TestAppContext) {
    use junto_client_core::protocol::roles::CreateServerRole;
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let own = window.own_account();
    let role = world
        .run(admin.handle.api().create_server_role(&CreateServerRole {
            color: None,
            hoist: false,
            name: "Guide".to_owned(),
            permissions: Vec::new(),
        }))
        .unwrap();
    world
        .run(admin.handle.api().assign_server_role(own, role.id))
        .unwrap();
    open_avatar(&mut window, own);
    window.until("the role is drawn in the own card", |window| {
        ready(window, own) && window.shows_named(format!("profile-role:{}", role.id))
    });
    assert!(!profile(&mut window).unwrap().can_contact());
    window.click("profile-message");
    window.click("profile-call");
    assert!(window.shows("profile-card"));
    assert!(window.view().call.is_none());
    window.press("escape");
    assert!(!window.shows("profile-card"));
}

#[gpui::test]
fn profile_retry_closed_reads_unavailable_and_logout_keep_the_current_identity(
    cx: &mut TestAppContext,
) {
    use crate::support::{Fault, FaultProxy};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Member");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    proxy.fail_ending("/card", [Fault::Refused]);
    open_avatar(&mut window, own);
    window.until("a read failure is visible", |window| {
        window.shows("profile-failed")
    });
    window.click("profile-retry");
    window.until("retry reads the card", |window| ready(window, own));
    window.press("escape");
    proxy.hold_request("GET", "/card");
    open_avatar(&mut window, own);
    window.until("the held read started", |_| {
        proxy
            .all()
            .iter()
            .any(|request| request.path.ends_with("/card") && request.answer.is_none())
    });
    window.press("escape");
    let missing = AccountId::generate();
    window.cx.update(|window, cx| {
        window.dispatch_action(
            Box::new(junto_client::ui::OpenProfile {
                server: server_id,
                account: missing,
            }),
            cx,
        )
    });
    proxy.release_requests();
    window.until(
        "the closed card cannot replace the missing profile",
        |window| {
            window.shows("profile-unavailable")
                && profile(window).is_some_and(|view| view.account == missing)
        },
    );
    assert!(!profile(&mut window).unwrap().can_contact());
    window.press("escape");
    proxy.hold_request("GET", "/card");
    open_avatar(&mut window, own);
    window
        .state
        .update(&mut window.cx, |state, cx| state.log_out(server_id, cx));
    proxy.release_requests();
    window.until("logout removes the card and its data", |window| {
        !window.shows("profile-card") && profile(window).is_none()
    });
}

#[gpui::test]
fn background_server_cards_and_two_windows_keep_their_owner_and_query(cx: &mut TestAppContext) {
    let world = World::new();
    let harbor = world.server("Harbor");
    let mill = world.server("Mill");
    let a = world.owner(&harbor);
    let b = world.owner(&mill);
    let (_directory, mut window) = member(cx, &world, &harbor.address(), &a, "Member");
    let harbor_id = window.view().selected.unwrap().id;
    window.join_further(&world, &mill, &b);
    let mill_id = window.view().selected.unwrap().id;
    let ada = crate::support::logged_in(&a.handle);
    let other = crate::support::logged_in(&b.handle);
    window.press("secondary-k");
    window.click("switcher-query");
    window.cx.simulate_input("Ada");
    window.until("both server contacts are listed", |window| {
        let entity = window.shell.entity_id();
        window.state.read_with(&window.cx, |state, _| {
            state
                .switcher_view(entity)
                .is_some_and(|view| !view.reading && view.rows.len() == 2)
        })
    });
    open_avatar(&mut window, ada);
    window.until("the card is from the background server", |window| {
        ready(window, ada)
    });
    assert_eq!(
        profile(&mut window).unwrap().owner.entry.server_id,
        harbor_id
    );
    assert_eq!(window.view().selected.unwrap().id, mill_id);
    cx.update(|cx| junto_client::shell::detached::open(window.state.clone(), cx));
    let mut detached = window.call_window();
    detached.cx.update(|window, cx| {
        window.dispatch_action(
            Box::new(junto_client::ui::OpenProfile {
                server: mill_id,
                account: other,
            }),
            cx,
        )
    });
    detached.until("the second card has its own server", |window| {
        ready(window, other)
    });
    assert_eq!(profile(&mut window).unwrap().account, ada);
    window.press("escape");
    assert!(window.shows("quick-switcher"));
    window.press("enter");
    window.until(
        "Enter on the restored account control opens its card",
        |window| ready(window, ada),
    );
    window.press("escape");
    assert!(window.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)));
    window.click("switcher-query");
    window.cx.simulate_input(" zyx");
    window.until(
        "the original query remains editable after Escape",
        |window| {
            let entity = window.shell.entity_id();
            window.state.read_with(&window.cx, |state, _| {
                state
                    .switcher_view(entity)
                    .is_some_and(|view| !view.reading && view.rows.is_empty())
            })
        },
    );
    assert_eq!(profile(&mut detached).unwrap().account, other);
    detached.click("profile-message");
    window.until(
        "Message from the detached card opens the correct main conversation",
        |window| {
            window
                .view()
                .selected
                .is_some_and(|server| server.id == mill_id)
                && window
                    .view()
                    .lists()
                    .and_then(|lists| lists.conversation.as_ref())
                    .is_some_and(|person| person.account_id == other)
        },
    );
}

#[gpui::test]
fn names_and_embedded_mentions_open_profiles_with_pointer_and_keyboard(cx: &mut TestAppContext) {
    use junto_client_core::protocol::message::{Block, Document, Inline};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    let other = crate::support::logged_in(&admin.handle);
    let channel = window.selected_channel();
    let message = admin.send(
        &world,
        channel,
        Document {
            blocks: vec![Block::Paragraph {
                content: vec![
                    Inline::Text {
                        text: "Review the plans with ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Account { account_id: own },
                    Inline::Text {
                        text: " and ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Account { account_id: other },
                ],
            }],
        },
    );
    window.until("the account mentions are drawn", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message.id)
            && window.shows("profile-text")
    });
    window.click_named(format!("profile-name:{server_id}:{other}"));
    window.until("the message author opens by name", |window| {
        ready(window, other)
    });
    window.press("escape");
    let bounds = window.cx.debug_bounds("profile-text").unwrap();
    window.press_at(
        gpui::point(bounds.left() + gpui::px(2.), bounds.center().y),
        gpui::MouseButton::Left,
    );
    window.press("enter");
    window.until("Enter opens the first mentioned person", |window| {
        ready(window, own)
    });
    window.press("escape");
    window.press("right");
    window.press("space");
    window.until("the next mention opens its own account", |window| {
        ready(window, other)
    });
}

#[gpui::test]
fn a_shown_card_refreshes_public_roles_and_a_target_that_becomes_unavailable(
    cx: &mut TestAppContext,
) {
    use crate::admin_pages::{invite, joined};
    use junto_client_core::protocol::roles::CreateServerRole;
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_target_device, target) = joined(&world, &server, &code, "Target");
    let account = crate::support::logged_in(&target);
    let server_id = window.view().selected.unwrap().id;
    window.cx.update(|window, cx| {
        window.dispatch_action(
            Box::new(junto_client::ui::OpenProfile {
                server: server_id,
                account,
            }),
            cx,
        )
    });
    window.until("the other member's card is shown", |window| {
        ready(window, account)
    });
    let role = world
        .run(admin.handle.api().create_server_role(&CreateServerRole {
            color: None,
            hoist: false,
            name: "Guide".to_owned(),
            permissions: Vec::new(),
        }))
        .unwrap();
    world
        .run(admin.handle.api().assign_server_role(account, role.id))
        .unwrap();
    window
        .cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(30));
    window.until(
        "the card refreshes roles without an administrative subscription",
        |window| window.shows_named(format!("profile-role:{}", role.id)),
    );
    world.run(admin.handle.api().ban_account(account)).unwrap();
    window
        .cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(30));
    window.until("the banned account's card becomes unavailable", |window| {
        window.shows("profile-unavailable")
    });
    assert!(!profile(&mut window).unwrap().can_contact());
}
