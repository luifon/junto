//! The controls of the window read the permissions that the server stated (ADR 0790): the
//! list of the workspace, the list of one channel, and for an archived channel the list
//! that its route gave. No test here gives the person a role with a permission: a control
//! that followed the roles would stay off.

use std::sync::Arc;

use junto_client_core::model::{Collection, Reading, Timeline};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::call::{Call, CallParticipant, CallPlace, ChannelCall};
use junto_client_core::protocol::ids::{AccountId, CallId, ChannelId};
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::structure::{
    ArchivedChannel, Channel, ChannelKind, ChannelPermissions,
};
use junto_media::calls::{ActiveCall, CallTarget, CallsView, Phase};
use junto_media::engine::{MicrophoneState, OutputState};
use junto_media::permission::MicrophoneAccess;

use super::panel_tests::message;
use super::tests::{Scene, logged_in, scene, server, time};
use super::*;
use crate::state::feed::FeedRow;

use WorkspacePermission::{
    DeafenMembers, DisconnectMembers, LockRoom, ManageChannel, ManageMessages, MentionEveryone,
    MoveMembers, MuteMembers, RecordCalls, RunMeetingMode, ViewChannel,
};

impl Scene {
    /// The list of the workspace "Studio" that the server states.
    fn states(&mut self, permissions: &[WorkspacePermission]) {
        self.studio_mut().permissions = permissions.to_vec();
    }

    /// The list of one channel of "Studio" that the server states.
    fn states_in(&mut self, channel_id: ChannelId, permissions: &[WorkspacePermission]) {
        let lists = &mut self.studio_mut().channel_permissions;
        lists.retain(|held| held.channel_id != channel_id);
        lists.push(ChannelPermissions {
            channel_id,
            permissions: permissions.to_vec(),
        });
    }

    /// A call in a voice channel of "Studio" with these people inside.
    fn calls(&mut self, channel_id: ChannelId, inside: &[AccountId]) {
        let workspace_id = self.studio;
        self.studio_mut().calls.push(ChannelCall {
            workspace_id,
            channel_id,
            in_use: true,
            call: Some(Call {
                id: CallId::generate(),
                place: CallPlace::Channel {
                    workspace_id,
                    channel_id,
                },
                started_at: Some(time()),
                lock: None,
                meeting: None,
                hides_occupants: false,
                participants: inside
                    .iter()
                    .map(|account_id| CallParticipant {
                        avatar: None,
                        account_id: *account_id,
                        identity: format!("seat-{account_id}"),
                        joined_at: time(),
                        muted: false,
                        deafened: false,
                        microphone_block: None,
                        sound_block: None,
                        hand_raised: false,
                        screen_share: None,
                    })
                    .collect(),
                recording: None,
            }),
        });
    }
}

/// The state of the scene with the lists exactly as its snapshot states them, an empty
/// list included: no list is made from the roles here.
fn stated_by_the_server(scene: &Scene) -> ServerModel {
    ServerModel::from_snapshot(scene.snapshot.clone())
}

fn shown(server_id: ServerId, scene: &Scene, channel_id: ChannelId) -> Selection {
    Selection {
        channels: [((server_id, scene.studio), channel_id)]
            .into_iter()
            .collect(),
        ..Selection::default()
    }
}

/// The view of the scene with this channel shown, or with no channel chosen.
fn view(scene: &Scene, channel_id: Option<ChannelId>, known: &Known) -> ShellView {
    let server = server(
        "Acme Studio",
        Some(stated_by_the_server(scene)),
        logged_in(scene.own, Vec::new()),
    );
    let selection = channel_id.map_or_else(Selection::default, |channel_id| {
        shown(server.id, scene, channel_id)
    });
    derive(std::slice::from_ref(&server), &selection, known)
}

