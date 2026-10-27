//! A voice channel that an administrator makes and sets in the administration, outside a
//! call (ADR 0514, ADR 0517): the next call of the channel has its limit and hides who is
//! inside.

use junto_client::state::admin::view::{ChannelPageView, Scope};
use junto_client::state::view::{Closed, VoiceState};
use junto_client_core::ServerHandle;
use junto_client_core::protocol::ids::ChannelId;
use junto_client_core::protocol::invite::CreateInvite;

use super::*;
use crate::harness::named;
use crate::support::{Device, TestServer, join, logged_in};

/// A member of the server without a window, in calls.
fn person(world: &World, server: &TestServer, owner: &ServerHandle, name: &str) -> (Admin, Caller) {
    let invite = CreateInvite {
        max_uses: Some(1),
        ..CreateInvite::default()
    };
    let code = world.run(owner.api().create_invite(&invite)).unwrap().code;
    let member = world.run(async {
        let device = Device::new();
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(&code, name)).await.unwrap();
        logged_in(&handle);
        Admin { device, handle }
    });
    let caller = member.caller(world);
    (member, caller)
}

fn voice_named(window: &mut Window, name: &str) -> Option<VoiceState> {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == name)
        .and_then(|row| row.voice)
}

/// The settings of a voice channel that the open administration shows.
fn shown_settings(window: &mut Window) -> Option<(Option<u32>, bool)> {
    match window.admin()?.scope {
        Scope::Channel(settings) => match settings.page {
            ChannelPageView::Overview(page) => page
                .voice
                .map(|voice| (voice.user_limit, voice.hides_occupants)),
            ChannelPageView::Permissions(_) | ChannelPageView::Archive(_) => None,
        },
        Scope::Server { .. } | Scope::Category(_) | Scope::Person(_) => None,
    }
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_voice_channel_made_and_set_in_the_administration_holds_its_settings_in_its_next_call(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    // The window claims the server: the owner manages the channels.
    let (_directory, mut window) = named(cx, "Ada");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    let owner = window.handle();
    let (bruno, bruno_calls) = person(&world, &server, &owner, "Bruno");
    let (_cal, cal_calls) = person(&world, &server, &owner, "Cal");
    let workspace_id = window.view().workspace().unwrap().id;

    // A new channel of the kind "Voice" on the channels page.
    window.open_administration();
    window.click("admin-nav-channels");
    window.click("channel-new");
    assert!(window.shows("channel-kind-0") && window.shows("channel-kind-1"));
    window.click("channel-kind-1");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Lounge", window, cx));
    window.click("admin-form-send");
    let model_of = |handle: &ServerHandle| handle.live().view().model;
    let mut lounge: Option<ChannelId> = None;
    window.until("a member has the voice channel", |_| {
        lounge = model_of(&bruno.handle)
            .and_then(|model| {
                model.workspace(workspace_id).and_then(|workspace| {
                    workspace
                        .channels
                        .iter()
                        .find(|held| held.channel.name == "Lounge")
                        .map(|held| held.channel.clone())
                })
            })
            .filter(|channel| channel.kind == ChannelKind::Voice)
            .map(|channel| channel.id);
        lounge.is_some()
    });
    let lounge = lounge.unwrap();
    window.until("the window lists the voice channel", |window| {
        voice_named(window, "Lounge").is_some_and(|voice| !voice.in_use)
    });

    // The settings of the channel, while no call runs: a limit of two people, and the
    // people outside do not see who is inside.
    let settings = crate::harness::selector(format!("channel-settings-{lounge}"));
    window.until("the channel has its row", |window| window.shows(settings));
    window.click(settings);
    window.until("the settings of the channel are open", |window| {
        window.shows("channel-limit") && window.shows("channel-hide")
    });
    assert_eq!(shown_settings(&mut window), Some((None, false)));
    window.click("channel-limit-raise");
    window.until("the limit is two", |window| {
        shown_settings(window) == Some((Some(2), false))
    });
    window.click("channel-hide");
    window.until("the channel hides who is inside", |window| {
        shown_settings(window) == Some((Some(2), true))
    });
    window.until("a member has the settings", |_| {
        model_of(&bruno.handle)
            .and_then(|model| model.channel(lounge).map(|held| held.channel.clone()))
            .is_some_and(|channel| channel.user_limit == Some(2) && channel.hides_occupants)
    });
    window.click("admin-close");
    assert!(!window.shows("admin"));

    // The next call of the channel: two people enter. The window sees that it is in use,
    // and not who is inside.
    let target = CallTarget {
        server_id: owner.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: lounge,
        },
    };
    enter(&world, &bruno_calls, target);
    enter(&world, &cal_calls, target);
    window.until("the room is in use and hidden", |window| {
        voice_named(window, "Lounge")
            .is_some_and(|voice| voice.in_use && voice.hidden && voice.occupants.is_empty())
    });
    let call = model_of(&bruno.handle)
        .and_then(|model| model.call_at(&target.place).cloned())
        .unwrap();
    assert!(call.hides_occupants);

    // The room is full: the join of the window is refused, and the refusal is told.
    window.click_channel("Lounge");
    window.click("join-call");
    window.until("the refusal tells that the room is full", |window| {
        window.shows("call-notice")
            && voice_named(window, "Lounge").is_some_and(|voice| voice.closed == Some(Closed::Full))
    });
    assert!(window.view().call.is_none());
    world.run(bruno_calls.calls.leave());
    world.run(cal_calls.calls.leave());
}
