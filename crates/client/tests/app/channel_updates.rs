//! Each control of using a channel follows a change of the permissions while the window
//! is open (ADR 0790): a rule of the channel and a change of the "everyone" role take the
//! controls away and give them back, without a restart.

use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::roles::UpdateWorkspaceRole;
use junto_client_core::protocol::structure::ChannelKind;

use crate::admin_pages::{default_workspace, text_channel};
use crate::calls::voice_channel;
use crate::channel_use::{everyone, rule};
use crate::harness::{Window, World, member};
use crate::messages::document;

/// The state of each control of the shown text channel, as the window draws it.
fn drawn(window: &mut Window, message: &str) -> [bool; 7] {
    let id = window.until_message(message).message.id;
    window.hover_message(id);
    [
        window.shows("composer"),
        window.shows("attach-file"),
        window.shows_named(format!("react:{id}")),
        window.shows_named(format!("thread:{id}")),
        window.shows_named(format!("reply:{id}")),
        window.shows("pins-button"),
        !window.shows("history-not-permitted"),
    ]
}

#[gpui::test]
fn every_control_follows_a_rule_and_a_role_that_change_while_the_window_is_open(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let workspace_id = default_workspace(&world, &admin.handle);
    let role = everyone(&world, &admin, workspace_id);
    let plans = text_channel(&world, &admin.handle, workspace_id, "plans");
    admin.send(&world, plans, document("Before the window"));
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Dara");
    voice_channel(&world, &admin, &mut window, "Standup");
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Standup")
        .unwrap()
        .id;
    window.click_channel("plans");
    assert_eq!(drawn(&mut window, "Before the window"), [true; 7]);

    // One rule takes every permission of writing and reading: each control goes.
    let taken = [
        WorkspacePermission::SendMessages,
        WorkspacePermission::AttachFiles,
        WorkspacePermission::AddReactions,
        WorkspacePermission::CreateThreads,
        WorkspacePermission::ReadMessageHistory,
    ];
    rule(&world, &admin, plans, role, &[], &taken);
    window.until("every control is not available", |window| {
        window.shows("read-only-bar") && window.shows("pins-button-refused")
    });
    // The window keeps only the messages of its connection, and says that earlier
    // messages are not available.
    window.until("the earlier message is withdrawn", |window| {
        window.shows("history-not-permitted")
            && !window
                .texts()
                .iter()
                .any(|text| text == "Before the window")
    });
    let id = admin.send(&world, plans, document("During the rule")).id;
    window.until_message("During the rule");
    window.hover_message(id);
    assert!(window.shows_named(format!("react-refused:{id}")));
    assert!(window.shows_named(format!("thread-refused:{id}")));
    assert!(!window.shows_named(format!("reply:{id}")));

    // The rule is removed: each control returns.
    rule(&world, &admin, plans, role, &[], &[]);
    window.until("every control returns", |window| {
        window.shows("composer") && window.shows("pins-button")
    });
    assert_eq!(drawn(&mut window, "Before the window"), [true; 7]);

    // The "everyone" role loses "send messages" and "connect" in the workspace: the
    // composer and the join follow the role as they follow a rule.
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let held = roles
        .iter()
        .find(|held| held.is_everyone)
        .unwrap()
        .permissions
        .clone();
    let fewer: Vec<WorkspacePermission> = held
        .iter()
        .copied()
        .filter(|permission| {
            !matches!(
                permission,
                WorkspacePermission::SendMessages | WorkspacePermission::Connect
            )
        })
        .collect();
    let change = |permissions: Vec<WorkspacePermission>| UpdateWorkspaceRole {
        name: None,
        permissions: Some(permissions),
    };
    world
        .run(api.update_workspace_role(workspace_id, role, &change(fewer)))
        .unwrap();
    window.until("the composer follows the role", |window| {
        window.shows("read-only-bar") && !window.shows("composer")
    });
    window.click_channel("Standup");
    window.until("the join follows the role", |window| {
        window.shows("join-call-refused")
    });
    // A rule of the voice channel gives "connect" back there.
    rule(
        &world,
        &admin,
        voice,
        role,
        &[WorkspacePermission::Connect],
        &[],
    );
    window.until("the rule of the channel gives the join", |window| {
        window.shows("join-call")
    });
    world
        .run(api.update_workspace_role(workspace_id, role, &change(held)))
        .unwrap();
    window.click_channel("plans");
    window.until("the composer returns with the role", |window| {
        window.shows("composer")
    });
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.write("After the changes");
    window.until_message("After the changes");
}
