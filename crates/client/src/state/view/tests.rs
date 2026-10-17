use std::sync::Arc;

use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessState, Notice};
use junto_client_core::live::{StreamFault, StreamStatus};
use junto_client_core::model::{Loadable, ServerModel};
use junto_client_core::protocol::account::{OwnAccount, RecoveryNotice};
use junto_client_core::protocol::call::{
    BlockReason, Call, CallLock, CallParticipant, CallPlace, ChannelCall, Meeting, MicrophoneBlock,
    ScreenShare, ShareAudio, SoundBlock,
};
use junto_client_core::protocol::events::{Cursor, EndReason};
use junto_client_core::protocol::ids::{
    AccountId, CallId, CategoryId, ChannelId, DeviceId, RecordingId, RecoveryId, ServerId,
    StreamEpoch, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::recording::{Recording, RecordingState};
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::secret::LinkCode;
use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
use junto_client_core::protocol::structure::{
    Category, Channel, ChannelKind, Workspace, WorkspaceMember,
};
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::version::{Incompatibility, SupportedVersions};
use time::OffsetDateTime;

use super::*;

pub(super) fn time() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

fn reachable() -> Reach {
    Reach::Reachable {
        version: SupportedVersions::THIS_BUILD
            .negotiate(&[SupportedVersions::THIS_BUILD.current()])
            .unwrap(),
    }
}

fn channel(workspace_id: WorkspaceId, kind: ChannelKind, name: &str, position: i32) -> Channel {
    Channel {
        id: ChannelId::generate(),
        workspace_id,
        category_id: None,
        kind,
        name: name.to_owned(),
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    }
}

fn participant(account_id: AccountId) -> CallParticipant {
    CallParticipant {
        avatar: None,
        account_id,
        identity: format!("seat-{account_id}"),
        joined_at: time(),
        muted: false,
        deafened: false,
        microphone_block: None,
        sound_block: None,
        hand_raised: false,
        screen_share: None,
    }
}

/// One server with the workspaces "Studio" (default) and "Lab". "Studio" has the text
/// channels "general" and "random" and the voice channels "Daily" and "HR", which hides
/// who is inside.
pub(super) struct Scene {
    pub(super) own: AccountId,
    pub(super) studio: WorkspaceId,
    pub(super) lab: WorkspaceId,
    pub(super) general: ChannelId,
    pub(super) random: ChannelId,
    pub(super) daily: ChannelId,
    pub(super) hidden: ChannelId,
    pub(super) snapshot: Snapshot,
}

pub(super) fn scene() -> Scene {
    let own = AccountId::generate();
    let (studio, lab) = (WorkspaceId::generate(), WorkspaceId::generate());
    let general = channel(studio, ChannelKind::Text, "general", 0);
    let random = channel(studio, ChannelKind::Text, "random", 3);
    let daily = channel(studio, ChannelKind::Voice, "Daily", 1);
    let mut hidden = channel(studio, ChannelKind::Voice, "HR", 2);
    hidden.hides_occupants = true;
    let workspace = |id, name: &str, is_default, channels| WorkspaceState {
        workspace: Workspace {
            id,
            name: name.to_owned(),
            is_default,
        },
        own_role_ids: Vec::new(),
        roles: Vec::new(),
        categories: Vec::new(),
        channels,
        calls: Vec::new(),
        permissions: Vec::new(),
        channel_permissions: Vec::new(),
        notification_default: None,
    };
    let snapshot = Snapshot {
        cursor: Cursor {
            epoch: StreamEpoch::generate(),
            position: 1,
        },
        account: OwnAccount {
            id: own,
            name: "Luca".to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: vec![studio, lab],
        },
        workspaces: vec![
            workspace(lab, "Lab", false, Vec::new()),
            workspace(
                studio,
                "Studio",
                true,
                vec![
                    general.clone(),
                    daily.clone(),
                    hidden.clone(),
                    random.clone(),
                ],
            ),
        ],
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        sent_call_invitations: Vec::new(),
        call_moves: Vec::new(),
        entry_requests: Vec::new(),
        direct_mutes: Vec::new(),
        messages: Vec::new(),
        threads: Vec::new(),
        threads_more: false,
        threads_next: None,
        notifications: Vec::new(),
        notification_settings: Vec::new(),
    };
    Scene {
        own,
        studio,
        lab,
        general: general.id,
        random: random.id,
        daily: daily.id,
        hidden: hidden.id,
        snapshot,
    }
}

impl Scene {
    pub(super) fn studio_mut(&mut self) -> &mut WorkspaceState {
        &mut self.snapshot.workspaces[1]
    }

    fn call(&mut self, channel_id: ChannelId, participants: Vec<CallParticipant>) -> &mut Call {
        let workspace_id = self.studio;
        let calls = &mut self.studio_mut().calls;
        calls.push(ChannelCall {
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
                participants,
                recording: None,
            }),
        });
        calls.last_mut().unwrap().call.as_mut().unwrap()
    }

    pub(super) fn model(&self) -> ServerModel {
        ServerModel::from_snapshot(crate::state::fixtures::stated(self.snapshot.clone()))
    }

    fn server(&self) -> ServerSnapshot {
        server(
            "Acme Studio",
            Some(self.model()),
            logged_in(self.own, Vec::new()),
        )
    }
}

pub(super) fn logged_in(account_id: AccountId, recoveries: Vec<RecoveryNotice>) -> AccessState {
    AccessState::LoggedIn {
        account_id,
        device_id: DeviceId::generate(),
        session_expires_at: time(),
        recoveries,
        previous_account: None,
    }
}

pub(super) fn server(
    name: &str,
    model: Option<ServerModel>,
    access: AccessState,
) -> ServerSnapshot {
    ServerSnapshot {
        id: ServerId::generate(),
        name: name.to_owned(),
        address: "acme.example:7443".to_owned(),
        keep_connected: true,
        reach: reachable(),
        access,
        in_progress: None,
        error: None,
        stream: if model.is_some() {
            StreamStatus::Live
        } else {
            StreamStatus::Off
        },
        model: model.map(Arc::new),
    }
}

fn without_account(access: AccessState) -> ServerSnapshot {
    server("Harbor", None, access)
}

fn derive_one(server: &ServerSnapshot) -> ShellView {
    derive(
        std::slice::from_ref(server),
        &Selection::default(),
        &Known::default(),
    )
}

fn rows(workspace: &WorkspaceView) -> Vec<(&SectionTitle, Vec<&str>)> {
    workspace
        .sections
        .iter()
        .map(|section| {
            (
                &section.title,
                section.rows.iter().map(|row| row.name.as_str()).collect(),
            )
        })
        .collect()
}

fn voice<'a>(workspace: &'a WorkspaceView, name: &str) -> &'a VoiceState {
    workspace
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .find(|row| row.name == name)
        .unwrap()
        .voice
        .as_ref()
        .unwrap()
}

#[test]
fn the_first_server_its_default_workspace_and_its_first_text_channel_are_shown_at_the_start() {
    let scene = scene();
    let servers = [
        scene.server(),
        without_account(AccessState::NoAccount { notice: None }),
    ];
    let view = derive(&servers, &Selection::default(), &Known::default());

    assert_eq!(view.servers.len(), 2);
    assert!(view.servers[0].selected && !view.servers[1].selected);
    assert_eq!(view.servers[0].status, ServerStatus::Ready);
    let names: Vec<(&str, bool)> = view.servers[0]
        .workspaces
        .iter()
        .map(|row| (row.name.as_str(), row.selected))
        .collect();
    assert_eq!(names, [("Lab", false), ("Studio", true)]);
    assert!(view.servers[1].workspaces.is_empty());

    let selected = view.selected.as_ref().unwrap();
    assert_eq!(selected.id, servers[0].id);
    let workspace = view.workspace().unwrap();
    assert_eq!(workspace.id, scene.studio);
    assert_eq!(
        rows(workspace),
        [
            (&SectionTitle::TextChannels, vec!["general", "random"]),
            (&SectionTitle::VoiceChannels, vec!["Daily", "HR"]),
        ]
    );
    assert_eq!(workspace.channel.as_ref().unwrap().id, scene.general);
    assert_eq!(
        view.shown(),
        Shown {
            server: Some(servers[0].id),
            workspace: Some(scene.studio),
            channel: Some(scene.general),
            ..Shown::default()
        }
    );
    assert_eq!(
        view.person,
        Person {
            account_id: Some(scene.own),
            name: "Luca".to_owned()
        }
    );
}

#[test]
fn channel_selection_is_scoped_to_server_when_workspace_ids_match() {
    let scene = scene();
    // Two servers state the same workspace with the same channels.
    let servers = [scene.server(), scene.server()];
    assert_ne!(servers[0].id, servers[1].id);
    let shown_channel = |selection: &Selection| {
        derive(&servers, selection, &Known::default())
            .shown()
            .channel
    };
    let mut selection = Selection {
        server: Some(servers[0].id),
        ..Selection::default()
    };
    selection
        .channels
        .insert((servers[0].id, scene.studio), scene.daily);
    assert_eq!(shown_channel(&selection), Some(scene.daily));
    // The second server shows its own choice, and its first channel without one.
    selection.server = Some(servers[1].id);
    assert_eq!(shown_channel(&selection), Some(scene.general));
    selection
        .channels
        .insert((servers[1].id, scene.studio), scene.random);
    assert_eq!(shown_channel(&selection), Some(scene.random));
    // The choice of the second server leaves the first one as it was.
    selection.server = Some(servers[0].id);
    assert_eq!(shown_channel(&selection), Some(scene.daily));
}

#[test]
fn pending_upload_progress_is_scoped_to_server() {
    use junto_client_core::transfers::{TransferFailure, UploadStage};

    let scene = scene();
    let servers = [scene.server(), scene.server()];
    let progress = |stage, sent| UploadProgress {
        stage,
        sent,
        bytes: 1_000,
        interrupted: false,
    };
    // Each server numbers its uploads from 1: both first uploads have the number 1.
    let sending = progress(UploadStage::Sending, 400);
    let failed = progress(UploadStage::Failed(TransferFailure::Interrupted), 0);
    let known = Known {
        uploads: HashMap::from([((servers[0].id, 1), sending), ((servers[1].id, 1), failed)]),
        ..Known::default()
    };
    for (index, expected) in [(0, sending), (1, failed)] {
        let selection = Selection {
            server: Some(servers[index].id),
            ..Selection::default()
        };
        let view = derive(&servers, &selection, &known);
        let uploads = &view.workspace().unwrap().uploads;
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads.get(&1), Some(&expected));
    }
}

#[test]
fn a_selection_is_followed_while_the_state_holds_it_and_replaced_when_it_is_gone() {
    let scene = scene();
    let servers = [
        scene.server(),
        without_account(AccessState::NoAccount { notice: None }),
    ];
    let mut selection = Selection {
        server: Some(servers[0].id),
        ..Selection::default()
    };
    selection
        .channels
        .insert((servers[0].id, scene.studio), scene.daily);
    let view = derive(&servers, &selection, &Known::default());
    let workspace = view.workspace().unwrap();
    let shown = workspace.channel.as_ref().unwrap();
    assert_eq!((shown.id, shown.kind), (scene.daily, ChannelKind::Voice));
    let selected: Vec<&str> = workspace
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .filter(|row| row.selected)
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(selected, ["Daily"]);

    // The other workspace, which has no channel.
    selection.workspaces.insert(servers[0].id, scene.lab);
    let view = derive(&servers, &selection, &Known::default());
    let workspace = view.workspace().unwrap();
    assert_eq!(workspace.id, scene.lab);
    assert!(workspace.sections.is_empty() && workspace.channel.is_none());
    assert!(view.servers[0].workspaces[0].selected);

    // A channel, a workspace and a server that are gone.
    selection
        .channels
        .insert((servers[0].id, scene.studio), ChannelId::generate());
    selection
        .workspaces
        .insert(servers[0].id, WorkspaceId::generate());
    let view = derive(&servers, &selection, &Known::default());
    assert_eq!(view.shown().workspace, Some(scene.studio));
    assert_eq!(view.shown().channel, Some(scene.general));
    selection.server = Some(ServerId::generate());
    assert_eq!(
        derive(&servers, &selection, &Known::default())
            .shown()
            .server,
        Some(servers[0].id)
    );

    // The second server, which has no lists.
    selection.server = Some(servers[1].id);
    let view = derive(&servers, &selection, &Known::default());
    assert!(view.servers[1].selected && !view.servers[0].selected);
    assert!(
        view.servers[0]
            .workspaces
            .iter()
            .all(|workspace| !workspace.selected)
    );
    assert_eq!(view.selected.as_ref().unwrap().content, Content::State);
    assert_eq!(view.shown().workspace, None);
}

