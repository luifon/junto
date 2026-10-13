//! The controls of a window follow the permissions that the server states (ADR 0790): a
//! role is given, changed and taken, and a channel gets an override, while the window is
//! open. The window compares no role and no override: it reads the lists of the snapshot.

use gpui::TestAppContext;
use junto_client::state::admin::view::Scope;
use junto_client::state::admin::{AdminPage, ChannelPage};
use junto_client::state::view::Allowed;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, MessageId};
use junto_client_core::protocol::roles::{CreateWorkspaceRole, UpdateWorkspaceRole};
use junto_client_core::protocol::structure::SetChannelOverride;

use crate::admin_pages::{default_workspace, text_channel};
use crate::harness::{Window, World, member};
use crate::messages::document;
use crate::support::logged_in;

/// What the window holds in the shown channel.
fn allowed(window: &mut Window) -> Allowed {
    window
        .view()
        .workspace()
        .map(|workspace| workspace.allowed)
        .unwrap_or_default()
}

/// Whether the row of a message of another person offers its deletion and its pin.
fn offered(window: &mut Window, message_id: MessageId) -> Option<(bool, bool)> {
    window
        .messages()
        .into_iter()
        .find(|row| row.message.id == message_id)
        .map(|row| (row.can_delete, row.can_pin))
}

/// The pages that the list of pages of the administration offers.
fn pages(window: &mut Window) -> Vec<AdminPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server { nav, .. }) => nav.into_iter().flat_map(|group| group.pages).collect(),
        _ => Vec::new(),
    }
}

/// The pages of the settings of a channel, while the administration shows them.
fn settings_pages(window: &mut Window) -> Option<Vec<ChannelPage>> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Channel(settings)) => Some(settings.pages),
        _ => None,
    }
}

/// Opens the settings of the shown channel with the gear of its header, reads their
/// pages, and closes them.
fn pages_of_the_shown_channel(window: &mut Window) -> Vec<ChannelPage> {
    let mut pages = None;
    window.until("the settings of the channel are open", |window| {
        pages = settings_pages(window);
        if pages.is_some() {
            return true;
        }
        window.click_if_shown("channel-settings");
        false
    });
    window.press("escape");
    window.until("the settings are closed", |window| !window.shows("admin"));
    pages.unwrap()
}

/// The channel that the main area shows.
fn shown_channel(window: &mut Window) -> Option<ChannelId> {
    window
        .view()
        .workspace()
        .and_then(|workspace| workspace.channel.as_ref().map(|row| row.id))
}

