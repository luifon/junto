//! The rules of the room of a call in the window (ADR 0739), with the real server and a
//! real media server: a room that is locked, full or hidden and the request to enter,
//! meeting mode as participant and as presenter, invitations, the menu of a person with
//! the blocks, the removal and the move, and the settings of the room.

use std::time::{Duration, Instant};

use gpui::TestAppContext;
use junto_client::shell::Open;
use junto_client::state::view::{
    CallPanel, Closed, EntryState, MeetingStart, Microphone, MicrophoneButton, OwnShare, RoomView,
    VoiceState,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::call::{
    BlockReason, CallInvitation, CallPlace, EntryRequest, LeaveReason, UpdateCallSettings,
};
use junto_client_core::protocol::ids::{AccountId, ChannelId};
use junto_client_core::protocol::roles::UpdateWorkspaceRole;
use junto_client_core::protocol::structure::ChannelKind;
use junto_media::calls::{CallTarget, EndReason, Outcome, Phase};
use junto_media::share::{CaptureRequest, PictureSource, ShareQuality, SoundChoice};

use super::{ROOM, enter, occupants, own, own_share, room};
use crate::calls::voice_channel;
use crate::harness::{Admin, Caller, PATIENCE, Window, World, member};

/// One server with the administrator Ada, who is in calls without a window, and the
/// window of the member Luca.
struct Scene {
    world: World,
    admin: Admin,
    caller: Caller,
    window: Window,
    target: CallTarget,
    voice: ChannelId,
    ada: AccountId,
    luca: AccountId,
    _directory: tempfile::TempDir,
    _server: crate::support::TestServer,
}

fn scene(cx: &mut TestAppContext) -> Scene {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    let workspace_id = window.view().workspace().unwrap().id;
    let voice = room(&mut window).id;
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: voice,
        },
    };
    Scene {
        ada: admin.handle.account().unwrap(),
        luca: window.view().person.account_id.unwrap(),
        world,
        admin,
        caller,
        window,
        target,
        voice,
        _directory: directory,
        _server: server,
    }
}

impl Scene {
    /// Gives every member of the workspace these permissions too.
    fn allow_everyone(&self, more: &[WorkspacePermission]) {
        let api = self.admin.handle.api();
        let CallPlace::Channel { workspace_id, .. } = self.target.place else {
            unreachable!("the room is a voice channel");
        };
        let roles = self.world.run(api.workspace_roles(workspace_id)).unwrap();
        let everyone = roles.iter().find(|role| role.is_everyone).unwrap();
        let mut permissions = everyone.permissions.clone();
        permissions.extend_from_slice(more);
        self.world
            .run(api.update_workspace_role(
                workspace_id,
                everyone.id,
                &UpdateWorkspaceRole {
                    name: None,
                    permissions: Some(permissions),
                },
            ))
            .unwrap();
    }

    fn join_in_the_window(&mut self) {
        self.window.click_channel(ROOM);
        self.window.click("join-call");
        self.window.until("the call is connected", connected);
    }

    /// Waits for the request to enter that the people inside got.
    fn entry_request(&mut self) -> EntryRequest {
        let admin = self.admin.handle.clone();
        let mut found = None;
        self.window
            .until("the people inside have the request", |_| {
                found = admin
                    .live()
                    .view()
                    .model
                    .and_then(|model| model.entry_requests.first().cloned());
                found.is_some()
            });
        found.unwrap()
    }

    /// Waits for the invitation that the administrator got.
    fn invitation_for_ada(&mut self) -> CallInvitation {
        let admin = self.admin.handle.clone();
        let mut found = None;
        self.window.until("the invitation has arrived", |_| {
            found = admin
                .live()
                .view()
                .model
                .and_then(|model| model.incoming_invitations().next().cloned());
            found.is_some()
        });
        found.unwrap()
    }