/// The state of the calls while this device is in the call of a voice channel.
fn in_call(server_id: ServerId, scene: &Scene, channel_id: ChannelId) -> Known {
    Known {
        media: MediaSnapshot {
            calls: CallsView {
                call: Some(ActiveCall {
                    camera: junto_media::camera::CameraState::Off,
                    cameras: Vec::new(),
                    target: CallTarget {
                        server_id,
                        place: CallPlace::Channel {
                            workspace_id: scene.studio,
                            channel_id,
                        },
                    },
                    identity: "seat".to_owned(),
                    phase: Phase::Connected,
                    interrupted_at: None,
                    microphone: MicrophoneState::Running,
                    access: MicrophoneAccess::Allowed,
                    input_device: None,
                    output: OutputState::Running,
                    output_device: None,
                    microphone_block: None,
                    sound_block: None,
                    share: ShareState::Off,
                    shares: Vec::new(),
                    watched: None,
                }),
                ..CallsView::default()
            },
            ..MediaSnapshot::default()
        },
        ..Known::default()
    }
}

/// A role that gives every permission. The server states the lists, so a role in the
/// state gives the window nothing.
fn role_with_everything(scene: &mut Scene) {
    let workspace_id = scene.studio;
    let role = WorkspaceRole {
        position: 1,
        id: junto_client_core::protocol::ids::WorkspaceRoleId::generate(),
        workspace_id,
        name: "Everything".to_owned(),
        permissions: WorkspacePermission::ALL.to_vec(),
        is_everyone: false,
    };
    let studio = scene.studio_mut();
    studio.own_role_ids = vec![role.id];
    studio.roles.push(role);
}

#[test]
fn the_shown_channel_offers_what_its_list_holds_and_not_what_a_role_gives() {
    let mut scene = scene();
    role_with_everything(&mut scene);
    // The server states one permission for the workspace, whatever the role holds: an
    // override can deny the rest in every channel.
    scene.states(&[ViewChannel]);
    let general = Some(scene.general);
    let held = view(&scene, general, &Known::default())
        .workspace()
        .unwrap()
        .allowed;
    assert_eq!(held, Allowed::default());
    // A list without any permission is a list too: the role gives the window nothing.
    scene.states(&[]);
    let held = view(&scene, general, &Known::default())
        .workspace()
        .unwrap()
        .allowed;
    assert_eq!(held, Allowed::default());

    // The list of the workspace holds for a channel without a list of its own.
    scene.states(&[ViewChannel, ManageMessages, MentionEveryone]);
    let held = view(&scene, general, &Known::default())
        .workspace()
        .unwrap()
        .allowed;
    assert!(held.manage_messages && held.mention_everyone && !held.manage);

    // A channel with a list of its own has that list, in both directions.
    scene.states_in(scene.general, &[ViewChannel, ManageChannel]);
    let held = view(&scene, general, &Known::default())
        .workspace()
        .unwrap()
        .allowed;
    assert!(held.manage && !held.manage_messages && !held.mention_everyone);
    let random = Some(scene.random);
    let held = view(&scene, random, &Known::default())
        .workspace()
        .unwrap()
        .allowed;
    assert!(!held.manage && held.manage_messages && held.mention_everyone);
    // Each row of the channel column carries the list of its channel.
    let workspace_view = view(&scene, random, &Known::default());
    let row = |name: &str| {
        workspace_view
            .workspace()
            .unwrap()
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .find(|row| row.name == name)
            .unwrap()
            .allowed
    };
    assert!(row("general").manage && !row("random").manage);
    assert!(row("random").manage_messages && !row("general").manage_messages);
}

