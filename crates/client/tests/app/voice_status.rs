use gpui::TestAppContext;
use junto_client_core::protocol::WorkspacePermission;

use crate::harness::{World, member};

#[gpui::test]
fn voice_status_can_be_edited_copied_and_cleared_and_follows_live_permissions(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    admin.allow_everyone(&world, &[WorkspacePermission::ManageChannel]);
    let channel = window
        .channels()
        .into_iter()
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    window.until("the channel management permission is current", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.id == channel && row.allowed.manage)
    });
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("voice-status-edit");
    window.type_into("voice-status-input", " Planning 🚀 ");
    proxy.fail_ending(
        &format!("/channels/{channel}/voice-status"),
        std::iter::repeat_n(
            crate::support::Fault::Busy,
            junto_client_core::api::RetryPolicy::default().attempts as usize,
        ),
    );
    window.click("voice-status-save");
    window.until(
        "a failed status edit retains its dialog for retry",
        |window| window.shows("voice-status-error"),
    );
    window.click("voice-status-save");
    window.until("the new status is drawn", |window| {
        window.shows_named(format!("voice-status:{channel}"))
            && !window.shows("voice-status-dialog")
    });
    let current =
        serde_json::to_value(world.run(admin.handle.api().channel(channel)).unwrap()).unwrap();
    assert_eq!(current["voice_status"], "Planning 🚀");
    window.right_click_named(format!("voice-status:{channel}"));
    window.click("context-action:Copy status");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Planning 🚀"
    );
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("voice-status-edit");
    window.click("voice-status-clear");
    window.until("the empty status leaves the column", |window| {
        !window.shows_named(format!("voice-status:{channel}"))
            && !window.shows("voice-status-dialog")
    });
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("voice-status-edit");
    window.type_into("voice-status-input", &"é".repeat(501));
    window.click("voice-status-save");
    window.until("an overlong status is explained", |window| {
        window.shows("voice-status-error")
    });
    window.click("voice-status-cancel");
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("voice-status-edit");
    let workspace = window.view().workspace().unwrap().id;
    let roles = world
        .run(admin.handle.api().workspace_roles(workspace))
        .unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap().id;
    world
        .run(admin.handle.api().set_channel_rule(
            channel,
            junto_client_core::protocol::structure::RuleSubject::Role { role_id: everyone },
            &junto_client_core::protocol::structure::SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::SetVoiceChannelStatus],
            },
        ))
        .unwrap();
    window.until(
        "revoking the status permission closes its editor",
        |window| !window.shows("voice-status-dialog"),
    );
}