#[test]
fn channels_without_a_category_come_first_by_kind_and_then_each_category_with_its_channels() {
    let mut scene = scene();
    let workspace_id = scene.studio;
    let category = |name: &str, position| Category {
        id: CategoryId::generate(),
        workspace_id,
        name: name.to_owned(),
        position,
    };
    let (projects, empty) = (category("Projects", 0), category("Empty", 1));
    let random = scene.random;
    let daily = scene.daily;
    let studio = scene.studio_mut();
    for held in &mut studio.channels {
        if held.id == random || held.id == daily {
            held.category_id = Some(projects.id);
        }
        if held.name == "HR" {
            // A category that the account cannot see.
            held.category_id = Some(CategoryId::generate());
        }
    }
    studio.categories = vec![projects, empty];
    let view = derive_one(&scene.server());
    assert_eq!(
        rows(view.workspace().unwrap()),
        [
            (&SectionTitle::TextChannels, vec!["general"]),
            (&SectionTitle::VoiceChannels, vec!["HR"]),
            (
                &SectionTitle::Category("Projects".to_owned()),
                vec!["Daily", "random"]
            ),
        ]
    );
}

#[test]
fn a_voice_channel_shows_its_people_with_their_marks_and_the_state_of_its_call() {
    let mut scene = scene();
    let (ana, bruno, carla, dora) = (
        AccountId::generate(),
        AccountId::generate(),
        AccountId::generate(),
        AccountId::generate(),
    );
    let own = scene.own;
    let (studio, daily, hidden) = (scene.studio, scene.daily, scene.hidden);
    let mut muted = participant(bruno);
    muted.muted = true;
    muted.deafened = true;
    let mut held = participant(carla);
    held.muted = true;
    held.microphone_block = Some(MicrophoneBlock {
        by: ana,
        reason: BlockReason::Moderator,
    });
    held.hand_raised = true;
    let mut sharing = participant(dora);
    sharing.screen_share = Some(ScreenShare {
        audio: ShareAudio::None,
        started_at: time(),
    });
    sharing.sound_block = Some(SoundBlock { by: ana });
    let call = scene.call(
        daily,
        vec![participant(ana), muted, held, sharing, participant(own)],
    );
    let call_id = call.id;
    call.lock = Some(CallLock {
        by: ana,
        at: time(),
    });
    call.meeting = Some(Meeting {
        presenter: ana,
        started_at: time(),
    });
    call.recording = Some(Recording {
        id: RecordingId::generate(),
        workspace_id: studio,
        channel_id: daily,
        call_id,
        state: RecordingState::Starting,
        started_by: ana,
        requested_at: time(),
        recording_since: None,
        stop: None,
        ended_at: None,
        failure: None,
        language: "auto".to_owned(),
        segments: 0,
        failed_segments: 0,
        bytes: None,
        timeline_started_at: None,
        transcript: TranscriptState::None,
        transcript_lines: 0,
    });
    scene.studio_mut().calls.push(ChannelCall {
        workspace_id: studio,
        channel_id: hidden,
        in_use: true,
        call: None,
    });
    for held in &mut scene.studio_mut().channels {
        if held.id == daily {
            held.user_limit = Some(5);
        }
    }

    let mut model = scene.model();
    let workspace = Arc::make_mut(&mut model.workspaces[1]);
    workspace.members = Loadable::Loaded(vec![
        WorkspaceMember {
            avatar: None,
            account_id: ana,
            name: "Ana".to_owned(),
            role_ids: Vec::new(),
        },
        WorkspaceMember {
            avatar: None,
            account_id: bruno,
            name: "Bruno".to_owned(),
            role_ids: Vec::new(),
        },
    ]);
    for held in &mut workspace.channels {
        if let Some(call) = &mut held.call {
            call.speaking = vec![ana];
        }
    }
    let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
    let mut known = Known::default();
    known.people.read_name(server.id, carla, "Carla");
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    let workspace = view.workspace().unwrap();

    let state = voice(workspace, "Daily");
    assert!(state.in_use && state.locked && state.meeting && state.recording && state.full);
    assert!(!state.hidden);
    assert_eq!(state.user_limit, Some(5));
    /// The name, the microphone, and whether the person shares, raised a hand, has the
    /// sound off and is the account itself.
    type Shown<'a> = (Option<&'a str>, Microphone, bool, bool, bool, bool);
    let people: Vec<Shown<'_>> = state
        .occupants
        .iter()
        .map(|occupant| {
            (
                occupant.name.as_deref(),
                occupant.microphone,
                occupant.sharing,
                occupant.hand_raised,
                occupant.sound_off,
                occupant.is_self,
            )
        })
        .collect();
    let open = |speaking| Microphone::Open { speaking };
    assert_eq!(
        people,
        [
            // From the members of the workspace.
            (Some("Ana"), open(true), false, false, false, false),
            (Some("Bruno"), Microphone::Closed, false, false, true, false),
            // From the profile route, because the member list does not hold the account.
            (
                Some("Carla"),
                Microphone::Held(BlockReason::Moderator),
                false,
                true,
                false,
                false
            ),
            // Not read yet.
            (None, open(false), true, false, true, false),
            (Some("Luca"), open(false), false, false, false, true),
        ]
    );

    let state = voice(workspace, "HR");
    assert!(state.in_use && state.hidden && state.occupants.is_empty());
    assert!(!state.locked && !state.recording && !state.full);
}

/// A call keeps the setting of its start (ADR 0517). The server tells nothing of a call
/// in use only when that call hides who is inside, so the room is shown as hidden also
/// after the setting of its channel was turned off.
#[test]
fn a_call_in_use_that_the_server_does_not_show_is_hidden_whatever_its_channel_says_now() {
    let mut scene = scene();
    let (studio, hidden) = (scene.studio, scene.hidden);
    scene.studio_mut().calls.push(ChannelCall {
        workspace_id: studio,
        channel_id: hidden,
        in_use: true,
        call: None,
    });
    for channel in &mut scene.studio_mut().channels {
        if channel.id == hidden {
            channel.hides_occupants = false;
        }
    }
    let server = scene.server();
    let known = Known::default();
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    let state = voice(view.workspace().unwrap(), "HR");
    assert!(state.in_use && state.hidden && state.occupants.is_empty());
}

#[test]
fn a_lock_and_a_recording_show_only_for_a_channel_in_use() {
    let mut scene = scene();
    let (daily, own) = (scene.daily, scene.own);
    let studio = scene.studio;
    let call = scene.call(daily, Vec::new());
    call.lock = Some(CallLock {
        by: own,
        at: time(),
    });
    call.meeting = Some(Meeting {
        presenter: own,
        started_at: time(),
    });
    call.recording = Some(Recording {
        id: RecordingId::generate(),
        workspace_id: studio,
        channel_id: daily,
        call_id: call.id,
        state: RecordingState::Recording,
        started_by: own,
        requested_at: time(),
        recording_since: Some(time()),
        stop: None,
        ended_at: None,
        failure: None,
        language: "auto".to_owned(),
        segments: 1,
        failed_segments: 0,
        bytes: None,
        timeline_started_at: None,
        transcript: TranscriptState::None,
        transcript_lines: 0,
    });
    call.started_at = None;
    scene.studio_mut().calls[0].in_use = false;
    let view = derive_one(&scene.server());
    let state = voice(view.workspace().unwrap(), "Daily");
    assert_eq!(
        state,
        &VoiceState {
            in_use: false,
            ..VoiceState::default()
        }
    );

    // The same call in a channel in use shows all three.
    scene.studio_mut().calls[0].in_use = true;
    let view = derive_one(&scene.server());
    let state = voice(view.workspace().unwrap(), "Daily");
    assert!(state.in_use && state.locked && state.meeting && state.recording);
}

