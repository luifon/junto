use crate::harness::{Window, World, member};
use gpui::TestAppContext;
use junto_client_core::{
    activity::ActivityState,
    protocol::{
        ids::{AccountId, ServerId},
        presence::{Presence, PresenceChoice, ReadPresence, SetPresence},
    },
};

fn value(window: &mut Window, server: ServerId, account: AccountId) -> Presence {
    window
        .state
        .read_with(&window.cx, |state, _| state.presence(server, account))
}

#[gpui::test]
fn the_status_picker_writes_all_choices_and_invisible_is_private(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    window.until("own presence is online", |window| {
        value(window, server_id, own) == Presence::Online
    });
    window.click("presence-open");
    for (index, choice, shown) in [
        (1, PresenceChoice::Idle, Presence::Idle),
        (2, PresenceChoice::DoNotDisturb, Presence::DoNotDisturb),
        (3, PresenceChoice::Invisible, Presence::Invisible),
        (0, PresenceChoice::Online, Presence::Online),
    ] {
        window.click_named(format!("presence-choice:{index}"));
        window.until("the status change is saved", |window| {
            window.state.read_with(&window.cx, |state, _| {
                state
                    .presence_server(server_id)
                    .is_some_and(|view| !view.changing && view.choice == Some(choice))
            }) && value(window, server_id, own) == shown
        });
        window.until("the status and notification preference agree", |window| {
            window.view().quiet.do_not_disturb
                == Some(junto_client::state::notices::Agreement::All(
                    choice == PresenceChoice::DoNotDisturb,
                ))
        });
        assert!(window.shows("presence-dialog"));
        let page = world
            .run(admin.handle.api().presence(&ReadPresence {
                accounts: vec![own],
                idle: false,
            }))
            .unwrap();
        assert_eq!(
            page.accounts[0].presence,
            if choice == PresenceChoice::Invisible {
                Presence::Offline
            } else {
                shown
            }
        );
    }
    window.press("escape");
    assert!(!window.shows("presence-dialog"));
    assert!(window.shows_named(format!("presence:{}", own.as_uuid().as_u128())));
}

#[gpui::test]
fn system_idle_and_remote_presence_update_people_and_both_windows(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let other = crate::support::logged_in(&admin.handle);
    world.run(async {
        admin.device.client.start_event_streams().unwrap();
        admin.device.client.select(Some(admin.handle.id()));
    });
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    window.click("members-button");
    window.until("the member badges are online", |window| {
        value(window, server_id, other) == Presence::Online
    });
    admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("Presence example"),
    );
    world
        .run(admin.handle.api().set_presence(&SetPresence {
            choice: PresenceChoice::Idle,
        }))
        .unwrap();
    window.until(
        "remote status reaches the member and message identities",
        |window| {
            value(window, server_id, other) == Presence::Idle
                && window.shows_named("presence-state:Idle".to_owned())
        },
    );
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until(
        "the conversation header shows the person's status",
        |window| window.shows("direct-header-presence"),
    );
    window.state.update(&mut window.cx, |state, cx| {
        state.use_activity_reader(|| ActivityState::Idle, cx)
    });
    window.until("system inactivity changes the automatic status", |window| {
        value(window, server_id, own) == Presence::Idle
    });
    cx.update(|cx| junto_client::shell::detached::open(window.state.clone(), cx));
    let mut detached = window.call_window();
    window.state.update(&mut window.cx, |state, cx| {
        state.use_activity_reader(|| ActivityState::Active, cx)
    });
    window.until("system input restores automatic online", |window| {
        value(window, server_id, own) == Presence::Online
    });
    detached.press("secondary-k");
    detached.click("switcher-query");
    detached.simulate(|cx| cx.simulate_input("Ada"));
    detached.until(
        "the other window draws the same person's status",
        |window| window.shows_named("presence-state:Idle".to_owned()),
    );
    world
        .run(admin.handle.api().set_presence(&SetPresence {
            choice: PresenceChoice::Online,
        }))
        .unwrap();
    detached.until("a remote change redraws the other window", |window| {
        value(window, server_id, other) == Presence::Online
            && window.shows_named("presence-state:Online".to_owned())
    });
    assert_eq!(value(&mut window, server_id, other), Presence::Online);
    detached.press("escape");
    window.click("presence-open");
    window.click_named("presence-choice:2".to_owned());
    window.until("both windows share the account choice", |window| {
        value(window, server_id, own) == Presence::DoNotDisturb
    });
    assert_eq!(value(&mut detached, server_id, own), Presence::DoNotDisturb);
    window.press("escape");
    window.state.update(&mut window.cx, |state, cx| {
        state.use_activity_reader(|| ActivityState::Unavailable, cx)
    });
    window.click("presence-open");
    assert!(window.shows("presence-activity-unavailable"));
}

#[gpui::test]
fn failed_status_changes_can_retry_and_logout_discards_held_presence(cx: &mut TestAppContext) {
    use crate::support::{Fault, FaultProxy};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Member");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    window.until("own presence is read", |window| {
        value(window, server_id, own) == Presence::Online
    });
    proxy.fail_ending("/account/presence", [Fault::Refused]);
    window.click("presence-open");
    window.click_named("presence-choice:3".to_owned());
    window.until("the failed write is visible", |window| {
        window.shows("presence-write-failed")
    });
    assert_eq!(value(&mut window, server_id, own), Presence::Online);
    window.click("presence-write-retry");
    window.until("retry saves invisible", |window| {
        value(window, server_id, own) == Presence::Invisible
    });
    proxy.hold_request("POST", "/presence");
    window.state.update(&mut window.cx, |state, cx| {
        state.retry_presence(server_id, cx)
    });
    window.until("the read is in flight", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .presence_server(server_id)
                .is_some_and(|view| view.reading)
        })
    });
    window
        .state
        .update(&mut window.cx, |state, cx| state.log_out(server_id, cx));
    proxy.release_requests();
    window.until("logout removes account-scoped presence", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.presence_server(server_id).is_none()
        })
    });
    assert_eq!(value(&mut window, server_id, own), Presence::Offline);
    assert!(!window.shows("presence-dialog"));
}
