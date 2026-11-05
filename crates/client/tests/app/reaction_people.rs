use gpui::TestAppContext;

use crate::harness::{World, member};
use crate::messages::document;

#[gpui::test]
fn reaction_people_show_names_and_follow_reaction_changes(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.own_account();
    let sent = admin.send(
        &world,
        window.selected_channel(),
        document("confirm this plan"),
    );
    window.until_message("confirm this plan");
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    world
        .run(window.handle().api().add_reaction(sent.id, "👍"))
        .unwrap();
    window.until("the reaction is visible", |window| {
        window.shows_named(format!("reaction:{}:0", sent.id))
    });
    window.hover_named(format!("reaction:{}:0", sent.id));
    window.until("hover shows the reactors", |window| {
        window.shows_named(format!(
            "reaction-hover-person:{}",
            admin.handle.account().unwrap()
        ))
    });
    window.click_named(format!("reaction-people:{}", sent.id));
    window.until("the roster shows both people", |window| {
        window.shows_named(format!(
            "reaction-person:{}",
            admin.handle.account().unwrap()
        )) && window.shows_named(format!("reaction-person:{own}"))
    });
    let drawn = window
        .in_shell(|shell, _, _| shell.painted_readonly_text())
        .join("\n");
    assert!(drawn.contains("Ada") && drawn.contains("Luca"));
    world
        .run(admin.handle.api().remove_reaction(sent.id, "👍"))
        .unwrap();
    window.until("the removed reaction leaves the roster", |window| {
        !window.shows_named(format!(
            "reaction-person:{}",
            admin.handle.account().unwrap()
        )) && window.shows_named(format!("reaction-person:{own}"))
    });
    window.press("escape");
    window.click_named(format!("reaction:{}:0", sent.id));
    window.until(
        "pressing the reaction still toggles the own reaction",
        |window| !window.shows_named(format!("reaction:{}:0", sent.id)),
    );
}

#[gpui::test]
fn reaction_people_clear_the_names_when_history_access_ends(cx: &mut TestAppContext) {
    use junto_client_core::protocol::WorkspacePermission;
    use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    let sent = admin.send(&world, channel, document("a confirmation"));
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    window.until_message("a confirmation");
    window.right_click_named(format!("message:{}", sent.id));
    window.click("message-reaction-people");
    let author = admin.handle.account().unwrap();
    window.until("the reactor is listed", |window| {
        window.shows_named(format!("reaction-person:{author}"))
    });
    let workspace = window.view().workspace().unwrap().id;
    let api = admin.handle.api();
    let everyone = world
        .run(api.workspace_roles(workspace))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    world
        .run(api.set_channel_rule(
            channel,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the roster refuses a hidden message", |window| {
        window.shows("reaction-people-error")
    });
    assert!(!window.shows_named(format!("reaction-person:{author}")));
    assert!(!window.shows_named(format!("reaction-profile:{author}")));
    assert!(
        !window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .join("\n")
            .contains("Ada")
    );
}

