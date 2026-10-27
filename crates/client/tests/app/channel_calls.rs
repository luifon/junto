//! The call controls of a voice channel outside a call follow its permissions (ADR 0790):
//! without "connect" the join is not available, with its reason. The calls during a call
//! are in `live::permissions`.

use gpui::TestAppContext;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::structure::ChannelKind;

use crate::admin_pages::default_workspace;
use crate::calls::voice_channel;
use crate::channel_use::{everyone, rule};
use crate::harness::{World, member};

#[gpui::test]
fn without_connect_the_join_of_a_voice_channel_is_not_available(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let workspace_id = default_workspace(&world, &admin.handle);
    let role = everyone(&world, &admin, workspace_id);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Carla");
    voice_channel(&world, &admin, &mut window, "Standup");
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Standup")
        .unwrap()
        .id;
    window.click_channel("Standup");
    window.until("the join is offered", |window| window.shows("join-call"));

    rule(
        &world,
        &admin,
        voice,
        role,
        &[],
        &[WorkspacePermission::Connect],
    );
    window.until("the join is not available", |window| {
        window.shows("join-call-refused") && !window.shows("join-call")
    });
    window.click("join-call-refused");
    window.settle();
    assert!(window.view().call.is_none());
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.calls().view().call.is_none())
    );

    // The rule is removed: the join is offered again.
    rule(&world, &admin, voice, role, &[], &[]);
    window.until("the join is offered again", |window| {
        window.shows("join-call") && !window.shows("join-call-refused")
    });
}
