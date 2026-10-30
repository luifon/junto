use gpui::TestAppContext;
use junto_client_core::address::ServerAddress;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::ChannelId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};

use crate::harness::{Window, World, member};
use crate::messages::document;

fn channel(window: &mut Window, origin: &ServerAddress, id: ChannelId) -> bool {
    window.state.update(&mut window.cx, |state, cx| {
        state.open_channel_link(origin, id, cx)
    })
}

#[gpui::test]
fn channel_links_open_active_and_archived_channels_and_conversation_links_open_directs(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = admin.text_channel(&world, &mut window, "planning");
    assert!(channel(&mut window, &origin, target));
    window.until("the linked channel is open", |window| {
        window.selected_channel() == target
    });
    window.click_channel("general");
    world
        .run(admin.handle.api().archive_channel(target))
        .unwrap();
    window.until("the target leaves the active list", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    assert!(channel(&mut window, &origin, target));
    window.until("the linked archive is open", |window| {
        window.selected_channel() == target
            && window
                .view()
                .workspace()
                .is_some_and(|workspace| workspace.archived.is_some())
    });
    let own = window.own_account();
    let direct = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(direct.id),
            &SendMessage {
                document: document("A linked conversation"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation_link(&origin, direct.id, cx)
    }));
    window.until("the linked direct conversation is open", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.place == PlaceId::Direct(direct.id) && feed.loaded)
    });
}

#[gpui::test]
fn channel_links_refuse_hidden_and_missing_targets_identically_and_return_unknown_origins(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let workspace = window.view().workspace().unwrap().id;
    let everyone = world
        .run(admin.handle.api().workspace_roles(workspace))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    world
        .run(admin.handle.api().set_channel_rule(
            target,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the target is hidden", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    for id in [target, ChannelId::generate()] {
        assert!(channel(&mut window, &origin, id));
        window.until("the target is refused", |window| {
            window.shows("message-link-refusal")
        });
        assert_eq!(window.selected_channel(), selected);
        assert_eq!(
            window
                .state
                .read_with(&window.cx, |state, _| state.message_link_refusal()),
            Some(junto_client::text::MESSAGE_LINK_UNAVAILABLE)
        );
        window.click("dismiss-message-link-refusal");
    }
    let unknown = ServerAddress::parse("https://unknown.example.test").unwrap();
    assert!(!channel(&mut window, &unknown, target));
    assert_eq!(window.selected_channel(), selected);
}

#[gpui::test]
fn a_late_channel_link_does_not_take_over_navigation_or_keep_archive_readers(
    cx: &mut TestAppContext,
) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let origin = ServerAddress::parse(&proxy.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let target = admin.text_channel(&world, &mut window, "planning");
    let next = admin.text_channel(&world, &mut window, "next");
    world
        .run(admin.handle.api().archive_channel(target))
        .unwrap();
    window.until("the linked channel is archived", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    proxy.hold_answer_ending("GET", &format!("/channels/{target}"));
    assert!(channel(&mut window, &origin, target));
    window.until("the link lookup waits for its answer", |_| {
        proxy.answers_waiting() == 1
    });
    window.click_channel("next");
    proxy.release();
    let permissions = format!("/api/v1/channels/{target}/permissions");
    window.until(
        "the stale lookup released its temporary readers",
        |window| {
            proxy
                .seen(&permissions)
                .iter()
                .any(|request| request.answer.is_some())
                && window
                    .handle()
                    .live()
                    .view()
                    .model
                    .is_some_and(|model| model.admin.archived(workspace).loaded().is_none())
        },
    );
    assert_eq!(window.selected_channel(), next);
    assert!(!window.shows("message-link-refusal"));
}
