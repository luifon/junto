use super::*;
use junto_media::camera::{CameraAccess, CameraError, CameraState};
use junto_media::share::picture::VideoPicture;
use std::collections::HashMap;

fn pictures() -> HashMap<ShareId, Arc<VideoPicture>> {
    [
        (ShareId::Own, (35, 90, 130), (180, 90, 35)),
        (ShareId::Of(person("Ana")), (110, 160, 90), (65, 60, 120)),
        (
            ShareId::Of(person("Carla")),
            (175, 120, 100),
            (60, 110, 130),
        ),
    ]
    .into_iter()
    .filter_map(|(id, left, right)| {
        let pixels = junto_media::share::synthetic::two_colours(640, 360, left, right);
        VideoPicture::new(&pixels, id == ShareId::Own, (640, 360))
            .map(|frame| (id, Arc::new(frame)))
    })
    .collect()
}
pub(super) fn draw(studio: &mut Studio) -> Result<(), String> {
    for (name, focus, hide, detached, phase) in [
        ("camera-grid", false, false, false, Phase::Connected),
        (
            "camera-no-video-hidden",
            false,
            true,
            false,
            Phase::Connected,
        ),
        (
            "call-connecting-cameras",
            false,
            false,
            false,
            Phase::Connecting,
        ),
        ("camera-speaker", true, false, false, Phase::Connected),
        (
            "camera-hide-without-video",
            false,
            true,
            false,
            Phase::Connected,
        ),
        ("call-detached", false, false, true, Phase::Connected),
        ("call-detached-speaker", true, false, true, Phase::Connected),
        (
            "call-detached-lost",
            false,
            false,
            true,
            Phase::Reconnecting,
        ),
    ] {
        let mut media = in_call(phase);
        media.voice.hide_people_without_video = hide;
        if let Some(call) = &mut media.calls.call
            && name != "camera-no-video-hidden"
            && phase != Phase::Connecting
        {
            call.camera = CameraState::Running;
            call.cameras = vec![person("Ana"), person("Carla")];
        }
        let mut screen = Screen::call(name, media);
        if detached {
            screen.size = (680.0, 470.0);
        }
        studio.draw(screen, move |shell, window, cx| {
            show_daily(shell, window, cx);
            shell.show_cameras_fixture(
                if name == "camera-no-video-hidden" {
                    HashMap::new()
                } else {
                    pictures()
                },
                focus,
                detached,
                cx,
            );
        })?;
    }
    let carla = ShareId::Of(person("Carla"));
    let mut media = with_call(in_call(Phase::Connected), |call| {
        call.shares = vec![seen("Carla", ShareKind::Screen, (1920, 1200))];
        call.watched = Some(carla);
    });
    media.voice.hide_people_without_video = true;
    let mut screen = Screen::call("call-detached-share", media).servers(vec![
        acme_with_shares(&[("Carla", ShareAudio::System)], true, |call| {
            call.recording = Some(recording(|_| {}))
        }),
        harbor(),
    ]);
    screen.size = (680.0, 470.0);
    studio.draw(screen, move |shell, window, cx| {
        show_pictures((carla, desktop_picture(1568, 980)), Vec::new())(shell, window, cx);
        shell.show_cameras_fixture(pictures(), false, true, cx);
    })?;
    for (name, error) in [
        (
            "camera-refused",
            CameraError::NoPermission(CameraAccess::Refused),
        ),
        (
            "camera-restricted",
            CameraError::NoPermission(CameraAccess::Restricted),
        ),
        ("camera-disconnected", CameraError::DeviceGone),
        ("camera-interrupted", CameraError::Interrupted),
        ("camera-unavailable", CameraError::Unsupported),
        ("camera-blocked", CameraError::Blocked),
        ("camera-not-connected", CameraError::NotConnected),
    ] {
        let mut media = in_call(Phase::Connected);
        if let Some(call) = &mut media.calls.call {
            call.camera = CameraState::Failed(error);
        }
        studio.draw(Screen::call(name, media), show_daily)?;
    }
    // A refusal of macOS: the notice has the button of the page of System Settings (ADR 0708).
    let mut media = in_call(Phase::Connected);
    if let Some(call) = &mut media.calls.call {
        call.camera = CameraState::Failed(CameraError::NoPermission(CameraAccess::Refused));
    }
    studio.draw(Screen::call("camera-refused-macos", media), |shell, window, cx| {
        shell.state().clone().update(cx, |state, cx| {
            state.use_permission_rules_of(junto_client::permissions::System::MacOs, cx);
        });
        show_daily(shell, window, cx);
    })?;
    studio.draw(
        Screen::call("camera-settings", in_call(Phase::Connected)),
        show_daily_tab(CallTab::Settings),
    )?;
    for (name, incoming, active, outcome) in [
        (
            "direct-incoming",
            true,
            false,
            junto_client::state::direct::RingOutcome::Waiting,
        ),
        (
            "direct-outgoing",
            false,
            true,
            junto_client::state::direct::RingOutcome::Waiting,
        ),
        (
            "direct-active",
            false,
            true,
            junto_client::state::direct::RingOutcome::Accepted,
        ),
        (
            "direct-detached",
            false,
            true,
            junto_client::state::direct::RingOutcome::Accepted,
        ),
        (
            "direct-declined",
            false,
            false,
            junto_client::state::direct::RingOutcome::Declined,
        ),
        (
            "direct-missed",
            true,
            false,
            junto_client::state::direct::RingOutcome::Missed,
        ),
        (
            "direct-ended",
            false,
            false,
            junto_client::state::direct::RingOutcome::Ended,
        ),
    ] {
        let (mut servers, conversation) = threads::with_directs(false);
        let place = CallPlace::direct(person("Luca"), person("Ana"));
        let invitation = CallInvitation {
            id: id(8700),
            call_id: id(8701),
            place,
            account_id: if incoming {
                person("Luca")
            } else {
                person("Ana")
            },
            invited_by: if incoming {
                person("Ana")
            } else {
                person("Luca")
            },
            created_at: at(),
            expires_at: at() + time::Duration::seconds(60),
        };
        if let Some(model) = &mut servers[0].model {
            let model = Arc::make_mut(model);
            if incoming && outcome == junto_client::state::direct::RingOutcome::Waiting {
                model.invitations.push(invitation.clone());
            }
            if outcome == junto_client::state::direct::RingOutcome::Accepted {
                let mut call = model
                    .call_at(&daily().place)
                    .cloned()
                    .ok_or("missing call fixture")?;
                call.place = place;
                model.direct_calls.push(call);
            }
        }
        let mut media = in_call(Phase::Connected);
        if active {
            if let Some(call) = &mut media.calls.call {
                call.target = CallTarget {
                    server_id: id(1),
                    place,
                };
            }
        } else {
            media.calls.call = None;
        }
        let mut screen = Screen::new(name, servers).media(media);
        if name == "direct-detached" {
            screen.size = (680.0, 470.0);
        }
        studio.draw(screen, move |shell, _, cx| {
            shell.state().clone().update(cx, |state, cx| {
                state.select_server(id(1), cx);
                state.select_conversation(id(1), conversation, cx);
                state.show_direct_fixture(id(1), person("Luca"), invitation, outcome, cx);
            });
            if name == "direct-detached" {
                shell.show_cameras_fixture(pictures(), false, true, cx);
            }
        })?;
    }
    Ok(())
}