    /// Turns meeting mode on as the administrator. The server accepts it when its
    /// admission check has seen the share, a moment after the report of the share.
    fn ada_starts_a_meeting(&mut self) {
        self.caller.calls.start_share(CaptureRequest {
            picture: PictureSource::Display(1),
            sound: SoundChoice::None,
            quality: ShareQuality::default(),
        });
        let api = self.admin.handle.api();
        let started = Instant::now();
        while let Err(error) = self.world.run(api.start_meeting(self.voice)) {
            assert!(
                started.elapsed() < PATIENCE,
                "meeting mode did not start: {error}"
            );
            self.window.cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn connected(window: &mut Window) -> bool {
    window
        .view()
        .call
        .is_some_and(|call| call.phase == Phase::Connected)
}

fn voice_of(window: &mut Window) -> VoiceState {
    room(window).voice.unwrap()
}

fn panel(window: &mut Window) -> Option<CallPanel> {
    window
        .view()
        .workspace()
        .and_then(|workspace| workspace.call.clone())
}

fn room_view(window: &mut Window) -> Option<RoomView> {
    panel(window).map(|panel| panel.room)
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_person_asks_to_enter_a_locked_room_and_sees_a_full_and_a_hidden_room(cx: &mut TestAppContext) {
    let mut scene = scene(cx);
    let api = scene.admin.handle.api();
    let voice = scene.voice;
    enter(&scene.world, &scene.caller, scene.target);
    scene.world.run(api.lock_call(voice)).unwrap();
    scene.window.until("the lock is shown", |window| {
        let voice = voice_of(window);
        voice.locked && voice.closed == Some(Closed::Locked)
    });

    // The box that asks to enter belongs to the room that the person chose.
    assert!(!scene.window.shows("entry-box"));
    scene.window.click_channel(ROOM);
    assert!(scene.window.shows("entry-box") && scene.window.shows("ask-to-enter"));
    // A join is refused, and the refusal is told.
    scene.window.click("join-call");
    scene
        .window
        .until("the refusal is told", |window| window.shows("call-notice"));
    scene.window.click("dismiss-call-notice");

    // The people inside deny the request.
    scene.window.click("ask-to-enter");
    scene.window.until("the request waits", |window| {
        voice_of(window).entry == Some(EntryState::Waiting)
    });
    assert!(scene.window.shows("withdraw-entry") && !scene.window.shows("ask-to-enter"));
    let request = scene.entry_request();
    assert_eq!(request.account_id, scene.luca);
    scene
        .world
        .run(api.answer_entry_request(request.id, false))
        .unwrap();
    scene.window.until("the denial is told", |window| {
        voice_of(window).entry == Some(EntryState::Denied)
    });
    // The person asks again and withdraws the request.
    scene.window.click("ask-to-enter");
    scene.window.until("the second request waits", |window| {
        voice_of(window).entry == Some(EntryState::Waiting)
    });
    scene.entry_request();
    scene.window.click("withdraw-entry");
    let admin = scene.admin.handle.clone();
    scene.window.until("the request is withdrawn", |window| {
        voice_of(window).entry.is_none()
            && admin
                .live()
                .view()
                .model
                .is_some_and(|model| model.entry_requests.is_empty())
    });
    // The people inside accept the third request: the device enters the locked room.
    scene.window.click("ask-to-enter");
    let request = scene.entry_request();
    scene
        .world
        .run(api.answer_entry_request(request.id, true))
        .unwrap();
    scene
        .window
        .until("the person is in the locked room", connected);
    assert!(!scene.window.shows("entry-box"));
    scene.window.until("the room shows its lock", |window| {
        room_view(window).is_some_and(|room| room.locked && room.inside == 2)
    });

    // The person inside answers a request: Ada left and asks to enter.
    scene.world.run(scene.caller.calls.leave());
    scene
        .window
        .until("Ada has left", |window| occupants(window).len() == 1);
    scene.world.run(api.request_entry(voice)).unwrap();
    scene.window.until("the request is shown", |window| {
        window.shows("entry-prompt")
    });
    let prompts = scene.window.view().entry_prompts;
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        (prompts[0].account_id, prompts[0].name.as_deref()),
        (scene.ada, Some("Ada"))
    );
    scene.window.click("entry-deny");
    scene.window.until("the request is answered", |window| {
        !window.shows("entry-prompt")
    });
    assert!(
        scene
            .world
            .run(scene.caller.calls.join(scene.target))
            .is_err()
    );
    scene.world.run(api.request_entry(voice)).unwrap();
    scene.window.until("the request is shown", |window| {
        window.shows("entry-prompt")
    });
    scene.window.click("entry-accept");
    scene.window.until("the request is answered", |window| {
        !window.shows("entry-prompt")
    });
    enter(&scene.world, &scene.caller, scene.target);
    scene
        .window
        .until("Ada is inside again", |window| occupants(window).len() == 2);

    // The lock of the room: every member has "lock room" at first.
    scene.window.click("lock-toggle");
    scene.window.until("the room is unlocked", |window| {
        room_view(window).is_some_and(|room| !room.locked)
    });
    scene.window.click("lock-toggle");
    scene.window.until("the room is locked", |window| {
        room_view(window).is_some_and(|room| room.locked)
    });
    scene.window.click("lock-toggle");
    scene.window.until("the room is unlocked", |window| {
        room_view(window).is_some_and(|room| !room.locked)
    });

    // The settings of the room need "manage channel": without it the switches take no
    // click, and a request is refused by the server and told.
    let room = room_view(&mut scene.window).unwrap();
    assert!(room.allowed.lock && !room.allowed.manage);
    scene
        .window
        .in_shell(|shell, window, cx| shell.open(Open::RoomSettings, window, cx));
    assert!(scene.window.shows("room-no-manage"));
    scene.window.click("room-hide");
    scene.window.click("room-limit-raise");
    let state = scene.window.state.clone();
    state.update(&mut scene.window.cx, |state, cx| {
        state.update_room(
            UpdateCallSettings {
                user_limit: None,
                hides_occupants: Some(true),
            },
            cx,
        );
    });
    scene
        .window
        .until("the refusal is told", |window| window.shows("call-refused"));
    let room = room_view(&mut scene.window).unwrap();
    assert_eq!((room.hides_occupants, room.user_limit), (false, None));
    scene.window.click("dismiss-call-refused");
    assert!(!scene.window.shows("call-refused"));
    scene.allow_everyone(&[WorkspacePermission::ManageChannel]);
    scene
        .window
        .until("the person may manage the room", |window| {
            !window.shows("room-no-manage")
        });
    // The first limit is the number of people inside, and a limit of one that is lowered
    // is removed.
    scene.window.click("room-limit-raise");
    scene.window.until("the limit is two", |window| {
        room_view(window).is_some_and(|room| room.user_limit == Some(2))
    });
    assert!(scene.window.shows("room-count"));
    scene.window.click("room-limit-lower");
    scene.window.until("the limit is one", |window| {
        room_view(window).is_some_and(|room| room.user_limit == Some(1))
    });
    scene.window.click("room-hide");
    scene
        .window
        .until("the room hides who is inside", |window| {
            room_view(window).is_some_and(|room| room.hides_occupants)
        });

    // The person leaves. The room is full and hides who is inside: the people outside
    // see that it is in use, and learn from a join that it is full.
    scene.window.click("leave-call");
    scene
        .window
        .until("the call has ended", |window| window.view().call.is_none());
    scene
        .window
        .until("the room is in use and hidden", |window| {
            let voice = voice_of(window);
            voice.in_use && voice.hidden && voice.occupants.is_empty() && voice.closed.is_none()
        });
    scene.window.click("join-call");
    scene
        .window
        .until("the refusal tells that it is full", |window| {
            window.shows("call-notice") && voice_of(window).closed == Some(Closed::Full)
        });
    assert!(scene.window.shows("entry-box"));
    // A full room that shows who is inside tells it under its row.
    scene
        .world
        .run(api.update_call_settings(
            voice,
            &UpdateCallSettings {
                user_limit: None,
                hides_occupants: Some(false),
            },
        ))
        .unwrap();
    scene.window.click("dismiss-call-notice");
    scene.window.until("the room is full and shown", |window| {
        let voice = voice_of(window);
        voice.full && voice.closed == Some(Closed::Full) && voice.occupants.len() == 1
    });
    let other = scene
        .window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Text)
        .unwrap();
    scene.window.click_channel(&other.name);
    assert!(!scene.window.shows("entry-box"));
    assert!(scene.window.shows_named(format!("full-note:{voice}")));
    // The request to enter passes the limit.
    scene.window.click_channel(ROOM);
    scene.window.click("ask-to-enter");
    let request = scene.entry_request();
    scene
        .world
        .run(api.answer_entry_request(request.id, true))
        .unwrap();
    scene
        .window
        .until("the person is in the full room", connected);
    scene.window.until("the room holds two people", |window| {
        room_view(window).is_some_and(|room| room.inside == 2 && room.user_limit == Some(1))
    });
    scene.window.click("leave-call");
    scene
        .window
        .until("the call has ended", |window| window.view().call.is_none());
    scene.world.run(scene.caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn meeting_mode_in_the_window_as_a_participant_and_as_the_presenter(cx: &mut TestAppContext) {
    let mut scene = scene(cx);
    let api = scene.admin.handle.api();
    let (voice, ada, luca) = (scene.voice, scene.ada, scene.luca);
    enter(&scene.world, &scene.caller, scene.target);
    scene.join_in_the_window();

    // The hand of the person.
    let hand = |window: &mut Window| window.view().call.and_then(|call| call.hand_raised);
    assert_eq!(hand(&mut scene.window), Some(false));
    scene.window.click("call-hand");
    scene.window.until("the hand is raised", |window| {
        hand(window) == Some(true) && own(window).is_some_and(|own| own.hand_raised)
    });
    scene.window.click("call-hand");
    scene
        .window
        .until("the hand is lowered", |window| hand(window) == Some(false));
    // A person who does not share cannot turn meeting mode on.
    let room = room_view(&mut scene.window).unwrap();
    assert_eq!(
        (room.meeting.is_none(), room.meeting_start),
        (true, MeetingStart::NotSharing)
    );
    scene.window.click("meeting-toggle");
    assert!(!scene.window.shows("meeting-bar") && !scene.window.shows("call-refused"));

    // Ada shares and turns meeting mode on: the bar tells who runs it, and the
    // microphone of the person is closed by the meeting.
    scene.ada_starts_a_meeting();
    scene.window.until("the meeting is shown", |window| {
        window.shows("meeting-bar")
            && own(window)
                .is_some_and(|own| own.microphone == Microphone::Held(BlockReason::Meeting))
    });
    let meeting = room_view(&mut scene.window).unwrap().meeting.unwrap();
    assert!(!meeting.by_self && !meeting.may_speak);
    assert_eq!(meeting.presenter.as_deref(), Some("Ada"));
    assert_eq!(
        scene.window.view().controls.microphone,
        MicrophoneButton::Held
    );
    assert!(!scene.window.shows("may-speak-unmute"));
    // The person raises a hand, and Ada removes the block. The microphone does not open
    // by itself: the person is told and opens it. The hand is lowered.
    scene.window.click("call-hand");
    scene
        .window
        .until("the hand is raised", |window| hand(window) == Some(true));
    scene
        .world
        .run(api.unblock_microphone(voice, luca))
        .unwrap();
    scene.window.until("the person may speak", |window| {
        window.shows("may-speak-unmute")
            && window.view().controls.microphone == MicrophoneButton::Muted
            && hand(window) == Some(false)
    });
    assert_eq!(
        own(&mut scene.window).unwrap().microphone,
        Microphone::Closed
    );
    // The statement follows the person to another channel.
    let other = scene
        .window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Text)
        .unwrap();
    scene.window.click_channel(&other.name);
    assert!(scene.window.shows("may-speak-notice") && !scene.window.shows("meeting-bar"));
    scene.window.click_channel(ROOM);
    assert!(!scene.window.shows("may-speak-notice"));
    scene.window.click("may-speak-unmute");
    scene.window.until("the microphone is open", |window| {
        !window.shows("may-speak-unmute")
            && window.view().controls.microphone == MicrophoneButton::Open
            && own(window).is_some_and(|own| matches!(own.microphone, Microphone::Open { .. }))
    });
    // Ada turns meeting mode off. The person had no block: nothing is told.
    scene.world.run(api.end_meeting(voice)).unwrap();
    scene.window.until("the meeting has ended", |window| {
        !window.shows("meeting-bar")
    });
    assert!(!scene.window.shows("call-told"));
    // In the next meeting the person keeps the block until the end. The own setting is
    // "open", so the people hear the person again, and the person is told.
    let started = Instant::now();
    while let Err(error) = scene.world.run(api.start_meeting(voice)) {
        assert!(started.elapsed() < PATIENCE, "no second meeting: {error}");
        std::thread::sleep(Duration::from_millis(200));
    }
    scene
        .window
        .until("the microphone is closed again", |window| {
            window.view().controls.microphone == MicrophoneButton::Held
        });
    scene.world.run(api.end_meeting(voice)).unwrap();
    scene.window.until("the open microphone is told", |window| {
        window.shows("call-told") && window.view().controls.microphone == MicrophoneButton::Open
    });
    scene.window.click("dismiss-call-told");
    assert!(!scene.window.shows("call-told"));

    // The person presents: Ada stops sharing, the person shares and turns meeting mode
    // on. The server accepts it when its check has seen the share; a refusal before
    // that is told.
    scene.caller.calls.stop_share();
    scene.window.click("call-share");
    scene
        .window
        .until("the screens and windows are listed", |window| {
            window.shows("share-sources")
        });
    scene.window.click("share-display:1");
    scene.window.click("share-sound-0");
    scene.window.click("start-share");
    scene.window.until("the person shares", |window| {
        own_share(window) == OwnShare::Running
            && room_view(window).is_some_and(|room| room.meeting_start == MeetingStart::Possible)
    });
    let started = Instant::now();
    loop {
        scene.window.click("meeting-toggle");
        scene.window.until("the server has answered", |window| {
            window.shows("call-refused") || window.shows("meeting-bar")
        });
        if scene.window.shows("meeting-bar") {
            break;
        }
        assert!(started.elapsed() < PATIENCE, "meeting mode did not start");
        scene.window.click("dismiss-call-refused");
        std::thread::sleep(Duration::from_millis(200));
    }
    scene.window.until("the person runs the meeting", |window| {
        room_view(window)
            .and_then(|room| room.meeting)
            .is_some_and(|meeting| meeting.by_self)
    });
    let calls = scene.caller.calls.clone();
    scene.window.until("the microphone of Ada is closed", |_| {
        calls.view().call.is_some_and(|call| {
            call.microphone_block
                .is_some_and(|block| block.reason == BlockReason::Meeting)
        })
    });
    assert!(!scene.window.shows("meeting-allow"));
    // Ada raises a hand, and the person allows her to speak.
    scene.world.run(api.raise_hand()).unwrap();
    scene.window.until("the hand of Ada is shown", |window| {
        window.shows("meeting-allow")
    });
    let meeting = room_view(&mut scene.window).unwrap().meeting.unwrap();
    assert_eq!(meeting.hands, [(ada, Some("Ada".to_owned()))]);
    scene.window.click("meeting-allow");
    scene.window.until("Ada may speak", |window| {
        let view = calls.view();
        view.muted
            && view
                .call
                .is_some_and(|call| call.microphone_block.is_none())
            && window.shows("meeting-allowed")
            && !window.shows("meeting-allow")
    });
    let meeting = room_view(&mut scene.window).unwrap().meeting.unwrap();
    assert!(meeting.hands.is_empty());
    assert_eq!(meeting.waiting, [Some("Ada".to_owned())]);
    // The presenter turns meeting mode off.
    scene.window.click("meeting-toggle");
    scene.window.until("the meeting has ended", |window| {
        !window.shows("meeting-bar") && room_view(window).is_some_and(|room| room.meeting.is_none())
    });
    scene.window.click("leave-call");
    scene
        .window
        .until("the call has ended", |window| window.view().call.is_none());
    scene.world.run(scene.caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn invitations_and_the_menu_of_a_person_with_blocks_a_move_and_a_removal(cx: &mut TestAppContext) {
    let mut scene = scene(cx);
    let api = scene.admin.handle.api();
    let (voice, ada, luca) = (scene.voice, scene.ada, scene.luca);
    let calls = scene.caller.calls.clone();
    scene.join_in_the_window();
    let other = scene
        .window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.id != voice)
        .unwrap();
    let ada_row = move || format!("occupant:{ada}");
    let ada_in = |window: &mut Window, channel_id: ChannelId| {
        window
            .channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .and_then(|row| row.voice)
            .is_some_and(|voice| {
                voice
                    .occupants
                    .iter()
                    .any(|occupant| occupant.account_id == ada)
            })
    };

    // The person invites Ada from the member list of the call. Ada declines, and the
    // person is told.
    scene.window.click("call-tab-members");
    scene.window.until("the members are listed", |window| {
        panel(window)
            .is_some_and(|panel| panel.outside.iter().any(|member| member.account_id == ada))
    });
    scene.window.click("invite-open");
    assert!(
        scene
            .window
            .in_shell(|shell, _, _| shell.member_picker_is_open())
    );
    assert!(scene.window.shows("member-picker"));
    scene.window.click_named(format!("invite:{ada}"));
    let invitation = scene.invitation_for_ada();
    assert_eq!(
        (invitation.invited_by, invitation.place),
        (luca, scene.target.place)
    );
    scene
        .window
        .until("the invitation is shown as made", |window| {
            window.shows_named(format!("withdraw-invitation:{ada}"))
        });
    scene
        .world
        .run(api.refuse_call_invitation(invitation.id))
        .unwrap();
    scene.window.until("the refusal is told", |window| {
        window.shows("call-told") && window.shows_named(format!("invite:{ada}"))
    });
    // A press outside the list of the members closes it.
    scene.window.click("dismiss-call-told");
    assert!(!scene.window.shows("member-picker"));
    // The person invites again and withdraws the invitation.
    scene.window.click("invite-open");
    scene.window.click_named(format!("invite:{ada}"));
    scene.invitation_for_ada();
    scene
        .window
        .click_named(format!("withdraw-invitation:{ada}"));
    let admin = scene.admin.handle.clone();
    scene.window.until("the invitation is withdrawn", |window| {
        window.shows_named(format!("invite:{ada}"))
            && admin
                .live()
                .view()
                .model
                .is_some_and(|model| model.incoming_invitations().next().is_none())
    });
    assert!(!scene.window.shows("call-told"));
    // The third invitation is accepted: Ada enters the call.
    scene.window.click_named(format!("invite:{ada}"));
    let invitation = scene.invitation_for_ada();
    scene
        .world
        .run(calls.join_invited(scene.target, invitation.id))
        .unwrap();
    scene.window.until("Ada is in the call", |window| {
        ada_in(window, voice) && !window.shows_named(format!("invite:{ada}"))
    });
    // Escape closes the list of the members.
    scene.window.press("escape");
    assert!(
        !scene
            .window
            .in_shell(|shell, _, _| shell.member_picker_is_open())
    );

    // The menu of a person. A member without a permission of moderation gets the volume
    // only; the person of this device has no menu.
    scene.window.right_click_named(ada_row());
    assert!(scene.window.shows("person-menu") && scene.window.shows("menu-volume"));
    for absent in ["menu-invite", "menu-move", "menu-mute", "menu-disconnect"] {
        assert!(!scene.window.shows(absent), "{absent}");
    }
    scene.window.press("escape");
    assert!(!scene.window.shows("person-menu"));
    scene.window.right_click_named(format!("occupant:{luca}"));
    assert!(!scene.window.shows("person-menu"));
    scene.allow_everyone(&[
        WorkspacePermission::MuteMembers,
        WorkspacePermission::DeafenMembers,
        WorkspacePermission::DisconnectMembers,
        WorkspacePermission::MoveMembers,
    ]);
    scene.window.until("the person may moderate", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.allowed.disconnect && workspace.allowed.move_members)
    });

    // The block of the microphone and of the sound, and their removal.
    let ada_shown = |window: &mut Window| {
        occupants(window)
            .into_iter()
            .find(|occupant| occupant.account_id == ada)
    };
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-mute");
    assert!(!scene.window.shows("person-menu"));
    scene
        .window
        .until("the microphone of Ada is blocked", |window| {
            ada_shown(window).is_some_and(|ada| {
                ada.microphone == Microphone::Held(BlockReason::Moderator)
                    && ada.held_by.as_deref() == Some("Luca")
            }) && calls
                .view()
                .call
                .is_some_and(|call| call.microphone_block.is_some())
        });
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-mute");
    scene.window.until("the block is removed", |window| {
        ada_shown(window).is_some_and(|ada| matches!(ada.microphone, Microphone::Open { .. }))
    });
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-block-sound");
    scene.window.until("the sound of Ada is blocked", |window| {
        ada_shown(window).is_some_and(|ada| ada.sound_blocked)
            && calls
                .view()
                .call
                .is_some_and(|call| call.sound_block.is_some())
    });
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-block-sound");
    scene.window.until("the sound block is removed", |window| {
        ada_shown(window).is_some_and(|ada| !ada.sound_blocked && !ada.sound_off)
    });

    // The move: the menu lists the other voice channels, and Ada is in the chosen one.
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-move");
    assert!(scene.window.shows("person-menu") && scene.window.shows("menu-back"));
    scene.window.click_named(format!("move-to:{}", other.id));
    assert!(!scene.window.shows("person-menu"));
    scene
        .window
        .until("Ada is in the other voice channel", |window| {
            ada_in(window, other.id)
                && !ada_in(window, voice)
                && calls.view().call.is_some_and(|call| {
                    call.phase == Phase::Connected
                        && call.target.place.channel_id() == Some(other.id)
                })
        });
    assert!(matches!(calls.view().outcome, Some(Outcome::Moved { .. })));
    // "Invite to my room" asks Ada back, from the menu of her row in the other channel.
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-invite");
    let invitation = scene.invitation_for_ada();
    scene
        .world
        .run(calls.join_invited(scene.target, invitation.id))
        .unwrap();
    scene
        .window
        .until("Ada is back", |window| ada_in(window, voice));
    // The keyboard opens the menu of the row that holds the focus: Shift and F10, the
    // Menu key, Enter and Space. Escape closes the menu and returns the focus to the row.
    scene.window.click_named(ada_row());
    assert!(!scene.window.shows("person-menu"));
    for keys in ["shift-f10", "enter", "space"] {
        scene.window.press(keys);
        assert!(scene.window.shows("person-menu"), "{keys}");
        assert!(
            scene
                .window
                .in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)),
            "{keys}"
        );
        assert_eq!(
            scene.window.in_shell(|shell, _, _| shell.person_menu_of()),
            Some((voice, ada))
        );
        scene.window.press("escape");
        assert!(!scene.window.shows("person-menu"), "{keys}");
    }
    // Another key opens nothing.
    scene.window.press("f10");
    assert!(!scene.window.shows("person-menu"));
    // The removal from the call.
    scene.window.right_click_named(ada_row());
    scene.window.click("menu-disconnect");
    scene.window.until("Ada is removed", |window| {
        !ada_in(window, voice) && calls.view().call.is_none()
    });
    assert!(matches!(
        calls.view().outcome,
        Some(Outcome::Ended {
            reason: EndReason::Server(Some(LeaveReason::Disconnected)),
            ..
        })
    ));
    calls.dismiss_outcome();