#[gpui::test]
fn the_controls_of_a_window_follow_a_role_and_an_override_that_change_while_it_is_open(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let ada = world.owner(&server);
    let api = ada.handle.api();
    let workspace_id = default_workspace(&world, &ada.handle);
    let plans = text_channel(&world, &ada.handle, workspace_id, "plans");
    let (_directory, mut window) = member(cx, &world, &server.address(), &ada, "Bruno");
    let bruno = logged_in(
        &window
            .state
            .read_with(&window.cx, |state, _| state.client().servers())
            .remove(0),
    );
    let general = shown_channel(&mut window).unwrap();
    assert_ne!(general, plans);
    let in_general = ada.send(&world, general, document("In general")).id;
    let in_plans = ada.send(&world, plans, document("In plans")).id;
    window.until("the message of the owner is drawn", |window| {
        offered(window, in_general).is_some()
    });

    // A member without a role: the list of the workspace is the list of the "everyone"
    // role, and no control of a moderator is offered.
    assert_eq!(offered(&mut window, in_general), Some((false, false)));
    let held = allowed(&mut window);
    assert!(held.lock && held.run_meeting);
    assert!(
        !held.manage_messages
            && !held.manage
            && !held.mention_everyone
            && !held.record
            && !held.mute
            && !held.move_members
    );
    window.open_administration();
    assert!(!pages(&mut window).contains(&AdminPage::Channels));
    assert!(!pages(&mut window).contains(&AdminPage::Roles));
    assert!(!window.shows("admin-nav-channels"));

    // The owner gives a role: the message rows, the composer, the call controls and the
    // pages of the administration follow, without an act in the window.
    let role = CreateWorkspaceRole {
        name: "Stewards".to_owned(),
        permissions: vec![
            WorkspacePermission::ManageMessages,
            WorkspacePermission::ManageChannel,
            WorkspacePermission::MentionEveryone,
            WorkspacePermission::RecordCalls,
            WorkspacePermission::MuteMembers,
        ],
    };
    let stewards = world
        .run(api.create_workspace_role(workspace_id, &role))
        .unwrap();
    world
        .run(api.assign_workspace_role(workspace_id, bruno, stewards.id))
        .unwrap();
    window.until("the window offers what the role allows", |window| {
        offered(window, in_general) == Some((true, true))
    });
    let held = allowed(&mut window);
    assert!(held.manage_messages && held.manage && held.mention_everyone);
    assert!(held.record && held.mute && !held.move_members);
    window.until("the page of the channels is offered", |window| {
        pages(window).contains(&AdminPage::Channels) && window.shows("admin-nav-channels")
    });
    assert!(!pages(&mut window).contains(&AdminPage::Roles));
    window.press("escape");
    window.until("the administration is closed", |window| {
        !window.shows("admin")
    });
    // The gear of the channel opens every page of its settings for a person who holds
    // "manage channel" in it.
    assert_eq!(
        pages_of_the_shown_channel(&mut window),
        [
            ChannelPage::Overview,
            ChannelPage::Permissions,
            ChannelPage::Archive
        ]
    );

    // An override of the role in one channel denies a permission there. No role of the
    // person changed: the list of that channel is another one than the list of the
    // workspace, and the window reads it.
    world
        .run(api.set_channel_override(
            general,
            stewards.id,
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![
                    WorkspacePermission::ManageMessages,
                    WorkspacePermission::MentionEveryone,
                ],
            },
        ))
        .unwrap();
    window.until("the channel with the override offers less", |window| {
        offered(window, in_general) == Some((false, false))
    });
    let held = allowed(&mut window);
    assert!(!held.manage_messages && !held.mention_everyone);
    assert!(held.manage && held.record);
    // The other channel has the list of the workspace.
    window.click_channel("plans");
    window.until("the other channel offers what the role allows", |window| {
        shown_channel(window) == Some(plans) && offered(window, in_plans) == Some((true, true))
    });
    assert!(allowed(&mut window).mention_everyone);

    // The override allows a permission that no role of the person gives.
    world
        .run(api.set_channel_override(
            plans,
            stewards.id,
            &SetChannelOverride {
                allow: vec![WorkspacePermission::MoveMembers],
                deny: Vec::new(),
            },
        ))
        .unwrap();
    window.until(
        "the allow of the override is held in the channel",
        |window| allowed(window).move_members,
    );
    window.click_channel("general");
    window.until("the first channel is shown again", |window| {
        shown_channel(window) == Some(general)
    });
    assert!(!allowed(&mut window).move_members);

    // The role loses a permission: the holders follow.
    let fewer = UpdateWorkspaceRole {
        name: None,
        permissions: Some(vec![
            WorkspacePermission::ManageMessages,
            WorkspacePermission::RecordCalls,
        ]),
    };
    world
        .run(api.update_workspace_role(workspace_id, stewards.id, &fewer))
        .unwrap();
    window.until(
        "the gear of the settings leads to no page of a manager",
        |window| {
            let held = allowed(window);
            !held.manage && !held.mute && held.record
        },
    );
    window.open_administration();
    window.until("the page of the channels is gone", |window| {
        !pages(window).contains(&AdminPage::Channels) && !window.shows("admin-nav-channels")
    });
    window.press("escape");
    window.until("the administration is closed again", |window| {
        !window.shows("admin")
    });
    assert_eq!(
        pages_of_the_shown_channel(&mut window),
        [ChannelPage::Archive]
    );

    // The role is taken: nothing of it is left.
    world
        .run(api.unassign_workspace_role(workspace_id, bruno, stewards.id))
        .unwrap();
    window.until("the window offers nothing of the role", |window| {
        let held = allowed(window);
        !held.record && !held.manage_messages
    });
    window.click_channel("plans");
    window.until("no channel offers the deletion", |window| {
        shown_channel(window) == Some(plans) && offered(window, in_plans) == Some((false, false))
    });
    assert!(!allowed(&mut window).move_members);
}
