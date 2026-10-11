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

fn time() -> OffsetDateTime {
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
struct Scene {
    own: AccountId,
    studio: WorkspaceId,
    lab: WorkspaceId,
    general: ChannelId,
    random: ChannelId,
    daily: ChannelId,
    hidden: ChannelId,
    snapshot: Snapshot,
}

fn scene() -> Scene {
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
        direct_mutes: Vec::new(),
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
    fn studio_mut(&mut self) -> &mut WorkspaceState {
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
                participants,
                recording: None,
            }),
        });
        calls.last_mut().unwrap().call.as_mut().unwrap()
    }

    fn model(&self) -> ServerModel {
        ServerModel::from_snapshot(self.snapshot.clone())
    }

    fn server(&self) -> ServerSnapshot {
        server(
            "Acme Studio",
            Some(self.model()),
            logged_in(self.own, Vec::new()),
        )
    }
}

fn logged_in(account_id: AccountId, recoveries: Vec<RecoveryNotice>) -> AccessState {
    AccessState::LoggedIn {
        account_id,
        device_id: DeviceId::generate(),
        session_expires_at: time(),
        recoveries,
        previous_account: None,
    }
}

fn server(name: &str, model: Option<ServerModel>, access: AccessState) -> ServerSnapshot {
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
    selection.channels.insert(scene.studio, scene.daily);
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
        .insert(scene.studio, ChannelId::generate());
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
            account_id: ana,
            name: "Ana".to_owned(),
            role_ids: Vec::new(),
        },
        WorkspaceMember {
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
    known.names.insert((server.id, carla), "Carla".to_owned());
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
    let role = |name: &str, is_everyone| WorkspaceRole {
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: name.to_owned(),
        permissions: Vec::new(),
        is_everyone,
    };
    let (everyone, leads, engineers) = (
        role("everyone", true),
        role("Leads", false),
        role("Engineers", false),
    );
    scene.studio_mut().roles = vec![everyone.clone(), leads.clone(), engineers.clone()];
    let member = |account_id, name: &str, role_ids| WorkspaceMember {
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
        member(AccountId::generate(), "Carla", vec![engineers.id, leads.id]),
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
    use junto_client_core::voice::VoiceSettings;
    use junto_media::audio::{AudioError, DeviceId, DeviceInfo, Devices};
    use junto_media::calls::{ActiveCall, CallTarget, CallsView, EndReason, Outcome, Phase};
    use junto_media::engine::{MicrophoneState, OutputState};
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
        }
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

    fn selected(scene: &Scene, channel_id: ChannelId) -> Selection {
        Selection {
            channels: [(scene.studio, channel_id)].into_iter().collect(),
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
        let outside = derive(servers, &selected(&scene, daily), &Known::default());
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
        let inside = derive(servers, &selected(&scene, daily), &known);
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
        let elsewhere = derive(servers, &selected(&scene, general), &known);
        assert!(elsewhere.workspace().unwrap().call.is_none());
        let strip = elsewhere.call.unwrap();
        assert_eq!((strip.name.as_deref(), strip.shown), (Some("Daily"), false));
        // A clock of this machine that is behind the server shows no negative time.
        known.media.now = Some(time() - time::Duration::seconds(5));
        let early = derive(servers, &selected(&scene, daily), &known);
        assert_eq!(early.call.unwrap().elapsed, Some(Duration::ZERO));
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
        let view = derive(servers, &selected(&scene, daily), &in_call(call.clone()));
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
            &selected(&empty, empty.daily),
            &in_call(call),
        );
        let people = &voice(view.workspace().unwrap(), "Daily").occupants;
        assert_eq!(people.len(), 1);
        assert!(people[0].is_self);

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
            &selected(&scene, daily),
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
            account_id: ana,
            name: "Ana".to_owned(),
            role_ids: Vec::new(),
        }]);
        let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
        let servers = std::slice::from_ref(&server);
        let at = |change: &dyn Fn(&mut Known)| {
            let mut known = in_call(active(target(&server, &scene, daily)));
            change(&mut known);
            derive(servers, &selected(&scene, daily), &known).controls
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
            &selected(&scene, daily),
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
            derive(servers, &selected(&scene, scene.daily), &known)
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
            &selected(&scene, daily),
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
            &selected(&scene, daily),
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
            &selected(&scene, daily),
            &known,
        );
        let outside = view.workspace().unwrap().call.clone().unwrap().outside;
        let shown: Vec<(&str, Option<&str>)> = outside
            .iter()
            .map(|member| (member.name.as_str(), member.place.as_deref()))
            .collect();
        assert_eq!(shown, [("Ben", Some("Focus room")), ("Cal", None)]);
    }
}