#[test]
fn the_members_are_in_the_order_of_their_names_with_their_roles() {
    let mut scene = scene();
    let workspace_id = scene.studio;
    let role = |name: &str, position| WorkspaceRole {
        position,
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: name.to_owned(),
        permissions: Vec::new(),
        is_everyone: position == 0,
    };
    let (everyone, leads, engineers) =
        (role("everyone", 0), role("Leads", 2), role("Engineers", 1));
    scene.studio_mut().roles = vec![everyone.clone(), leads.clone(), engineers.clone()];
    let member = |account_id, name: &str, role_ids| WorkspaceMember {
        avatar: None,
        account_id,
        name: name.to_owned(),
        role_ids,
    };
    let own = scene.own;
    let mut model = scene.model();
    assert_eq!(
        derive_one(&server(
            "Acme",
            Some(model.clone()),
            logged_in(own, Vec::new())
        ))
        .workspace()
        .unwrap()
        .members,
        Members::NotLoaded
    );
    Arc::make_mut(&mut model.workspaces[1]).members = Loadable::Loading;
    let loading = server("Acme", Some(model.clone()), logged_in(own, Vec::new()));
    assert_eq!(
        derive_one(&loading).workspace().unwrap().members,
        Members::Loading
    );

    Arc::make_mut(&mut model.workspaces[1]).members = Loadable::Loaded(vec![
        member(own, "luca", vec![engineers.id, everyone.id]),
        member(
            AccountId::generate(),
            "Car\nla\u{202e}",
            vec![engineers.id, leads.id],
        ),
        member(AccountId::generate(), "ana", Vec::new()),
    ]);
    let loaded = server("Acme", Some(model.clone()), logged_in(own, Vec::new()));
    let view = derive_one(&loaded);
    let Members::Loaded(rows) = &view.workspace().unwrap().members else {
        panic!("the members are loaded");
    };
    let shown: Vec<(&str, bool, Vec<&str>)> = rows
        .iter()
        .map(|row| {
            (
                row.name.as_str(),
                row.is_self,
                row.roles.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("ana", false, vec![]),
            ("Carla", false, vec!["Leads", "Engineers"]),
            ("luca", true, vec!["Engineers"]),
        ]
    );

    // A read that failed leaves the part "not loaded" in the state.
    Arc::make_mut(&mut model.workspaces[1]).members = Loadable::NotLoaded;
    let failed = server("Acme", Some(model), logged_in(own, Vec::new()));
    let known = Known {
        members_failed: vec![(failed.id, workspace_id)],
        ..Known::default()
    };
    let view = derive(std::slice::from_ref(&failed), &Selection::default(), &known);
    assert_eq!(view.workspace().unwrap().members, Members::Failed);
}

#[test]
fn every_state_of_the_access_and_of_the_connection_has_its_status() {
    let scene = scene();
    let own = scene.own;
    let with = |change: &dyn Fn(&mut ServerSnapshot)| {
        let mut server = scene.server();
        change(&mut server);
        status(&server)
    };
    assert_eq!(with(&|_| {}), ServerStatus::Ready);
    // A reload shows the state that is held.
    assert_eq!(
        with(&|server| server.stream = StreamStatus::Connecting),
        ServerStatus::Ready
    );
    assert_eq!(
        with(&|server| server.stream = StreamStatus::Interrupted(StreamFault::Silent)),
        ServerStatus::Reconnecting
    );
    // A login after an ended session got no answer (ADR 0787): the flow stays logged in,
    // and the server shows its last state as for an interrupted connection.
    assert_eq!(
        with(&|server| {
            server.reach = Reach::Unreachable;
            server.stream = StreamStatus::Interrupted(StreamFault::SessionEnded(None));
        }),
        ServerStatus::Reconnecting
    );
    let no_model = |stream| {
        with(&move |server| {
            server.model = None;
            server.stream = stream;
        })
    };
    assert_eq!(no_model(StreamStatus::Connecting), ServerStatus::Connecting);
    assert_eq!(no_model(StreamStatus::Off), ServerStatus::Standby);
    assert_eq!(
        no_model(StreamStatus::Interrupted(StreamFault::Closed(None))),
        ServerStatus::Unreachable
    );
    assert_eq!(
        no_model(StreamStatus::Interrupted(StreamFault::SessionEnded(Some(
            EndReason::SessionEnded
        )))),
        ServerStatus::Connecting
    );

    let logged_out = |reach, in_progress| {
        let mut server = without_account(AccessState::LoggedOut { account_id: own });
        server.reach = reach;
        server.in_progress = in_progress;
        status(&server)
    };
    use junto_client_core::flow::access::ActionKind;
    assert_eq!(logged_out(Reach::Unknown, None), ServerStatus::Connecting);
    assert_eq!(
        logged_out(Reach::Unreachable, Some(ActionKind::LogIn)),
        ServerStatus::Connecting
    );
    assert_eq!(
        logged_out(Reach::Unreachable, None),
        ServerStatus::Unreachable
    );
    assert_eq!(logged_out(reachable(), None), ServerStatus::LoggedOut);
    let too_old = Incompatibility::ClientTooOld { server_oldest: 3 };
    assert_eq!(
        logged_out(Reach::Incompatible(too_old), None),
        ServerStatus::Incompatible(too_old)
    );
    assert_eq!(
        logged_out(
            Reach::OtherServer {
                reported: ServerId::generate()
            },
            None
        ),
        ServerStatus::OtherServer
    );

    let of = |access| status(&without_account(access));
    assert_eq!(
        of(AccessState::NoAccount {
            notice: Some(Notice::Removed)
        }),
        ServerStatus::NoAccount(Some(Notice::Removed))
    );
    assert_eq!(
        of(AccessState::WaitingForApproval {
            join_request_id: None,
            name: "Luca".to_owned()
        }),
        ServerStatus::WaitingForApproval {
            name: "Luca".to_owned()
        }
    );
    assert_eq!(
        of(AccessState::WaitingForLink {
            code: LinkCode::new("ABCD-EFGH"),
            public_key: junto_client_core::keys::DeviceKey::generate()
                .unwrap()
                .public_key(),
            expires_at: time(),
        }),
        ServerStatus::WaitingForLink {
            code: "ABCD-EFGH".to_owned()
        }
    );
    assert_eq!(
        of(AccessState::Banned { account_id: None }),
        ServerStatus::Banned
    );
}

#[test]
fn a_server_without_lists_shows_its_state_its_failure_and_the_preset_name() {
    let refused = FlowError::OriginRejected;
    let mut server = without_account(AccessState::LoggedOut {
        account_id: AccountId::generate(),
    });
    server.error = Some(refused.clone());
    server.in_progress = None;
    server.keep_connected = false;
    let known = Known {
        preset_name: "Luca".to_owned(),
        ..Known::default()
    };
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    let selected = view.selected.as_ref().unwrap();
    assert_eq!(selected.status, ServerStatus::LoggedOut);
    assert_eq!(selected.content, Content::State);
    assert_eq!(selected.error, Some(refused));
    assert!(!selected.busy && !selected.keep_connected);
    assert!(view.workspace().is_none());
    assert_eq!(
        view.person,
        Person {
            account_id: None,
            name: "Luca".to_owned()
        }
    );

    // Without a server the window shows the preset name and nothing else.
    let empty = derive(&[], &Selection::default(), &known);
    assert!(empty.servers.is_empty() && empty.selected.is_none());
    assert_eq!(empty.person.name, "Luca");
    assert_eq!(empty.shown(), Shown::default());
}

#[test]
fn recoveries_and_a_stale_state_are_shown_with_the_lists() {
    let scene = scene();
    let recovery = RecoveryNotice {
        id: RecoveryId::generate(),
        device_id: None,
        performed_by: None,
        performed_at: time(),
    };
    let mut server = scene.server();
    server.access = logged_in(scene.own, vec![recovery.clone()]);
    server.stream = StreamStatus::Interrupted(StreamFault::Silent);
    let view = derive_one(&server);
    assert!(view.servers[0].has_recovery);
    let selected = view.selected.as_ref().unwrap();
    assert_eq!(selected.recoveries, [recovery]);
    assert_eq!(selected.status, ServerStatus::Reconnecting);
    assert!(view.workspace().is_some());
    assert_eq!(view.servers[0].workspaces.len(), 2);

    // An account that sees no workspace.
    let mut scene = scene;
    scene.snapshot.workspaces.clear();
    let view = derive_one(&scene.server());
    assert_eq!(
        view.selected.as_ref().unwrap().content,
        Content::NoWorkspace
    );
}

#[test]
fn an_address_is_shown_without_its_scheme() {
    assert_eq!(
        shown_address("https://acme.example:7443"),
        "acme.example:7443"
    );
    assert_eq!(shown_address("http://127.0.0.1:8080"), "127.0.0.1:8080");
    assert_eq!(shown_address("acme.example"), "acme.example");
}

#[test]
fn the_state_of_another_account_is_not_shown_as_the_state_of_the_account_of_the_session() {
    let scene = scene();
    // The device was logged in to another account, and the event stream still holds the
    // state of the account before.
    let mut server = scene.server();
    let now = AccountId::generate();
    server.access = AccessState::LoggedIn {
        account_id: now,
        device_id: DeviceId::generate(),
        session_expires_at: time(),
        recoveries: Vec::new(),
        previous_account: Some(scene.own),
    };
    let known = Known {
        preset_name: "Preset".to_owned(),
        ..Known::default()
    };
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    assert_eq!(view.servers[0].status, ServerStatus::Connecting);
    assert!(view.servers[0].workspaces.is_empty());
    let selected = view.selected.as_ref().unwrap();
    assert_eq!(selected.content, Content::State);
    assert!(selected.account_changed);
    assert_eq!(
        view.person,
        Person {
            account_id: None,
            name: "Preset".to_owned()
        }
    );

    // After a ban or a logout the lists are gone, and so is the name of the account.
    for access in [
        AccessState::Banned {
            account_id: Some(scene.own),
        },
        AccessState::LoggedOut {
            account_id: scene.own,
        },
        AccessState::NoAccount {
            notice: Some(Notice::Removed),
        },
    ] {
        let mut server = scene.server();
        server.access = access;
        let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
        assert!(view.workspace().is_none());
        assert!(view.servers[0].workspaces.is_empty());
        assert_eq!(view.person.account_id, None);
        assert_eq!(view.person.name, "Preset");
        assert!(!view.selected.as_ref().unwrap().account_changed);
    }
}

mod calls {
    use std::time::Duration;

    use junto_client_core::protocol::call::LeaveReason;
    use junto_client_core::protocol::error::ErrorCode;
    use junto_client_core::protocol::recording::{
        RecordingFailure, RecordingStop, RecordingStopReason,
    };
    use junto_client_core::voice::VoiceSettings;
    use junto_media::audio::{AudioError, DeviceId, DeviceInfo, Devices};
    use junto_media::calls::{ActiveCall, CallTarget, CallsView, EndReason, Outcome, Phase};
    use junto_media::engine::{MicrophoneState, OutputState, SeenShare, ShareInfo};
    use junto_media::permission::MicrophoneAccess;

    use super::*;

    fn target(server: &ServerSnapshot, scene: &Scene, channel_id: ChannelId) -> CallTarget {
        CallTarget {
            server_id: server.id,
            place: CallPlace::Channel {
                workspace_id: scene.studio,
                channel_id,
            },
        }
    }

    fn active(target: CallTarget) -> ActiveCall {
        ActiveCall {
            camera: junto_media::camera::CameraState::Off,
            cameras: Vec::new(),
            target,
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
        }
    }

    /// ADR 0789: the row of the person in the call of this device says what the device
    /// does. What the server lists for the person is the last report of the device.
    #[test]
    fn the_listed_own_tile_tracks_local_controls_before_server_reports() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        scene.studio_mut().calls.clear();
        // The server lists the person with the microphone closed, the sound off and a
        // share: the report of the device from before the person changed all three.
        let mut listed = participant(own);
        listed.muted = true;
        listed.deafened = true;
        listed.hand_raised = true;
        listed.screen_share = Some(ScreenShare {
            audio: ShareAudio::None,
            started_at: time(),
        });
        scene.call(daily, vec![listed]);
        let server = server(
            "Acme Studio",
            Some(scene.model()),
            logged_in(own, Vec::new()),
        );
        let servers = std::slice::from_ref(&server);
        let own_row = |known: &Known| {
            let view = derive(servers, &selected(&server, &scene, daily), known);
            occupant(&view, "Daily", own)
        };

        // The device sends the microphone, plays the sound and shares nothing.
        let mut known = in_call(active(target(&server, &scene, daily)));
        known.media.levels.own = (3, true);
        let shown = own_row(&known);
        assert_eq!(
            shown.microphone,
            Microphone::Open { speaking: true },
            "the tile of the person says closed while the device sends the microphone"
        );
        assert!(!shown.sound_off, "the tile says that the sound is off");
        assert!(
            !shown.sharing,
            "the tile shows a share that the device ended"
        );
        assert_eq!(shown.level, Some(3));
        // The hand is held by the server.
        assert!(shown.hand_raised);

        // The person closes the microphone and turns the sound off, and the share of the
        // device runs. The list of the server is the same.
        known.media.calls.muted = true;
        known.media.calls.deafened = true;
        known.media.calls.call.as_mut().unwrap().share = running_share();
        let shown = own_row(&known);
        assert_eq!(shown.microphone, Microphone::Closed);
        assert!(shown.sound_off && shown.sharing);

        // An open setting with a microphone that does not run sends nothing.
        known.media.calls.muted = false;
        for state in [
            MicrophoneState::Closed,
            MicrophoneState::Asking,
            MicrophoneState::NoPermission,
            MicrophoneState::Failed(AudioError::NoDevice),
        ] {
            known.media.calls.call.as_mut().unwrap().microphone = state;
            assert_eq!(own_row(&known).microphone, Microphone::Closed);
        }

        // The device is not in the call: the row is the seat of the account on another
        // device, as the server lists it.
        let elsewhere = own_row(&Known::default());
        assert_eq!(elsewhere.microphone, Microphone::Closed);
        assert!(elsewhere.sound_off && elsewhere.sharing);
    }

    #[test]
    fn the_controls_hold_the_action_and_the_seat_that_they_show() {
        let scene = scene();
        let server = server(
            "Acme",
            Some(scene.model()),
            logged_in(scene.own, Vec::new()),
        );
        let servers = std::slice::from_ref(&server);
        let call = active(target(&server, &scene, scene.daily));
        let seat = call.seat();
        let mut known = in_call(call);
        let controls = derive(servers, &Selection::default(), &known).controls;
        assert_eq!(controls.microphone_action, MicrophoneAction::Close);
        assert_eq!(controls.seat, Some(seat.clone()));
        known.media.calls.muted = true;
        let controls = derive(servers, &Selection::default(), &known).controls;
        assert_eq!(
            controls.microphone_action,
            MicrophoneAction::Open(Some(seat))
        );
        // Outside a call the setting is the one of the next call.
        let mut outside = Known::default();
        outside.media.calls.muted = true;
        let controls = derive(servers, &Selection::default(), &outside).controls;
        assert_eq!(controls.microphone_action, MicrophoneAction::Open(None));
        assert_eq!(controls.seat, None);
    }

    #[test]
    fn a_stale_call_snapshot_cannot_authorize_an_inside_action() {
        let scene = scene();
        let server = server(
            "Acme",
            Some(scene.model()),
            logged_in(scene.own, Vec::new()),
        );
        let target = target(&server, &scene, scene.daily);
        let shown = CallsView {
            call: Some(active(target)),
            ..CallsView::default()
        };
        let mut current = shown.clone();
        current.call.as_mut().unwrap().phase = Phase::Entering;
        assert_eq!(crate::state::actionable_call(&shown, &current), None);
        current.call.as_mut().unwrap().identity = "replacement".to_owned();
        assert_eq!(crate::state::actionable_call(&shown, &current), None);
        current.call.as_mut().unwrap().phase = Phase::Connected;
        assert_eq!(crate::state::actionable_call(&shown, &current), None);
        current.call.as_mut().unwrap().identity = "seat".to_owned();
        assert_eq!(
            crate::state::actionable_call(&shown, &current),
            Some(target)
        );
    }

    #[test]
    fn a_rendered_call_action_cannot_target_a_replacement_call() {
        let scene = scene();
        let server = server(
            "Acme",
            Some(scene.model()),
            logged_in(scene.own, Vec::new()),
        );
        let original = active(target(&server, &scene, scene.daily));
        let action = crate::state::CallAction::of(&original);
        let mut current = CallsView {
            call: Some(original),
            ..CallsView::default()
        };
        let requests = std::cell::Cell::new(0);
        let callback = |current: &CallsView| {
            if action.permits(current) {
                requests.set(requests.get() + 1);
            }
        };
        current.call.as_mut().unwrap().identity = "replacement".to_owned();
        callback(&current);
        assert_eq!(requests.get(), 0);
        current.call.as_mut().unwrap().identity = "seat".to_owned();
        current.call.as_mut().unwrap().phase = Phase::Entering;
        callback(&current);
        assert_eq!(requests.get(), 0);
        current.call.as_mut().unwrap().phase = Phase::Connected;
        callback(&current);
        assert_eq!(requests.get(), 1);
    }

    fn in_call(call: ActiveCall) -> Known {
        Known {
            media: MediaSnapshot {
                calls: CallsView {
                    call: Some(call),
                    ..CallsView::default()
                },
                ..MediaSnapshot::default()
            },
            ..Known::default()
        }
    }

    fn selected(server: &ServerSnapshot, scene: &Scene, channel_id: ChannelId) -> Selection {
        Selection {
            channels: [((server.id, scene.studio), channel_id)]
                .into_iter()
                .collect(),
            ..Selection::default()
        }
    }

    fn occupant(view: &ShellView, name: &str, account_id: AccountId) -> Occupant {
        voice(view.workspace().unwrap(), name)
            .occupants
            .iter()
            .find(|occupant| occupant.account_id == account_id)
            .cloned()
            .unwrap()
    }

    #[test]
    fn a_call_waiting_for_admission_offers_no_invitation_or_entry_answer() {
        let mut scene = scene();
        let other = AccountId::generate();
        let call_id = scene.call(scene.daily, vec![]).id;
        let focus = channel(scene.studio, ChannelKind::Voice, "Focus", 5);
        let focus_id = focus.id;
        scene.studio_mut().channels.push(focus);
        scene.call(focus_id, vec![participant(other)]);
        let mut model = scene.model();
        model.entry_requests.push(EntryRequest {
            id: EntryRequestId::generate(),
            call_id,
            place: CallPlace::Channel {
                workspace_id: scene.studio,
                channel_id: scene.daily,
            },
            account_id: other,
            created_at: time(),
            expires_at: time() + time::Duration::seconds(60),
        });
        let server = server("Acme", Some(model), logged_in(scene.own, Vec::new()));
        let mut active = active(target(&server, &scene, scene.daily));
        for phase in [Phase::Connecting, Phase::Entering, Phase::Reconnecting] {
            active.phase = phase;
            let view = derive(
                std::slice::from_ref(&server),
                &selected(&server, &scene, scene.daily),
                &in_call(active.clone()),
            );
            let menu = view.person_menu(focus_id, other).unwrap();
            assert_eq!(menu.invite_to, None);
            assert!(view.entry_prompts.is_empty());
            assert_eq!(view.call.as_ref().unwrap().hand_raised, None);
            assert!(!view.workspace().unwrap().call.as_ref().unwrap().room.ready);
        }
        active.phase = Phase::Connected;
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, scene.daily),
            &in_call(active),
        );
        assert!(
            view.person_menu(focus_id, other)
                .unwrap()
                .invite_to
                .is_some()
        );
        assert_eq!(view.entry_prompts.len(), 1);
        assert!(view.call.as_ref().unwrap().hand_raised.is_some());
        assert!(view.workspace().unwrap().call.as_ref().unwrap().room.ready);
    }

    #[test]
    fn the_call_of_this_device_shows_measured_levels_and_the_panel_of_the_call() {
        let mut scene = scene();
        let (ana, ben) = (AccountId::generate(), AccountId::generate());
        let (own, daily, general) = (scene.own, scene.daily, scene.general);
        scene.call(
            daily,
            vec![participant(ana), participant(ben), participant(own)],
        );
        let mut model = scene.model();
        // The server marks Ben as speaking. In the call of this device the sound decides.
        let place = CallPlace::Channel {
            workspace_id: scene.studio,
            channel_id: daily,
        };
        let mut speaking = model.call_at(&place).unwrap().clone();
        speaking.speaking = vec![ben];
        let workspace = Arc::make_mut(&mut model.workspaces[1]);
        workspace
            .channels
            .iter_mut()
            .find(|channel| channel.channel.id == daily)
            .unwrap()
            .call = Some(speaking);
        let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let servers = std::slice::from_ref(&server);

        // Outside the call: the marks of the server, and no measured level.
        let outside = derive(
            servers,
            &selected(&server, &scene, daily),
            &Known::default(),
        );
        let row = voice(outside.workspace().unwrap(), "Daily");
        assert_eq!(row.relation, Relation::Outside);
        let shown = occupant(&outside, "Daily", ben);
        assert_eq!(shown.microphone, Microphone::Open { speaking: true });
        assert_eq!((shown.level, shown.volume), (None, 100));
        assert!(outside.workspace().unwrap().call.is_none());
        assert_eq!(
            (outside.call.clone(), outside.call_notice.clone()),
            (None, None)
        );
        assert_eq!(outside.controls, Controls::default());

        // In the call: the levels of the media engine.
        let mut known = in_call(active(target(&server, &scene, daily)));
        known.media.levels = ShownLevels {
            input: 7,
            own: (2, true),
            people: [(ana, (4, true))].into_iter().collect(),
        };
        known.media.voice.set_volume(server.id, ana, 140);
        known.media.now = Some(time() + time::Duration::seconds(125));
        let inside = derive(servers, &selected(&server, &scene, daily), &known);
        assert_eq!(
            voice(inside.workspace().unwrap(), "Daily").relation,
            Relation::Inside
        );
        let shown = occupant(&inside, "Daily", ana);
        assert_eq!(shown.microphone, Microphone::Open { speaking: true });
        assert_eq!((shown.level, shown.volume), (Some(4), 140));
        let shown = occupant(&inside, "Daily", ben);
        assert_eq!(shown.microphone, Microphone::Open { speaking: false });
        assert_eq!((shown.level, shown.presence), (Some(0), Presence::Here));
        let shown = occupant(&inside, "Daily", own);
        assert!(shown.is_self);
        assert_eq!(shown.microphone, Microphone::Open { speaking: true });
        assert_eq!(shown.level, Some(2));
        let panel = inside.workspace().unwrap().call.clone().unwrap();
        assert_eq!(
            (panel.phase, panel.audio.input_level),
            (Phase::Connected, 7)
        );
        assert_eq!(panel.audio.microphone, MicrophoneLine::Running);
        let strip = inside.call.clone().unwrap();
        assert_eq!(strip.name.as_deref(), Some("Daily"));
        assert_eq!(strip.elapsed, Some(Duration::from_secs(125)));
        assert_eq!((strip.channel_id, strip.shown), (Some(daily), true));

        // Another channel is shown: the strip stays, and the main area has no call.
        let elsewhere = derive(servers, &selected(&server, &scene, general), &known);
        assert!(elsewhere.workspace().unwrap().call.is_none());
        let strip = elsewhere.call.unwrap();
        assert_eq!((strip.name.as_deref(), strip.shown), (Some("Daily"), false));
        // A clock of this machine that is behind the server shows no negative time.
        known.media.now = Some(time() - time::Duration::seconds(5));
        let early = derive(servers, &selected(&server, &scene, daily), &known);
        assert_eq!(early.call.unwrap().elapsed, Some(Duration::ZERO));
    }

    #[test]
    fn levels_put_into_a_derived_view_give_the_view_that_a_derivation_with_them_gives() {
        let mut scene = scene();
        let (ana, ben, cyd) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let (own, daily, general) = (scene.own, scene.daily, scene.general);
        let mut held = participant(cyd);
        held.microphone_block = Some(MicrophoneBlock {
            reason: BlockReason::Moderator,
            by: ana,
        });
        let mut closed = participant(ben);
        closed.muted = true;
        scene.call(
            daily,
            vec![participant(ana), closed, held, participant(own)],
        );
        let model = scene.model();
        let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let servers = std::slice::from_ref(&server);
        let levels = [
            ShownLevels::default(),
            ShownLevels {
                input: 7,
                own: (2, true),
                people: [(ana, (4, true)), (ben, (1, false)), (cyd, (3, true))]
                    .into_iter()
                    .collect(),
            },
            ShownLevels {
                input: 20,
                own: (5, false),
                people: [(ana, (0, false))].into_iter().collect(),
            },
        ];
        let calls = [
            Known::default(),
            in_call(active(target(&server, &scene, daily))),
        ];
        for (known, channel) in calls
            .iter()
            .flat_map(|known| [daily, general].map(|channel| (known, channel)))
        {
            let selection = selected(&server, &scene, channel);
            for (before, after) in levels
                .iter()
                .flat_map(|before| levels.iter().map(move |after| (before, after)))
            {
                let with = |levels: &ShownLevels| {
                    let mut known = known.clone();
                    known.media.levels = levels.clone();
                    derive(servers, &selection, &known)
                };
                let mut shown = with(before);
                shown.show_levels(after);
                assert_eq!(shown, with(after));
            }
        }
    }

    #[test]
    fn the_chat_of_the_call_of_this_device_has_no_thread_and_its_threads_cannot_be_opened() {
        use junto_client_core::model::Named;
        use junto_client_core::protocol::message::{MessagePlace, ThreadState};

        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        scene.call(daily, vec![participant(own)]);
        let mut model = scene.model();
        let place = MessagePlace::Channel {
            workspace_id: scene.studio,
            channel_id: daily,
        };
        let root = super::super::panel_tests::message(place, 2, own, "Who takes notes?");
        model.threads = vec![ThreadState {
            root_id: root.id,
            last_read: 2,
            latest: 4,
            unread: 2,
            mentions: 0,
            following: true,
            follow_setting: None,
        }];
        model.named = vec![(root.id, Named::Read(Arc::new(root.clone())))];
        let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let servers = std::slice::from_ref(&server);
        let mut selection = selected(&server, &scene, daily);
        selection.thread = Some((server.id, daily, root.id));
        let entry = |view: &ShellView| {
            let workspace = view.workspace().unwrap();
            let row = workspace
                .sections
                .iter()
                .flat_map(|section| section.rows.iter())
                .find(|row| row.id == daily)
                .unwrap();
            assert_eq!(row.threads.len(), 1);
            row.threads[0].clone()
        };

        // Outside the call the thread of the voice channel is shown and can be opened.
        let outside = derive(servers, &selection, &Known::default());
        assert!(outside.workspace().unwrap().thread.is_some());
        assert!(entry(&outside).available);

        // In the call the side panel is the panel of the call: no thread and no pin list
        // is shown, and the row of the thread says that it cannot be opened.
        let known = in_call(active(target(&server, &scene, daily)));
        let inside = derive(servers, &selection, &known);
        let workspace = inside.workspace().unwrap();
        assert!(workspace.call.is_some());
        assert!(workspace.thread.is_none() && workspace.pins.is_none());
        assert_eq!(inside.shown().thread, None);
        assert!(!entry(&inside).available);
        selection.thread = None;
        selection.pins = Some((server.id, daily));
        let inside = derive(servers, &selection, &known);
        assert!(inside.workspace().unwrap().pins.is_none());

        // Another channel is shown: a selection of the voice channel shows the call, so
        // its thread still cannot be opened.
        let general = scene.general;
        let elsewhere = derive(servers, &selected(&server, &scene, general), &known);
        assert!(!entry(&elsewhere).available);
    }

    #[test]
    fn the_person_is_listed_in_the_call_from_the_moment_the_device_has_its_place() {
        let mut scene = scene();
        let (ana, own, daily) = (AccountId::generate(), scene.own, scene.daily);
        scene.call(daily, vec![participant(ana)]);
        let server = scene.server();
        let servers = std::slice::from_ref(&server);
        let mut call = active(target(&server, &scene, daily));
        call.phase = Phase::Connecting;
        let view = derive(
            servers,
            &selected(&server, &scene, daily),
            &in_call(call.clone()),
        );
        let people = &voice(view.workspace().unwrap(), "Daily").occupants;
        assert_eq!(people.len(), 2);
        let shown = occupant(&view, "Daily", own);
        assert!(shown.is_self);
        assert_eq!(shown.name.as_deref(), Some("Luca"));
        assert_eq!(shown.presence, Presence::Connecting);
        assert_eq!(occupant(&view, "Daily", ana).presence, Presence::Here);
        assert_eq!(
            view.workspace().unwrap().call.as_ref().unwrap().phase,
            Phase::Connecting
        );

        // The first person of a call: the server holds no details of the call yet.
        let empty = self::scene();
        let server = empty.server();
        let call = active(target(&server, &empty, empty.daily));
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &empty, empty.daily),
            &in_call(call),
        );
        let people = &voice(view.workspace().unwrap(), "Daily").occupants;
        assert_eq!(people.len(), 1);
        assert!(people[0].is_self);
        // The media connection is made, and the server has not listed the person yet:
        // the person is not inside the call for the server (ADR 0517).
        assert_eq!(people[0].presence, Presence::Connecting);
        let mut listed = self::scene();
        let person = listed.own;
        listed.call(listed.daily, vec![participant(person)]);
        let server = listed.server();
        let call = active(target(&server, &listed, listed.daily));
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &listed, listed.daily),
            &in_call(call),
        );
        assert_eq!(occupant(&view, "Daily", person).presence, Presence::Here);

        // While the call is restored, the person is marked and the others are not.
        scene.call(scene.hidden, Vec::new());
        let mut call = active(target(&scene.server(), &scene, daily));
        call.phase = Phase::Reconnecting;
        call.interrupted_at = Some(time());
        let server = ServerSnapshot {
            id: call.target.server_id,
            ..scene.server()
        };
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, daily),
            &in_call(call),
        );
        assert_eq!(
            occupant(&view, "Daily", own).presence,
            Presence::Reconnecting
        );
        let panel = view.workspace().unwrap().call.clone().unwrap();
        assert_eq!(
            (panel.phase, panel.interrupted_at),
            (Phase::Reconnecting, Some(time()))
        );
        assert_eq!(view.call.unwrap().phase, Phase::Reconnecting);
    }

    #[test]
    fn the_controls_show_the_own_settings_a_block_and_a_microphone_that_cannot_be_used() {
        let mut scene = scene();
        let (ana, own, daily) = (AccountId::generate(), scene.own, scene.daily);
        scene.studio_mut().calls.clear();
        let mut held = participant(own);
        let block = MicrophoneBlock {
            by: ana,
            reason: BlockReason::Moderator,
        };
        held.microphone_block = Some(block.clone());
        scene.call(daily, vec![participant(ana), held]);
        let mut model = scene.model();
        Arc::make_mut(&mut model.workspaces[1]).members = Loadable::Loaded(vec![WorkspaceMember {
            avatar: None,
            account_id: ana,
            name: "Ana".to_owned(),
            role_ids: Vec::new(),
        }]);
        let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let servers = std::slice::from_ref(&server);
        let at = |change: &dyn Fn(&mut Known)| {
            let mut known = in_call(active(target(&server, &scene, daily)));
            change(&mut known);
            derive(servers, &selected(&server, &scene, daily), &known).controls
        };
        fn call(known: &mut Known) -> &mut ActiveCall {
            known.media.calls.call.as_mut().unwrap()
        }

        // The person's own settings hold in a call and outside one.
        let mut outside = Known::default();
        outside.media.calls.muted = true;
        outside.media.calls.deafened = true;
        let controls = derive(servers, &Selection::default(), &outside).controls;
        assert_eq!(controls.microphone, MicrophoneButton::Muted);
        assert!(controls.sound_off && controls.warning.is_none());
        assert_eq!(at(&|_| {}).microphone, MicrophoneButton::Open);

        // A block of the server is shown apart from the own setting, with who set it.
        let blocked = at(&|known| {
            known.media.calls.muted = true;
            call(known).microphone_block = Some(block.clone());
        });
        assert_eq!(blocked.microphone, MicrophoneButton::Held);
        assert_eq!(
            blocked.warning,
            Some(SoundWarning::MicrophoneHeld {
                reason: BlockReason::Moderator,
                by: Some("Ana".to_owned()),
            })
        );
        let view = derive(
            servers,
            &selected(&server, &scene, daily),
            &in_call(active(target(&server, &scene, daily))),
        );
        let shown = occupant(&view, "Daily", own);
        assert_eq!(shown.microphone, Microphone::Held(BlockReason::Moderator));
        assert_eq!(shown.held_by.as_deref(), Some("Ana"));

        for (change, expected) in [
            (
                MicrophoneState::NoPermission,
                SoundWarning::NoMicrophoneAccess,
            ),
            (
                MicrophoneState::Failed(AudioError::NoDevice),
                SoundWarning::MicrophoneFailed,
            ),
        ] {
            let controls = at(&|known| call(known).microphone = change.clone());
            assert_eq!(controls.warning, Some(expected));
            assert_eq!(controls.microphone, MicrophoneButton::Open);
        }
        let sound = at(&|known| {
            call(known).sound_block = Some(SoundBlock { by: ana });
            call(known).output = OutputState::Failed(AudioError::NoDevice);
        });
        assert_eq!(
            sound.warning,
            Some(SoundWarning::SoundHeld {
                by: Some("Ana".to_owned())
            })
        );
        let output = at(&|known| call(known).output = OutputState::Failed(AudioError::NoDevice));
        assert_eq!(output.warning, Some(SoundWarning::OutputFailed));
    }

    #[test]
    fn a_join_that_is_asked_for_and_how_a_join_or_a_call_ended_are_shown() {
        let scene = scene();
        let server = scene.server();
        let servers = std::slice::from_ref(&server);
        let daily = target(&server, &scene, scene.daily);
        let with = |change: &dyn Fn(&mut CallsView)| {
            let mut known = Known::default();
            change(&mut known.media.calls);
            derive(servers, &selected(&server, &scene, scene.daily), &known)
        };

        let joining = with(&|calls| calls.joining = Some(daily));
        assert_eq!(
            voice(joining.workspace().unwrap(), "Daily").relation,
            Relation::Joining
        );
        assert_eq!(
            voice(joining.workspace().unwrap(), "HR").relation,
            Relation::Outside
        );
        assert!(joining.call.is_none() && joining.workspace().unwrap().call.is_none());

        let locked = FlowError::refused(ErrorCode::CallLocked);
        let refused = with(&|calls| {
            calls.outcome = Some(Outcome::JoinFailed {
                target: daily,
                error: locked.clone(),
            });
        });
        assert_eq!(
            refused.call_notice,
            Some(CallNotice::JoinFailed {
                channel: Some("Daily".to_owned()),
                error: locked.clone(),
            })
        );
        let ended = |reason: EndReason| {
            with(&|calls| {
                calls.outcome = Some(Outcome::Ended {
                    target: daily,
                    reason: reason.clone(),
                });
            })
            .call_notice
        };
        let removed = EndReason::Server(Some(LeaveReason::Disconnected));
        assert_eq!(
            ended(removed.clone()),
            Some(CallNotice::Ended {
                channel: Some("Daily".to_owned()),
                reason: removed,
            })
        );
        assert!(ended(EndReason::Elsewhere).is_some());
        // An end that the person caused is told by nothing.
        assert_eq!(ended(EndReason::Server(Some(LeaveReason::Left))), None);
        assert_eq!(ended(EndReason::Server(Some(LeaveReason::Switched))), None);

        // A call on a server whose state is not shown has no channel name.
        let mut unknown = daily;
        unknown.server_id = ServerId::generate();
        let notice = with(&|calls| {
            calls.outcome = Some(Outcome::Ended {
                target: unknown,
                reason: EndReason::Elsewhere,
            });
        })
        .call_notice;
        assert_eq!(
            notice,
            Some(CallNotice::Ended {
                channel: None,
                reason: EndReason::Elsewhere,
            })
        );
    }

    #[test]
    fn the_sound_settings_list_the_default_each_device_and_a_chosen_device_that_is_gone() {
        let mut scene = scene();
        let daily = scene.daily;
        scene.call(daily, Vec::new());
        let server = scene.server();
        let device = |id: &str, name: &str, is_default| DeviceInfo {
            id: DeviceId(id.to_owned()),
            name: name.to_owned(),
            is_default,
        };
        let mut call = active(target(&server, &scene, daily));
        call.microphone = MicrophoneState::NoPermission;
        call.access = MicrophoneAccess::Refused;
        call.output = OutputState::Failed(AudioError::NoDevice);
        let mut known = in_call(call);
        known.media.devices = Devices {
            inputs: vec![
                device("built-in", "Built-in microphone", true),
                device("desk", "Desk microphone", false),
            ],
            outputs: vec![device("speakers", "Speakers", true)],
        };
        known.media.voice = VoiceSettings {
            input_device: Some("desk".to_owned()),
            output_device: Some("headset".to_owned()),
            ..VoiceSettings::default()
        };
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, daily),
            &known,
        );
        let audio = view.workspace().unwrap().call.clone().unwrap().audio;
        let listed = |choices: &[DeviceChoice]| -> Vec<(Option<String>, String, bool)> {
            choices
                .iter()
                .map(|choice| (choice.id.clone(), choice.name.clone(), choice.selected))
                .collect()
        };
        assert_eq!(
            listed(&audio.inputs),
            [
                (None, "Built-in microphone".to_owned(), false),
                (
                    Some("built-in".to_owned()),
                    "Built-in microphone".to_owned(),
                    false
                ),
                (Some("desk".to_owned()), "Desk microphone".to_owned(), true),
            ]
        );
        // The chosen output is not connected: it is listed without a name, as the choice.
        assert_eq!(
            listed(&audio.outputs),
            [
                (None, "Speakers".to_owned(), false),
                (Some("speakers".to_owned()), "Speakers".to_owned(), false),
                (Some("headset".to_owned()), String::new(), true),
            ]
        );
        assert_eq!(audio.microphone, MicrophoneLine::NoPermission);
        assert_eq!(audio.access, MicrophoneAccess::Refused);
        assert!(audio.output_failed);

        // Without a choice the default of the system is the choice.
        known.media.voice = VoiceSettings::default();
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, daily),
            &known,
        );
        let audio = view.workspace().unwrap().call.clone().unwrap().audio;
        assert!(audio.inputs[0].selected && audio.outputs[0].selected);
        assert_eq!(
            audio.inputs.iter().filter(|choice| choice.selected).count(),
            1
        );
    }

    #[test]
    fn the_members_outside_the_call_are_listed_with_the_voice_channel_that_they_are_in() {
        let mut scene = scene();
        let (ana, ben, cal) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let (own, daily) = (scene.own, scene.daily);
        let studio = scene.studio;
        let focus = channel(studio, ChannelKind::Voice, "Focus room", 5);
        let focus_id = focus.id;
        scene.studio_mut().channels.push(focus);
        scene.call(daily, vec![participant(ana), participant(own)]);
        scene.call(focus_id, vec![participant(ben)]);
        let mut model = scene.model();
        let member = |account_id, name: &str| WorkspaceMember {
            avatar: None,
            account_id,
            name: name.to_owned(),
            role_ids: Vec::new(),
        };
        Arc::make_mut(&mut model.workspaces[1]).members = Loadable::Loaded(vec![
            member(own, "Luca"),
            member(cal, "Cal"),
            member(ben, "Ben"),
            member(ana, "Ana"),
        ]);
        let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let known = in_call(active(target(&server, &scene, daily)));
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, daily),
            &known,
        );
        let outside = view.workspace().unwrap().call.clone().unwrap().outside;
        let shown: Vec<(&str, Option<&str>)> = outside
            .iter()
            .map(|member| (member.name.as_str(), member.place.as_deref()))
            .collect();
        assert_eq!(shown, [("Ben", Some("Focus room")), ("Cal", None)]);
    }

    fn sharing(account_id: AccountId, audio: ShareAudio) -> CallParticipant {
        let mut participant = participant(account_id);
        participant.screen_share = Some(ScreenShare {
            audio,
            started_at: time(),
        });
        participant
    }

    fn seen(account_id: AccountId, kind: ShareKind) -> SeenShare {
        SeenShare {
            account_id,
            kind: Some(kind),
            frame_rate: Some(30),
            size: Some((1920, 1200)),
        }
    }

    fn panel(view: &ShellView) -> CallPanel {
        view.workspace().unwrap().call.clone().unwrap()
    }

    #[test]
    fn the_shares_of_a_call_are_listed_with_their_sound_and_the_watched_one_is_marked() {
        let mut scene = scene();
        let (ana, ben) = (AccountId::generate(), AccountId::generate());
        let (own, daily, general) = (scene.own, scene.daily, scene.general);
        scene.call(
            daily,
            vec![
                sharing(ana, ShareAudio::System),
                // The server has not told the share of Ben yet.
                participant(ben),
                participant(own),
            ],
        );
        let server = scene.server();
        let servers = std::slice::from_ref(&server);

        // Nobody shares: the call has no stage of shares, and the button offers a share.
        let mut call = active(target(&server, &scene, daily));
        let mut known = in_call(call.clone());
        known.media.can_share = true;
        let view = derive(servers, &selected(&server, &scene, daily), &known);
        assert_eq!(panel(&view).stage, None);
        let strip = view.call.clone().unwrap();
        assert_eq!((strip.share, strip.can_share), (OwnShare::Off, true));
        assert_eq!(view.share_notice, None);

        // Two people share. The engine tells the tracks; the server tells the sound.
        call.shares = vec![seen(ana, ShareKind::Screen), seen(ben, ShareKind::Window)];
        call.watched = Some(ShareId::Of(ana));
        let mut known = in_call(call.clone());
        known.media.voice.share_volume = 60;
        let view = derive(servers, &selected(&server, &scene, daily), &known);
        let stage = panel(&view).stage.unwrap();
        assert_eq!(stage.volume, 60);
        assert_eq!(stage.shares.len(), 2);
        let first = &stage.shares[0];
        assert_eq!((first.id, first.account_id), (ShareId::Of(ana), ana));
        assert_eq!(
            (first.kind, first.sound, first.watched),
            (Some(ShareKind::Screen), Some(ShareAudio::System), true)
        );
        assert_eq!(
            (first.size, first.frame_rate),
            (Some((1920, 1200)), Some(30))
        );
        let second = &stage.shares[1];
        assert_eq!(
            (second.kind, second.sound, second.watched),
            (Some(ShareKind::Window), None, false)
        );
        assert_eq!(
            stage.watched().map(|share| share.id),
            Some(ShareId::Of(ana))
        );
        assert!(occupant(&view, "Daily", ana).sharing);

        // The share of this device is the last of the list, with what the engine sends.
        call.share = ShareState::Running(ShareInfo {
            kind: ShareKind::Window,
            sound: ShareAudio::Application,
            sound_held: true,
            size: (1280, 800),
            frame_rate: 15,
        });
        call.watched = Some(ShareId::Own);
        let view = derive(
            servers,
            &selected(&server, &scene, daily),
            &in_call(call.clone()),
        );
        let stage = panel(&view).stage.unwrap();
        let own_share = stage.shares.last().unwrap();
        assert_eq!(stage.shares.len(), 3);
        assert_eq!((own_share.id, own_share.account_id), (ShareId::Own, own));
        assert_eq!(own_share.name.as_deref(), Some("Luca"));
        assert_eq!(
            (own_share.sound, own_share.sound_held, own_share.watched),
            (Some(ShareAudio::Application), true, true)
        );
        assert_eq!(
            (own_share.size, own_share.frame_rate),
            (Some((1280, 800)), Some(15))
        );
        assert!(!stage.shares[0].watched);
        assert_eq!(view.call.clone().unwrap().share, OwnShare::Running);
        assert!(occupant(&view, "Daily", own).sharing);
        // The strip tells the share while another channel is shown.
        let elsewhere = derive(
            servers,
            &selected(&server, &scene, general),
            &in_call(call.clone()),
        );
        assert_eq!(elsewhere.call.unwrap().share, OwnShare::Running);

        // A share that starts, and one that ended by itself.
        call.shares.clear();
        call.share = ShareState::Starting;
        let view = derive(
            servers,
            &selected(&server, &scene, daily),
            &in_call(call.clone()),
        );
        assert_eq!(view.call.clone().unwrap().share, OwnShare::Starting);
        assert_eq!(panel(&view).stage, None);
        call.share = ShareState::Ended(CaptureError::SourceGone);
        let view = derive(servers, &selected(&server, &scene, general), &in_call(call));
        assert_eq!(view.call.clone().unwrap().share, OwnShare::Off);
        assert_eq!(view.share_notice, Some(CaptureError::SourceGone));
    }

    #[test]
    fn the_settings_of_a_share_and_of_the_view_are_in_the_panel_of_the_call() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        scene.call(daily, vec![participant(own)]);
        let server = scene.server();
        let servers = std::slice::from_ref(&server);
        let mut known = in_call(active(target(&server, &scene, daily)));
        let shown = panel(&derive(servers, &selected(&server, &scene, daily), &known));
        assert_eq!(
            (
                shown.share_resolution,
                shown.share_frame_rate,
                shown.hide_without_video
            ),
            (ShareResolution::P1080, 30, false)
        );
        known.media.voice.share_resolution = ShareResolution::Source;
        known.media.voice.share_frame_rate = 60;
        known.media.voice.hide_people_without_video = true;
        let shown = panel(&derive(servers, &selected(&server, &scene, daily), &known));
        assert_eq!(
            (
                shown.share_resolution,
                shown.share_frame_rate,
                shown.hide_without_video
            ),
            (ShareResolution::Source, 60, true)
        );
    }

    fn recording(scene: &Scene, call_id: CallId) -> Recording {
        Recording {
            id: RecordingId::generate(),
            workspace_id: scene.studio,
            channel_id: scene.daily,
            call_id,
            state: RecordingState::Recording,
            started_by: scene.own,
            requested_at: time(),
            recording_since: Some(time() + time::Duration::seconds(10)),
            stop: None,
            ended_at: None,
            failure: None,
            language: "auto".to_owned(),
            segments: 4,
            failed_segments: 0,
            bytes: None,
            timeline_started_at: None,
            transcript: TranscriptState::None,
            transcript_lines: 0,
        }
    }

    fn stopped(reason: RecordingStopReason) -> Option<RecordingStop> {
        Some(RecordingStop {
            reason,
            by: None,
            at: time() + time::Duration::minutes(30),
        })
    }

    #[test]
    fn the_recording_of_the_call_is_shown_with_its_time_and_what_the_person_may_do() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        let call_id = scene.call(daily, vec![participant(own)]).id;
        let everyone = WorkspaceRole {
            position: 0,
            id: WorkspaceRoleId::generate(),
            workspace_id: scene.studio,
            name: "everyone".to_owned(),
            permissions: Vec::new(),
            is_everyone: true,
        };
        scene.studio_mut().roles = vec![everyone];
        let shown = |scene: &Scene, known: &Known| {
            let server = scene.server();
            let mut known = known.clone();
            let mut call = known.media.calls.call.clone().unwrap();
            call.target = target(&server, scene, daily);
            known.media.calls.call = Some(call);
            if let Some((server_id, _)) = &mut known.recording_problem {
                *server_id = server.id;
            }
            panel(&derive(
                std::slice::from_ref(&server),
                &selected(&server, scene, daily),
                &known,
            ))
            .recording
        };
        let first = scene.server();
        let mut known = in_call(active(target(&first, &scene, daily)));
        known.media.now = Some(time() + time::Duration::seconds(75));

        // No recording, and the roles of the person do not allow one.
        let control = shown(&scene, &known);
        assert_eq!(
            control,
            RecordingControl {
                state: RecordingShown::None,
                may_record: false,
                busy: false,
            }
        );
        // The role "everyone" allows it. The owner of the server may record without a role.
        scene.studio_mut().roles[0].permissions = vec![WorkspacePermission::RecordCalls];
        assert!(shown(&scene, &known).may_record);
        scene.studio_mut().roles[0].permissions.clear();
        scene.snapshot.account.is_owner = true;
        assert!(shown(&scene, &known).may_record);
        known.recording_busy = true;
        assert!(shown(&scene, &known).busy);
        known.recording_busy = false;

        // The server asks the recorder to start.
        let mut running = recording(&scene, call_id);
        running.state = RecordingState::Starting;
        running.recording_since = None;
        scene.studio_mut().calls[0].call.as_mut().unwrap().recording = Some(running.clone());
        assert_eq!(shown(&scene, &known).state, RecordingShown::Starting);

        // The recorder runs: the time since it started.
        running.state = RecordingState::Recording;
        running.recording_since = Some(time() + time::Duration::seconds(10));
        scene.studio_mut().calls[0].call.as_mut().unwrap().recording = Some(running.clone());
        assert_eq!(
            shown(&scene, &known).state,
            RecordingShown::Running {
                elapsed: Some(Duration::from_secs(65)),
                stopping: false,
            }
        );
        // Its end was asked for, and the recorder has not ended yet.
        running.stop = stopped(RecordingStopReason::Stopped);
        scene.studio_mut().calls[0].call.as_mut().unwrap().recording = Some(running.clone());
        assert!(matches!(
            shown(&scene, &known).state,
            RecordingShown::Running { stopping: true, .. }
        ));

        // A recording that runs is shown, whatever was told about one before.
        known.recording_problem = Some((first.id, Box::new(running.clone())));
        assert!(matches!(
            shown(&scene, &known).state,
            RecordingShown::Running { .. }
        ));
    }

    #[test]
    fn a_recording_that_failed_or_that_the_server_stopped_is_told_for_its_call_only() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        let call_id = scene.call(daily, vec![participant(own)]).id;
        let server = scene.server();
        let servers = std::slice::from_ref(&server);
        let known_with = |recording: Recording, of: ServerId| {
            let mut known = in_call(active(target(&server, &scene, daily)));
            known.recording_problem = Some((of, Box::new(recording)));
            known
        };
        let shown = |known: &Known| {
            panel(&derive(servers, &selected(&server, &scene, daily), known))
                .recording
                .state
        };
        let fault_of = |change: &dyn Fn(&mut Recording)| {
            let mut ended = recording(&scene, call_id);
            ended.state = RecordingState::Processing;
            change(&mut ended);
            match shown(&known_with(ended, server.id)) {
                RecordingShown::Failed(problem) => Some(problem.fault),
                RecordingShown::None => None,
                other => panic!("{other:?}"),
            }
        };

        // The recorder was lost: what was saved has a start and an end.
        let mut lost = recording(&scene, call_id);
        lost.state = RecordingState::Processing;
        lost.stop = stopped(RecordingStopReason::RecorderLost);
        assert_eq!(
            shown(&known_with(lost.clone(), server.id)),
            RecordingShown::Failed(RecordingProblem {
                fault: RecordingFault::RecorderLost,
                since: Some(time() + time::Duration::seconds(10)),
                until: Some(time() + time::Duration::minutes(30)),
                segments: 4,
                failed_segments: 0,
            })
        );
        assert!(is_recording_problem(&lost));
        // The recording of another call, or of another server, tells nothing here.
        let mut other = lost.clone();
        other.call_id = CallId::generate();
        assert_eq!(shown(&known_with(other, server.id)), RecordingShown::None);
        assert_eq!(
            shown(&known_with(lost, ServerId::generate())),
            RecordingShown::None
        );

        // Each way in which a recording ends without the person asking for it.
        assert_eq!(
            fault_of(&|ended| ended.stop = stopped(RecordingStopReason::StorageReserve)),
            Some(RecordingFault::StorageReserve)
        );
        assert_eq!(
            fault_of(&|ended| ended.stop = stopped(RecordingStopReason::SegmentLimit)),
            Some(RecordingFault::SegmentLimit)
        );
        assert_eq!(
            fault_of(&|ended| ended.stop = stopped(RecordingStopReason::Unknown)),
            Some(RecordingFault::Other)
        );
        assert_eq!(
            fault_of(&|ended| {
                ended.state = RecordingState::Failed;
                ended.failure = Some(RecordingFailure::RecorderUnavailable);
            }),
            Some(RecordingFault::RecorderUnavailable)
        );
        assert_eq!(
            fault_of(&|ended| {
                ended.state = RecordingState::Failed;
                ended.failure = Some(RecordingFailure::FilesMissing);
                ended.stop = stopped(RecordingStopReason::Stopped);
            }),
            Some(RecordingFault::NothingSaved)
        );
        assert_eq!(
            fault_of(&|ended| {
                ended.state = RecordingState::PartlyFailed;
                ended.stop = stopped(RecordingStopReason::Stopped);
                ended.failed_segments = 1;
            }),
            Some(RecordingFault::PartlySaved)
        );
        // A recording that a person stopped, or that ended with its call, is no problem.
        for reason in [RecordingStopReason::Stopped, RecordingStopReason::CallEnded] {
            assert_eq!(fault_of(&|ended| ended.stop = stopped(reason)), None);
            let mut ready = recording(&scene, call_id);
            ready.state = RecordingState::Ready;
            ready.stop = stopped(reason);
            assert!(!is_recording_problem(&ready));
        }
    }

    use junto_client_core::protocol::WorkspacePermission;
    use junto_client_core::protocol::call::{CallInvitation, EntryRequest};
    use junto_client_core::protocol::ids::{CallInvitationId, EntryRequestId};

    /// Gives the "everyone" role of "Studio" these permissions.
    fn everyone_may(scene: &mut Scene, permissions: &[WorkspacePermission]) {
        let workspace_id = scene.studio;
        scene.studio_mut().roles = vec![WorkspaceRole {
            position: 0,
            id: WorkspaceRoleId::generate(),
            workspace_id,
            name: "everyone".to_owned(),
            permissions: permissions.to_vec(),
            is_everyone: true,
        }];
    }

    fn meeting_block(by: AccountId) -> Option<MicrophoneBlock> {
        Some(MicrophoneBlock {
            by,
            reason: BlockReason::Meeting,
        })
    }

    fn running_share() -> ShareState {
        ShareState::Running(ShareInfo {
            kind: ShareKind::Screen,
            sound: ShareAudio::None,
            sound_held: false,
            size: (1280, 720),
            frame_rate: 30,
        })
    }

    #[test]
    fn meeting_mode_can_be_turned_on_by_a_person_who_shares_has_the_permission_and_no_block() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        scene.call(daily, vec![participant(own)]);
        let start = |scene: &Scene, change: &dyn Fn(&mut ActiveCall)| {
            let server = scene.server();
            let mut call = active(target(&server, scene, daily));
            change(&mut call);
            let view = derive(
                std::slice::from_ref(&server),
                &selected(&server, scene, daily),
                &in_call(call),
            );
            let room = panel(&view).room;
            assert_eq!(room.meeting, None);
            room.meeting_start
        };
        // Without a role that allows it.
        assert_eq!(start(&scene, &|_| {}), MeetingStart::NoPermission);
        everyone_may(&mut scene, &[WorkspacePermission::RunMeetingMode]);
        assert_eq!(start(&scene, &|_| {}), MeetingStart::NotSharing);
        assert_eq!(
            start(&scene, &|call| call.share = ShareState::Starting),
            MeetingStart::NotSharing
        );
        assert_eq!(
            start(&scene, &|call| call.share = running_share()),
            MeetingStart::Possible
        );
        // A person whose microphone is blocked cannot start it (ADR 0512).
        assert_eq!(
            start(&scene, &|call| {
                call.share = running_share();
                call.microphone_block = Some(MicrophoneBlock {
                    by: AccountId::generate(),
                    reason: BlockReason::Moderator,
                });
            }),
            MeetingStart::MicrophoneBlocked
        );
    }

    #[test]
    fn the_presenter_sees_the_raised_hands_and_the_people_who_were_allowed_to_speak() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        let (ana, ben, cleo) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        scene.snapshot.account.name = "Luca".to_owned();
        // Ana raised a hand. Ben was allowed and has not opened the microphone. Cleo was
        // allowed and speaks. A raised hand of a person without a block is no request.
        let mut with_hand = participant(ana);
        with_hand.microphone_block = meeting_block(own);
        with_hand.hand_raised = true;
        let mut allowed = participant(ben);
        allowed.muted = true;
        allowed.hand_raised = true;
        let call = scene.call(
            daily,
            vec![with_hand, allowed, participant(cleo), participant(own)],
        );
        call.meeting = Some(Meeting {
            presenter: own,
            started_at: time(),
        });
        let server = scene.server();
        let mut own_call = active(target(&server, &scene, daily));
        own_call.share = running_share();
        let mut known = in_call(own_call);
        known.people.read_name(server.id, ana, "Ana");
        known.people.read_name(server.id, ben, "Ben");
        // The presenter's own microphone setting does not make a prompt.
        known.media.calls.muted = true;
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, daily),
            &known,
        );
        let room = panel(&view).room;
        assert_eq!(
            room.meeting,
            Some(MeetingView {
                by_self: true,
                presenter: Some("Luca".to_owned()),
                hands: vec![(ana, Some("Ana".to_owned()))],
                waiting: vec![Some("Ben".to_owned())],
                may_speak: false,
            })
        );
        assert_eq!((room.name.as_str(), room.inside), ("Daily", 4));
        let strip = view.call.unwrap();
        assert_eq!((strip.hand_raised, strip.may_speak), (Some(false), false));
    }

    #[test]
    fn a_participant_of_meeting_mode_is_told_who_runs_it_and_when_the_person_may_speak() {
        let mut scene = scene();
        let (own, daily, general) = (scene.own, scene.daily, scene.general);
        let ana = AccountId::generate();
        let mut held = participant(own);
        held.microphone_block = meeting_block(ana);
        held.hand_raised = true;
        let call = scene.call(daily, vec![participant(ana), held]);
        call.meeting = Some(Meeting {
            presenter: ana,
            started_at: time(),
        });
        let shown = |scene: &Scene, held: bool, muted: bool, channel: ChannelId| {
            let server = scene.server();
            let mut call = active(target(&server, scene, daily));
            call.microphone_block = held.then(|| meeting_block(ana)).flatten();
            let mut known = in_call(call);
            known.media.calls.muted = muted;
            known.people.read_name(server.id, ana, "Ana");
            derive(
                std::slice::from_ref(&server),
                &selected(&server, scene, channel),
                &known,
            )
        };
        // Blocked by the meeting, with the hand raised.
        let view = shown(&scene, true, false, daily);
        let meeting = panel(&view).room.meeting.unwrap();
        assert!(!meeting.by_self && !meeting.may_speak);
        assert_eq!(meeting.presenter.as_deref(), Some("Ana"));
        assert!(meeting.hands.is_empty() && meeting.waiting.is_empty());
        let strip = view.call.unwrap();
        assert_eq!((strip.hand_raised, strip.may_speak), (Some(true), false));
        assert_eq!(view.controls.microphone, MicrophoneButton::Held);
        // The presenter removed the block, and the own setting is "closed": the person
        // may speak. The strip tells it when another channel is shown too.
        let view = shown(&scene, false, true, daily);
        assert!(panel(&view).room.meeting.unwrap().may_speak);
        assert_eq!(view.controls.microphone, MicrophoneButton::Muted);
        let view = shown(&scene, false, true, general);
        let strip = view.call.unwrap();
        assert!(strip.may_speak && !strip.shown);
        // The person opened the microphone: nothing is left to tell.
        let view = shown(&scene, false, false, daily);
        assert!(!panel(&view).room.meeting.unwrap().may_speak);
        assert!(!view.call.unwrap().may_speak);
    }

    #[test]
    fn the_room_shows_its_lock_its_limit_its_hide_setting_and_what_the_roles_allow() {
        let mut scene = scene();
        let (own, daily) = (scene.own, scene.daily);
        let ana = AccountId::generate();
        let call = scene.call(daily, vec![participant(ana), participant(own)]);
        call.lock = Some(CallLock {
            by: ana,
            at: time(),
        });
        for channel in &mut scene.studio_mut().channels {
            if channel.id == daily {
                channel.user_limit = Some(8);
                channel.hides_occupants = true;
            }
        }
        let room = |scene: &Scene| {
            let server = scene.server();
            let known = in_call(active(target(&server, scene, daily)));
            let view = derive(
                std::slice::from_ref(&server),
                &selected(&server, scene, daily),
                &known,
            );
            (panel(&view).room, view.workspace().unwrap().allowed)
        };
        let (shown, allowed) = room(&scene);
        assert!(shown.locked && shown.hides_occupants);
        // The call began before the setting was turned on, and keeps its own.
        assert!(shown.hide_waits_for_next_call);
        assert_eq!((shown.user_limit, shown.inside), (Some(8), 2));
        assert_eq!(shown.allowed, Allowed::default());
        assert_eq!(allowed, Allowed::default());
        everyone_may(
            &mut scene,
            &[
                WorkspacePermission::LockRoom,
                WorkspacePermission::ManageChannel,
                WorkspacePermission::MoveMembers,
            ],
        );
        let (shown, allowed) = room(&scene);
        assert_eq!(
            shown.allowed,
            Allowed {
                lock: true,
                manage: true,
                move_members: true,
                ..Allowed::default()
            }
        );
        assert_eq!(allowed, shown.allowed);
        // An administrator of the server may do each of them.
        scene.snapshot.account.is_owner = true;
        let (shown, _) = room(&scene);
        assert_eq!(
            shown.allowed,
            Allowed {
                manage_messages: true,
                mention_everyone: true,
                lock: true,
                run_meeting: true,
                manage: true,
                record: true,
                mute: true,
                deafen: true,
                disconnect: true,
                move_members: true,
                manage_recordings: true,
            }
        );
    }

    #[test]
    fn a_room_that_is_locked_or_full_tells_a_person_outside_and_shows_the_request_to_enter() {
        let mut scene = scene();
        let (own, daily, hidden) = (scene.own, scene.daily, scene.hidden);
        let ana = AccountId::generate();
        scene.call(daily, vec![participant(ana)]);
        let studio = scene.studio;
        scene.studio_mut().calls.push(ChannelCall {
            workspace_id: studio,
            channel_id: hidden,
            in_use: true,
            call: None,
        });
        let server_of = |scene: &Scene| scene.server();
        let closed = |scene: &Scene, known: &dyn Fn(&ServerSnapshot) -> Known, name: &str| {
            let server = server_of(scene);
            let view = derive(
                std::slice::from_ref(&server),
                &Selection::default(),
                &known(&server),
            );
            let voice = voice(view.workspace().unwrap(), name).clone();
            (voice.closed, voice.entry)
        };
        let nothing = |_: &ServerSnapshot| Known::default();
        // An open room.
        assert_eq!(closed(&scene, &nothing, "Daily"), (None, None));
        // A full room. The lock is told before the limit.
        for channel in &mut scene.studio_mut().channels {
            if channel.id == daily {
                channel.user_limit = Some(1);
            }
        }
        assert_eq!(
            closed(&scene, &nothing, "Daily"),
            (Some(Closed::Full), None)
        );
        scene.studio_mut().calls[0].call.as_mut().unwrap().lock = Some(CallLock {
            by: ana,
            at: time(),
        });
        assert_eq!(
            closed(&scene, &nothing, "Daily"),
            (Some(Closed::Locked), None)
        );
        // The request of the person is shown at its voice channel, and at no other.
        let asked = |state: EntryState| {
            move |server: &ServerSnapshot| Known {
                entry: Some(OwnEntry {
                    server_id: server.id,
                    channel_id: daily,
                    request_id: Some(EntryRequestId::generate()),
                    state,
                }),
                ..Known::default()
            }
        };
        assert_eq!(
            closed(&scene, &asked(EntryState::Waiting), "Daily"),
            (Some(Closed::Locked), Some(EntryState::Waiting))
        );
        assert_eq!(
            closed(&scene, &asked(EntryState::Denied), "HR"),
            (None, None)
        );
        // A voice channel that hides who is inside shows no lock. The answer of the
        // server to a join tells it (ADR 0514).
        let refused = |code: ErrorCode| {
            move |server: &ServerSnapshot| {
                let mut known = Known::default();
                known.media.calls.outcome = Some(Outcome::JoinFailed {
                    target: CallTarget {
                        server_id: server.id,
                        place: CallPlace::Channel {
                            workspace_id: studio,
                            channel_id: hidden,
                        },
                    },
                    error: FlowError::refused(code),
                });
                known
            }
        };
        assert_eq!(
            closed(&scene, &refused(ErrorCode::CallLocked), "HR").0,
            Some(Closed::Locked)
        );
        assert_eq!(
            closed(&scene, &refused(ErrorCode::CallFull), "HR").0,
            Some(Closed::Full)
        );
        assert_eq!(closed(&scene, &refused(ErrorCode::Forbidden), "HR").0, None);
        // A person inside is told nothing of this.
        scene.studio_mut().calls[0]
            .call
            .as_mut()
            .unwrap()
            .participants
            .push(participant(own));
        let inside = |server: &ServerSnapshot| {
            in_call(active(CallTarget {
                server_id: server.id,
                place: CallPlace::Channel {
                    workspace_id: studio,
                    channel_id: daily,
                },
            }))
        };
        assert_eq!(closed(&scene, &inside, "Daily"), (None, None));
    }

    #[test]
    fn invitations_and_requests_to_enter_are_listed_for_the_person_whom_they_concern() {
        let mut scene = scene();
        let (own, daily, studio) = (scene.own, scene.daily, scene.studio);
        let (ana, ben) = (AccountId::generate(), AccountId::generate());
        let call_id = scene.call(daily, vec![participant(ana)]).id;
        let place = CallPlace::Channel {
            workspace_id: studio,
            channel_id: daily,
        };
        let invitation = |account_id, invited_by| CallInvitation {
            id: CallInvitationId::generate(),
            call_id,
            place,
            account_id,
            invited_by,
            created_at: time(),
            expires_at: time() + time::Duration::seconds(60),
        };
        // Ana invites the person. Outside a call, the invitation waits for an answer.
        let incoming = invitation(own, ana);
        scene.snapshot.call_invitations = vec![incoming.clone()];
        let server = scene.server();
        let mut known = Known::default();
        known.people.read_name(server.id, ana, "Ana");
        let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
        assert_eq!(
            view.invitations,
            [Invited {
                id: incoming.id,
                caller: incoming.invited_by,
                target: target(&server, &scene, daily),
                by: Some("Ana".to_owned()),
                channel: Some("Daily".to_owned()),
            }]
        );
        assert!(view.entry_prompts.is_empty());
        // The profile of the caller is read for the name and the picture of the question.
        assert!(view.wanted_people[&server.id].accounts.contains(&ana));

        // The person is in the call and invited Ben, who is a member outside the call.
        // An invitation that the person made is no invitation for the person.
        let outgoing = invitation(ben, own);
        scene.snapshot.call_invitations = vec![outgoing.clone()];
        scene.studio_mut().calls[0]
            .call
            .as_mut()
            .unwrap()
            .participants
            .push(participant(own));
        let mut model = scene.model();
        let request = EntryRequest {
            id: EntryRequestId::generate(),
            call_id,
            place,
            account_id: ben,
            created_at: time(),
            expires_at: time() + time::Duration::seconds(60),
        };
        // A request for another call is not one for the call of this device.
        let elsewhere = EntryRequest {
            id: EntryRequestId::generate(),
            place: CallPlace::Channel {
                workspace_id: studio,
                channel_id: scene.hidden,
            },
            ..request.clone()
        };
        model.entry_requests = vec![elsewhere, request.clone()];
        let workspace = Arc::make_mut(&mut model.workspaces[1]);
        workspace.members = Loadable::Loaded(vec![
            WorkspaceMember {
                avatar: None,
                account_id: ben,
                name: "Ben".to_owned(),
                role_ids: Vec::new(),
            },
            WorkspaceMember {
                avatar: None,
                account_id: ana,
                name: "Ana".to_owned(),
                role_ids: Vec::new(),
            },
        ]);
        let server = super::server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let known = in_call(active(target(&server, &scene, daily)));
        let view = derive(
            std::slice::from_ref(&server),
            &selected(&server, &scene, daily),
            &known,
        );
        assert!(view.invitations.is_empty());
        assert_eq!(
            view.entry_prompts,
            [EntryPrompt {
                id: request.id,
                account_id: ben,
                name: Some("Ben".to_owned()),
                channel: Some("Daily".to_owned()),
            }]
        );
        assert_eq!(
            panel(&view).outside,
            [OutsideMember {
                account_id: ben,
                name: "Ben".to_owned(),
                place: None,
                invitation: Some(outgoing.id),
            }]
        );
    }

    #[test]
    fn detached_title_names_the_call_and_its_server() {
        let scene = scene();
        let server = scene.server();
        let known = in_call(active(target(&server, &scene, scene.daily)));
        let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
        assert_eq!(view.call_title(), "Daily · Acme Studio");
    }

    #[test]
    fn renewed_identity_and_moderator_moves_keep_the_detached_window() {
        let scene = scene();
        let server = scene.server();
        let before = active(target(&server, &scene, scene.daily));
        let mut after = before.clone();
        after.identity = "renewed-seat".to_owned();
        assert_eq!(
            crate::state::call_window::call_transition(Some(&before), Some(&after)),
            (false, false)
        );
        after.target = target(&server, &scene, scene.hidden);
        assert_eq!(
            crate::state::call_window::call_transition(Some(&before), Some(&after)),
            (false, true)
        );
        assert_eq!(
            crate::state::call_window::call_transition(Some(&before), None),
            (true, false)
        );
    }

    #[test]
    fn a_person_drop_chooses_invite_or_move_from_current_permissions() {
        use crate::state::person_drop::{PersonDrag, PersonDrop};
        let mut scene = scene();
        let (own, daily, other_room) = (scene.own, scene.daily, scene.hidden);
        scene
            .studio_mut()
            .channels
            .iter_mut()
            .find(|c| c.id == other_room)
            .unwrap()
            .hides_occupants = false;
        let other = AccountId::generate();
        scene.call(daily, vec![participant(own)]);
        scene.call(other_room, vec![participant(other)]);
        for may_move in [false, true] {
            scene.snapshot.account.is_owner = may_move;
            let server = scene.server();
            let known = in_call(active(target(&server, &scene, daily)));
            let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
            let person = PersonDrag {
                server_id: server.id,
                channel_id: other_room,
                account_id: other,
            };
            assert_eq!(
                view.person_drop(person, daily),
                Some(if may_move {
                    PersonDrop::Move
                } else {
                    PersonDrop::Invite
                })
            );
            assert_eq!(view.person_drop(person, other_room), None);
            assert_eq!(view.person_drop(person, scene.general), None);
            assert_eq!(
                view.person_drop(
                    PersonDrag {
                        server_id: ServerId::generate(),
                        ..person
                    },
                    daily
                ),
                None
            );
            assert_eq!(
                view.person_drop(
                    PersonDrag {
                        account_id: own,
                        ..person
                    },
                    daily
                ),
                None
            );
            assert_eq!(
                view.person_drop(
                    PersonDrag {
                        account_id: AccountId::generate(),
                        ..person
                    },
                    daily
                ),
                None
            );
            let outside = derive(
                std::slice::from_ref(&server),
                &Selection::default(),
                &Known::default(),
            );
            assert_eq!(
                outside.person_drop(person, daily),
                may_move.then_some(PersonDrop::Move)
            );
        }
    }

    #[test]
    fn the_menu_of_a_person_holds_what_the_roles_of_the_person_allow() {
        let mut scene = scene();
        let (own, daily, hidden) = (scene.own, scene.daily, scene.hidden);
        let (ana, ben) = (AccountId::generate(), AccountId::generate());
        let studio = scene.studio;
        let focus = channel(studio, ChannelKind::Voice, "Focus", 4);
        let focus_id = focus.id;
        scene.studio_mut().channels.push(focus);
        let mut blocked = participant(ben);
        blocked.microphone_block = Some(MicrophoneBlock {
            by: ana,
            reason: BlockReason::Moderator,
        });
        blocked.sound_block = Some(SoundBlock { by: ana });
        scene.call(daily, vec![participant(ana), participant(own)]);
        scene.call(focus_id, vec![blocked]);
        let menu = |scene: &Scene, channel_id: ChannelId, account_id: AccountId, inside: bool| {
            let server = scene.server();
            let mut known = if inside {
                in_call(active(target(&server, scene, daily)))
            } else {
                Known::default()
            };
            known.people.read_name(server.id, ben, "Ben");
            known.media.voice.set_volume(server.id, ben, 140);
            let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
            view.person_menu(channel_id, account_id)
                .map(|menu| (menu, server.id))
        };
        // The own row has no menu, and neither has a person who is not there.
        assert_eq!(menu(&scene, daily, own, true), None);
        assert_eq!(menu(&scene, focus_id, ana, true), None);
        assert_eq!(menu(&scene, hidden, ben, true), None);
        // A member without moderation: the invitation into the own room, and the volume.
        let (shown, server_id) = menu(&scene, focus_id, ben, true).unwrap();
        assert_eq!(
            shown,
            PersonMenu {
                server_id,
                channel_id: focus_id,
                account_id: ben,
                name: Some("Ben".to_owned()),
                place: "Focus".to_owned(),
                invite_to: Some("Daily".to_owned()),
                destinations: Vec::new(),
                volume: 140,
                microphone_blocked: None,
                sound_blocked: None,
                disconnect: false,
            }
        );
        // A person of the own call is not invited into it, and a person outside a call
        // invites nobody.
        assert_eq!(menu(&scene, daily, ana, true).unwrap().0.invite_to, None);
        assert_eq!(
            menu(&scene, focus_id, ben, false).unwrap().0.invite_to,
            None
        );
        // With the permissions of moderation: the blocks as they are, the removal, and
        // every other voice channel as a destination.
        everyone_may(
            &mut scene,
            &[
                WorkspacePermission::MuteMembers,
                WorkspacePermission::DeafenMembers,
                WorkspacePermission::DisconnectMembers,
                WorkspacePermission::MoveMembers,
            ],
        );
        let (shown, _) = menu(&scene, focus_id, ben, true).unwrap();
        assert_eq!(
            (
                shown.microphone_blocked,
                shown.sound_blocked,
                shown.disconnect
            ),
            (Some(true), Some(true), true)
        );
        assert_eq!(
            shown.destinations,
            [(daily, "Daily".to_owned()), (hidden, "HR".to_owned())]
        );
        let (shown, _) = menu(&scene, daily, ana, true).unwrap();
        assert_eq!(
            (shown.microphone_blocked, shown.sound_blocked),
            (Some(false), Some(false))
        );
        assert_eq!(
            shown.destinations,
            [(hidden, "HR".to_owned()), (focus_id, "Focus".to_owned())]
        );
    }
}