#[test]
fn the_message_of_another_person_offers_deletion_and_pin_by_the_list_of_its_channel() {
    let mut scene = scene();
    role_with_everything(&mut scene);
    scene.states(&[ViewChannel]);
    let ana = AccountId::generate();
    let place = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.general,
    };
    let sent = message(place, 2, ana, "The plan");
    let offered = |scene: &Scene| {
        let mut model = stated_by_the_server(scene);
        model.timelines.push(Arc::new(Timeline {
            place: PlaceId::Channel(scene.general),
            messages: vec![Arc::new(sent.clone())],
            more_before: false,
            more_after: false,
            reading: Reading::default(),
            loaded: true,
            failed: None,
            stale: false,
            unread_after: None,
            root: None,
            forks: Vec::new(),
            anchor: None,
        }));
        let server = server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
        let view = derive(
            std::slice::from_ref(&server),
            &shown(server.id, scene, scene.general),
            &Known::default(),
        );
        let feed = view.lists().unwrap().feed.clone().unwrap();
        feed.rows
            .iter()
            .find_map(|row| match row {
                FeedRow::Message(row) => Some((row.can_delete, row.can_pin)),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(offered(&scene), (false, false));
    scene.states_in(scene.general, &[ViewChannel, ManageMessages]);
    assert_eq!(offered(&scene), (true, true));
    // The list of the workspace holds it and the list of the channel does not: an
    // override denies it in this channel.
    scene.states(&[ViewChannel, ManageMessages]);
    scene.states_in(scene.general, &[ViewChannel]);
    assert_eq!(offered(&scene), (false, false));
}

#[test]
fn the_controls_of_a_call_read_the_list_of_the_voice_channel_of_the_call() {
    let mut scene = scene();
    role_with_everything(&mut scene);
    scene.states(&[ViewChannel]);
    let (own, daily) = (scene.own, scene.daily);
    scene.calls(daily, &[own]);
    let panel = |scene: &Scene| {
        let server = server(
            "Acme Studio",
            Some(stated_by_the_server(scene)),
            logged_in(scene.own, Vec::new()),
        );
        let known = in_call(server.id, scene, daily);
        let view = derive(
            std::slice::from_ref(&server),
            &shown(server.id, scene, daily),
            &known,
        );
        view.lists().unwrap().call.clone().unwrap()
    };
    let shown_panel = panel(&scene);
    assert!(!shown_panel.recording().unwrap().may_record);
    assert!(
        !shown_panel.room().unwrap().allowed.lock && !shown_panel.room().unwrap().allowed.manage
    );
    assert_eq!(
        shown_panel.room().unwrap().meeting_start,
        MeetingStart::NoPermission
    );

    // The list of the voice channel gives what the list of the workspace does not.
    scene.states_in(
        daily,
        &[
            ViewChannel,
            RecordCalls,
            LockRoom,
            RunMeetingMode,
            ManageChannel,
        ],
    );
    let shown_panel = panel(&scene);
    assert!(shown_panel.recording().unwrap().may_record);
    assert!(shown_panel.room().unwrap().allowed.lock && shown_panel.room().unwrap().allowed.manage);
    assert_eq!(
        shown_panel.room().unwrap().meeting_start,
        MeetingStart::NotSharing
    );

    // And it takes what the list of the workspace gives.
    scene.states(&[ViewChannel, RecordCalls, LockRoom, RunMeetingMode]);
    scene.states_in(daily, &[ViewChannel]);
    let shown_panel = panel(&scene);
    assert!(
        !shown_panel.recording().unwrap().may_record && !shown_panel.room().unwrap().allowed.lock
    );
    assert_eq!(
        shown_panel.room().unwrap().meeting_start,
        MeetingStart::NoPermission
    );
}

#[test]
fn the_menu_of_a_person_reads_the_list_of_the_voice_channel_of_that_person() {
    let mut scene = scene();
    role_with_everything(&mut scene);
    scene.states(&[ViewChannel]);
    let (daily, studio) = (scene.daily, scene.studio);
    let ana = AccountId::generate();
    let focus = Channel {
        id: ChannelId::generate(),
        workspace_id: studio,
        category_id: None,
        kind: ChannelKind::Voice,
        name: "Focus".to_owned(),
        position: 4,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    };
    let focus_id = focus.id;
    scene.studio_mut().channels.push(focus);
    scene.calls(daily, &[ana]);
    let menu = |scene: &Scene| {
        view(scene, None, &Known::default())
            .person_menu(daily, ana)
            .unwrap()
    };
    let shown_menu = menu(&scene);
    assert!(shown_menu.destinations.is_empty());
    assert_eq!(
        (
            shown_menu.microphone_blocked,
            shown_menu.sound_blocked,
            shown_menu.disconnect
        ),
        (None, None, false)
    );

    // The moderation of the person follows the channel that the person is in.
    scene.states_in(
        daily,
        &[
            ViewChannel,
            MuteMembers,
            DeafenMembers,
            DisconnectMembers,
            MoveMembers,
        ],
    );
    let shown_menu = menu(&scene);
    assert_eq!(
        (
            shown_menu.microphone_blocked,
            shown_menu.sound_blocked,
            shown_menu.disconnect
        ),
        (Some(false), Some(false), true)
    );
    // A move needs "move members" in the channel that the person is moved to as well.
    assert!(shown_menu.destinations.is_empty());
    scene.states_in(focus_id, &[ViewChannel, MoveMembers]);
    let names: Vec<String> = menu(&scene)
        .destinations
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert_eq!(names, ["Focus"]);
}

#[test]
fn an_archived_channel_offers_its_restore_when_its_own_list_is_read() {
    let mut scene = scene();
    role_with_everything(&mut scene);
    // The list of the workspace holds the permission. The snapshot states no list for
    // an archived channel, and an override of the channel can deny it there.
    scene.states(&[ViewChannel, ManageChannel]);
    let general = scene.general;
    let studio = scene.studio;
    let archived = |scene: &Scene, list: Collection<Vec<WorkspacePermission>>| {
        let mut model = stated_by_the_server(scene);
        let channel = model.channel(general).unwrap().channel.clone();
        let workspace = Arc::make_mut(&mut model.workspaces[1]);
        workspace.channels.retain(|held| held.channel.id != general);
        let admin = Arc::make_mut(&mut model.admin);
        admin.archived = vec![(
            studio,
            Collection::Loaded(vec![ArchivedChannel {
                channel,
                archived_at: time(),
                deletes_at: None,
            }]),
        )];
        if !list.is_not_loaded() {
            admin.channel_permissions = vec![(general, list)];
        }
        let server = server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
        let view = derive(
            std::slice::from_ref(&server),
            &shown(server.id, scene, general),
            &Known::default(),
        );
        let workspace = view.workspace().unwrap();
        (
            workspace.archived.map(|state| state.may_restore),
            workspace.allowed,
        )
    };
    assert_eq!(
        archived(&scene, Collection::NotLoaded),
        (Some(false), Allowed::default())
    );
    assert_eq!(
        archived(&scene, Collection::Loading),
        (Some(false), Allowed::default())
    );
    let (restore, held) = archived(&scene, Collection::Loaded(vec![ViewChannel, ManageChannel]));
    assert_eq!(restore, Some(true));
    assert!(held.manage && !held.manage_messages);
    assert_eq!(
        archived(&scene, Collection::Loaded(vec![ViewChannel])).0,
        Some(false)
    );
}

#[test]
fn the_composer_and_the_message_actions_follow_the_permissions_of_using_the_channel() {
    use WorkspacePermission::{
        AddReactions, AttachFiles, CreateThreads, MentionRoles, ReadMessageHistory, SendMessages,
        SendMessagesInThreads,
    };
    let mut scene = scene();
    let ana = AccountId::generate();
    let place = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.general,
    };
    let own = message(place, 2, scene.own, "Mine");
    let plain = message(place, 3, ana, "No thread");
    let mut threaded = message(place, 4, ana, "A thread");
    threaded.reply_count = 2;
    let derived = |scene: &Scene| {
        let mut model = stated_by_the_server(scene);
        model.timelines.push(Arc::new(Timeline {
            place: PlaceId::Channel(scene.general),
            messages: vec![
                Arc::new(own.clone()),
                Arc::new(plain.clone()),
                Arc::new(threaded.clone()),
            ],
            more_before: false,
            more_after: false,
            reading: Reading::default(),
            loaded: true,
            failed: None,
            stale: false,
            unread_after: None,
            root: None,
            forks: Vec::new(),
            anchor: None,
        }));
        let server = server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
        derive(
            std::slice::from_ref(&server),
            &shown(server.id, scene, scene.general),
            &Known::default(),
        )
    };
    let rows = |view: &ShellView| -> Vec<feed::MessageRow> {
        view.lists()
            .unwrap()
            .feed
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .filter_map(|row| match row {
                FeedRow::Message(row) => Some(row.clone()),
                _ => None,
            })
            .collect()
    };
    let all = [
        ViewChannel,
        SendMessages,
        SendMessagesInThreads,
        CreateThreads,
        ReadMessageHistory,
        AttachFiles,
        AddReactions,
        MentionRoles,
    ];
    scene.states(&all);
    let view = derived(&scene);
    assert_eq!(
        view.lists().unwrap().writing,
        Writing {
            refused: None,
            attach: true,
            mention_roles: true,
            mention_everyone: false,
        }
    );
    let held = rows(&view);
    assert!(held[0].can_edit && held[1].can_reply && !held[1].react_refused);
    assert_eq!(
        (held[1].thread_refused, held[2].thread_refused),
        (None, None)
    );

    // Without "send messages": no composer, no reply, no edit of the own message.
    scene.states_in(
        scene.general,
        &[
            ViewChannel,
            SendMessagesInThreads,
            CreateThreads,
            ReadMessageHistory,
        ],
    );
    let view = derived(&scene);
    let writing = view.lists().unwrap().writing;
    assert_eq!(writing.refused, Some(SendMessages));
    assert!(!writing.attach && !writing.mention_roles);
    let held = rows(&view);
    assert!(!held[0].can_edit && held[0].can_delete && !held[1].can_reply);
    assert!(held[1].react_refused && held[1].can_react);
    assert_eq!(held[1].thread_refused, None);

    // A new thread needs "create threads" and "send messages in threads"; every thread is
    // read from the history.
    let history = [ViewChannel, SendMessages, ReadMessageHistory];
    scene.states_in(
        scene.general,
        &[&history[..], &[SendMessagesInThreads]].concat(),
    );
    let held = rows(&derived(&scene));
    assert_eq!(
        (held[1].thread_refused, held[2].thread_refused),
        (Some(CreateThreads), None)
    );
    scene.states_in(scene.general, &[&history[..], &[CreateThreads]].concat());
    let held = rows(&derived(&scene));
    assert_eq!(held[1].thread_refused, Some(SendMessagesInThreads));
    scene.states_in(
        scene.general,
        &[
            ViewChannel,
            SendMessages,
            SendMessagesInThreads,
            CreateThreads,
        ],
    );
    let held = rows(&derived(&scene));
    assert_eq!(
        (held[1].thread_refused, held[2].thread_refused),
        (Some(ReadMessageHistory), Some(ReadMessageHistory))
    );

    // The writing of a thread.
    let allowed = |held: &[WorkspacePermission]| super::allowed(held);
    assert_eq!(
        Writing::thread(&allowed(&[SendMessagesInThreads]), false).refused,
        Some(CreateThreads)
    );
    assert_eq!(
        Writing::thread(&allowed(&[SendMessagesInThreads]), true).refused,
        None
    );
    assert_eq!(
        Writing::thread(&allowed(&[SendMessages, CreateThreads]), true).refused,
        Some(SendMessagesInThreads)
    );
    assert_eq!(Writing::OPEN.refused, None);
}