    // An invitation for the person: the window shows it whatever is selected. The
    // person declines the first one and accepts the second one, into a locked room.
    scene.window.click("leave-call");
    scene
        .window
        .until("the call has ended", |window| window.view().call.is_none());
    enter(&scene.world, &scene.caller, scene.target);
    scene.world.run(api.lock_call(voice)).unwrap();
    scene.world.run(api.invite_to_call(voice, luca)).unwrap();
    scene.window.until("the invitation is shown", |window| {
        window.shows("call-invitation")
    });
    let invited = scene.window.view().invitations;
    assert_eq!(invited.len(), 1);
    assert_eq!(
        (invited[0].by.as_deref(), invited[0].channel.as_deref()),
        (Some("Ada"), Some(ROOM))
    );
    scene.window.click("invitation-decline");
    scene.window.until("the invitation is gone", |window| {
        !window.shows("call-invitation")
    });
    assert!(scene.window.view().call.is_none());
    scene.world.run(api.invite_to_call(voice, luca)).unwrap();
    scene.window.until("the invitation is shown", |window| {
        window.shows("call-invitation")
    });
    scene.window.click("invitation-join");
    scene
        .window
        .until("the person is in the locked room", connected);
    assert!(!scene.window.shows("call-invitation"));
    scene.window.click("leave-call");
    scene
        .window
        .until("the call has ended", |window| window.view().call.is_none());
    scene.world.run(calls.leave());
}