#[test]
fn direct_call_refusals_name_the_direct_call() {
    for code in [
        ErrorCode::NotFound,
        ErrorCode::RateLimited,
        ErrorCode::MediaUnavailable,
    ] {
        let mut known = Known::default();
        known.media.calls.outcome = Some(Outcome::JoinFailed {
            target: CallTarget {
                server_id: ServerId::generate(),
                place: CallPlace::direct(AccountId::generate(), AccountId::generate()),
            },
            error: FlowError::Refused {
                code,
                category: junto_client_core::api::ErrorCategory::ClientFault,
            },
        });
        let notice = call_notice(&[], &known).unwrap();
        assert!(matches!(&notice, CallNotice::DirectJoinFailed { .. }));
        assert!(!crate::text::call_notice(&notice).contains(crate::text::A_VOICE_CHANNEL));
    }
}

#[test]
fn a_direct_invitation_names_the_caller_without_a_cached_name() {
    use junto_client_core::protocol::{call::CallInvitation, ids::CallInvitationId};
    let mut scene = scene();
    let caller = AccountId::generate();
    scene.snapshot.call_invitations.push(CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::direct(scene.own, caller),
        account_id: scene.own,
        invited_by: caller,
        created_at: time(),
        expires_at: time() + time::Duration::seconds(60),
    });
    let mut model = scene.model();
    Arc::make_mut(&mut model.workspaces[1]).members = Loadable::Loaded(vec![WorkspaceMember {
        avatar: None,
        account_id: caller,
        name: "Ana".to_owned(),
        role_ids: Vec::new(),
    }]);
    let server = server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
    let view = derive_one(&server);
    assert_eq!(view.invitations.len(), 1);
    assert_eq!(view.invitations[0].by.as_deref(), Some("Ana"));
}

#[test]
fn direct_call_end_notices_do_not_describe_a_voice_channel() {
    use junto_client_core::protocol::call::LeaveReason;
    use junto_media::calls::{EndReason, Outcome};
    let scene = scene();
    let server = scene.server();
    let target = CallTarget {
        server_id: server.id,
        place: CallPlace::direct(scene.own, AccountId::generate()),
    };
    for reason in [
        EndReason::Server(Some(LeaveReason::AccessEnded)),
        EndReason::Server(Some(LeaveReason::Disconnected)),
        EndReason::Server(Some(LeaveReason::ConnectionLost)),
        EndReason::Server(Some(LeaveReason::NeverConnected)),
        EndReason::Elsewhere,
        EndReason::Refused(FlowError::NotAvailable),
    ] {
        let mut known = Known::default();
        known.media.calls.outcome = Some(Outcome::Ended { target, reason });
        let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
        let line = crate::text::call_notice(view.call_notice.as_ref().unwrap());
        assert!(!line.contains("voice channel"), "{line}");
        assert!(!line.contains("in Direct call"), "{line}");
    }
}