#[gpui::test]
fn reaction_people_switch_emoji_and_work_in_conversations_and_threads(cx: &mut TestAppContext) {
    use junto_client_core::protocol::message::SendMessage;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let author = admin.handle.account().unwrap();
    let own = window.own_account();
    let direct = window.open_conversation_with(author, "Ada");
    let sent = world
        .run(admin.handle.api().send_message(
            direct,
            &SendMessage {
                document: document("a private confirmation"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    world
        .run(window.handle().api().add_reaction(sent.id, "🎉"))
        .unwrap();
    window.until_message("a private confirmation");
    window.click_named(format!("reaction-people:{}", sent.id));
    window.click_named("reaction-emoji:🎉".to_owned());
    window.until("each emoji has its own roster", |window| {
        window.shows_named(format!("reaction-person:{own}"))
            && !window.shows_named(format!("reaction-person:{author}"))
    });
    world
        .run(admin.handle.api().add_reaction(sent.id, "🚀"))
        .unwrap();
    window.until("a new emoji appears while the roster is open", |window| {
        window.shows_named("reaction-emoji:🚀".to_owned())
    });
    window.click_named("reaction-emoji:🚀".to_owned());
    window.until("the new emoji lists its reactor", |window| {
        window.shows_named(format!("reaction-person:{author}"))
            && !window.shows_named(format!("reaction-person:{own}"))
    });
    window.press("escape");
    window.click_channel("general");
    let root = admin.send(&world, window.selected_channel(), document("a discussion"));
    let reply = admin.reply(&world, root.id, document("a thread confirmation"));
    world
        .run(admin.handle.api().add_reaction(reply, "👍"))
        .unwrap();
    window.until_message("a discussion");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread is drawn", |window| {
        window
            .replies()
            .contains(&"a thread confirmation".to_owned())
    });
    window.click_named(format!("reaction-people:{reply}"));
    window.until("the thread roster is shown", |window| {
        window.shows_named(format!("reaction-person:{author}"))
    });
}

#[gpui::test]
fn reaction_people_page_forward_and_back_through_the_roster(cx: &mut TestAppContext) {
    use crate::support::{Device, join, logged_in};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let author = admin.handle.account().unwrap();
    let own = window.own_account();
    let sent = admin.send(
        &world,
        window.selected_channel(),
        document("three confirmations"),
    );
    let code = admin.invite(&world, 1);
    let third = world.run(async {
        let device = Device::new();
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(&code, "Wren")).await.unwrap();
        let id = logged_in(&handle);
        handle.api().add_reaction(sent.id, "👍").await.unwrap();
        id
    });
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    world
        .run(window.handle().api().add_reaction(sent.id, "👍"))
        .unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.set_reaction_page_size(1, cx)
    });
    window.until_message("three confirmations");
    window.until("the roster control appears", |window| {
        window.shows_named(format!("reaction-people:{}", sent.id))
    });
    window.click_named(format!("reaction-people:{}", sent.id));
    window.until("the first page lists its reactor", |window| {
        window.shows_named(format!("reaction-person:{author}"))
    });
    assert!(window.shows_named(format!("reaction-profile:{author}")));
    window.click("reaction-next");
    window.until("the next page lists another reactor", |window| {
        window.shows_named(format!("reaction-person:{own}"))
    });
    assert!(!window.shows_named(format!("reaction-person:{author}")));
    assert!(!window.shows_named(format!("reaction-profile:{author}")));
    assert!(window.shows_named(format!("reaction-profile:{own}")));
    window.click("reaction-next");
    window.until("the final page is reachable", |window| {
        window.shows_named(format!("reaction-person:{third}"))
    });
    assert!(window.shows_named(format!("reaction-profile:{third}")));
    assert!(!window.shows_named(format!("reaction-profile:{own}")));
    window.click("reaction-previous");
    window.until("the previous page can be read again", |window| {
        window.shows_named(format!("reaction-person:{own}"))
    });
}

#[gpui::test]
fn a_reactor_opens_its_profile_and_shows_current_presence(cx: &mut TestAppContext) {
    use junto_client::state::profile::ProfileRead;
    use junto_client_core::protocol::presence::{Presence, PresenceChoice, SetPresence};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    world.run(async {
        admin.device.client.start_event_streams().unwrap();
        admin.device.client.select(Some(admin.handle.id()));
    });
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let server_id = window.view().selected.unwrap().id;
    let author = admin.handle.account().unwrap();
    let sent = admin.send(
        &world,
        window.selected_channel(),
        document("a reviewed plan"),
    );
    world
        .run(admin.handle.api().set_presence(&SetPresence {
            choice: PresenceChoice::DoNotDisturb,
        }))
        .unwrap();
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    window.until_message("a reviewed plan");
    window.until("the roster control is drawn", |window| {
        window.shows_named(format!("reaction-people:{}", sent.id))
    });
    window.click_named(format!("reaction-people:{}", sent.id));
    window.until("the reactor is listed", |window| {
        window.shows_named(format!("reaction-person:{author}"))
    });
    assert!(
        window.shows_named(format!("reaction-profile:{author}")),
        "the reactor has a profile control"
    );
    window.until("the reactor's presence is current", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.presence(server_id, author))
            == Presence::DoNotDisturb
            && window.shows_named(format!("reaction-presence:{author}:DoNotDisturb"))
    });
    for _ in 0..3 {
        window.press("tab");
    }
    window.press("enter");
    window.until("the reactor's profile opens with the keyboard", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.profile_view(window.shell.entity_id())
            })
            .is_some_and(|view| {
                view.account == author && matches!(view.read, ProfileRead::Shown(_))
            })
    });
    window.press("escape");
    assert!(window.shows("reaction-people-dialog"));
    window.click_named(format!("reaction-profile:{author}"));
    window.until("the reactor's profile is read", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.profile_view(window.shell.entity_id())
            })
            .is_some_and(|view| {
                view.account == author && matches!(view.read, ProfileRead::Shown(_))
            })
    });
    window.press("escape");
    assert!(window.shows("reaction-people-dialog"));
    window.right_click_named(format!("reaction-profile:{author}"));
    assert!(window.shows("account-menu"));
    window.click_named(format!("account-copy-id:{author}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        author.to_string()
    );
    assert!(window.shows("reaction-people-dialog"));
    window.press("menu");
    assert!(window.shows_named(format!("account-copy-id:{author}")));
    world
        .run(admin.handle.api().remove_reaction(sent.id, "👍"))
        .unwrap();
    window.until("the removed reactor closes its identity menu", |window| {
        !window.shows("account-menu") && !window.shows_named(format!("reaction-profile:{author}"))
    });
}

fn external_link_closes_reactor_overlays(cx: &mut TestAppContext, profile: bool) {
    use crate::links::{deliver_from_outside, registered_main_window};
    use junto_client_core::{address::ServerAddress, links::AppLink};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, original) = member(cx, &world, &server.address(), &admin, "Reader");
    let mut window = registered_main_window(cx, original);
    let source = window.selected_channel();
    let destination = admin.text_channel(&world, &mut window, "destination");
    let target = admin.send(&world, destination, document("the link destination"));
    window.click_named(format!("channel-row:{source}"));
    window.until("the source channel is ready", |window| {
        window.selected_channel() == source && window.feed().is_some_and(|feed| feed.loaded)
    });
    let sent = admin.send(&world, source, document("a reviewed plan"));
    let author = admin.handle.account().unwrap();
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    window.until_message("a reviewed plan");
    window.until("the roster control is drawn", |window| {
        window.shows_named(format!("reaction-people:{}", sent.id))
    });
    window.click_named(format!("reaction-people:{}", sent.id));
    window.until("the reactor profile control is drawn", |window| {
        window.shows_named(format!("reaction-profile:{author}"))
    });
    for escape in [true, false] {
        if profile {
            window.click_named(format!("reaction-profile:{author}"));
            window.until("the reactor profile is open", |window| {
                window.shows("profile-card")
            });
        } else {
            window.right_click_named(format!("reaction-profile:{author}"));
            assert!(window.shows("account-menu"));
        }
        if escape {
            window.press("escape");
            assert!(window.shows("reaction-people-dialog"));
            assert!(!window.shows("account-menu") && !window.shows("profile-card"));
        }
    }
    let server = ServerAddress::parse(&server.address()).unwrap();
    let link = if profile {
        AppLink::Message {
            server,
            id: target.id,
        }
    } else {
        AppLink::Channel {
            server,
            id: destination,
        }
    };
    deliver_from_outside(&mut window, link);
    window.until("the external destination is loaded", |window| {
        window.selected_channel() == destination
            && window.feed().is_some_and(|feed| feed.loaded)
            && (!profile
                || window
                    .messages()
                    .iter()
                    .any(|row| row.message.id == target.id && row.target))
    });
    assert!(
        !window.shows("reaction-people-dialog"),
        "the reaction roster covers the link destination"
    );
    assert!(!window.shows("account-menu") && !window.shows("profile-card"));
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state
                .profile_view(window.shell.entity_id()))
            .is_none()
    );
    window.press("escape");
    assert!(!window.shows("reaction-people-dialog"));
}

#[gpui::test]
fn an_external_channel_link_closes_the_reactor_account_menu_and_roster(cx: &mut TestAppContext) {
    external_link_closes_reactor_overlays(cx, false);
}

#[gpui::test]
fn an_external_message_link_closes_the_reactor_profile_and_roster(cx: &mut TestAppContext) {
    external_link_closes_reactor_overlays(cx, true);
}

/// The first reaction that arrives while the menu of a message is open moves no item of the
/// menu: "Who reacted" stays in it while the message has no reaction, disabled (ADR 0730).
#[gpui::test]
fn the_first_reaction_that_arrives_while_a_message_menu_is_open_moves_no_item(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    let sent = admin.send(&world, channel, document("a plan to confirm"));
    window.until_message("a plan to confirm");
    window.right_click_message(sent.id);
    let before = window.cx.debug_bounds("message-mark-read").unwrap();
    assert!(window.shows("message-reaction-people"));
    window.click("message-reaction-people");
    assert!(
        window.shows("message-mark-read"),
        "a press on the disabled item leaves the menu open"
    );
    world
        .run(admin.handle.api().add_reaction(sent.id, "👍"))
        .unwrap();
    window.until("the message has its reaction", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == sent.id && !row.message.reactions.is_empty())
    });
    assert_eq!(window.cx.debug_bounds("message-mark-read"), Some(before));
    window.click("message-reaction-people");
    let author = admin.handle.account().unwrap();
    window.until("the reactor is listed", |window| {
        window.shows_named(format!("reaction-person:{author}"))
    });
}
