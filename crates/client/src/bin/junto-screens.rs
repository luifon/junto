//! Draws the screens of the application without a window and writes one PNG file per
//! screen (ADR 0731). A developer compares the files with the design.
//!
//! The tool runs the real window code on gpui's test platform with its headless renderer,
//! so nothing appears on the screen and no keyboard focus is taken. A screen is drawn from
//! fixed data of servers; the join dialog is drawn from the real flow, against a transport
//! that answers as one server.
//!
//! ```sh
//! cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
//! ```

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, HeadlessAppContext, px, size};
use gpui_base::Root;
use gpui_base::input::InputState;
use junto_client::app::{self, WindowStart};
use junto_client::assets::Assets;
use junto_client::shell::{Open, Shell};
use junto_client::state::AppState;
use junto_client::state::view::ServerSnapshot;
use junto_client::theme;
use junto_client_core::api::{
    HttpRequest, HttpResponse, Transport, TransportError, TransportErrorKind, TransportFuture,
};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessState, Notice};
use junto_client_core::live::{StreamFault, StreamStatus};
use junto_client_core::model::{Loadable, ServerModel};
use junto_client_core::protocol::account::{OwnAccount, RecoveryNotice};
use junto_client_core::protocol::call::{
    BlockReason, Call, CallLock, CallParticipant, CallPlace, ChannelCall, Meeting, MicrophoneBlock,
    ScreenShare, ShareAudio,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::Cursor;
use junto_client_core::protocol::ids::{AccountId, ChannelId, WorkspaceId};
use junto_client_core::protocol::recording::{Recording, RecordingState};
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::server::ServerInfo;
use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
use junto_client_core::protocol::structure::{
    Category, Channel, ChannelKind, Workspace, WorkspaceMember,
};
use junto_client_core::secret::SecretBytes;
use junto_client_core::version::{Incompatibility, SupportedVersions};
use junto_client_core::{Client, ClientOptions, Reach};
use time::OffsetDateTime;

/// The window of the design: 1440 by 900 pixels at 100 %.
const WINDOW: (f32, f32) = (1440.0, 900.0);

/// An identifier that is the same in every run, so that a generated avatar is the same.
fn id<T: FromStr>(number: u32) -> T
where
    T::Err: std::fmt::Debug,
{
    match format!("0198f1c2-7d3a-7b1e-9c55-{number:012x}").parse() {
        Ok(id) => id,
        Err(error) => panic!("a fixed identifier is not valid: {error:?}"),
    }
}

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_626_640).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn reachable() -> Reach {
    match SupportedVersions::THIS_BUILD.negotiate(&[SupportedVersions::THIS_BUILD.current()]) {
        Ok(version) => Reach::Reachable { version },
        Err(_) => Reach::Unknown,
    }
}

const PEOPLE: [&str; 7] = ["Luca", "Ana", "Bruno", "Carla", "Marta", "Diego", "Elisa"];

fn person(name: &str) -> AccountId {
    let index = PEOPLE
        .iter()
        .position(|person| *person == name)
        .unwrap_or(PEOPLE.len());
    id(100 + index as u32)
}

fn participant(name: &str) -> CallParticipant {
    CallParticipant {
        account_id: person(name),
        identity: format!("seat-{name}"),
        joined_at: at(),
        muted: false,
        deafened: false,
        microphone_block: None,
        sound_block: None,
        hand_raised: false,
        screen_share: None,
    }
}

fn channel(number: u32, workspace_id: WorkspaceId, kind: ChannelKind, name: &str) -> Channel {
    Channel {
        id: id(number),
        workspace_id,
        category_id: None,
        kind,
        name: name.to_owned(),
        position: number as i32,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    }
}

fn workspace(number: u32, name: &str, is_default: bool) -> WorkspaceState {
    WorkspaceState {
        workspace: Workspace {
            id: id(number),
            name: name.to_owned(),
            is_default,
        },
        own_role_ids: Vec::new(),
        roles: Vec::new(),
        categories: Vec::new(),
        channels: Vec::new(),
        calls: Vec::new(),
    }
}

/// The workspace "Studio" of the design: four text channels, and three voice channels of
/// which one is recorded, one is locked and one hides who is inside.
fn studio() -> WorkspaceState {
    let mut studio = workspace(10, "Studio", true);
    let workspace_id = studio.workspace.id;
    let role = |number, name: &str, is_everyone| WorkspaceRole {
        id: id(number),
        workspace_id,
        name: name.to_owned(),
        permissions: Vec::new(),
        is_everyone,
    };
    studio.roles = vec![
        role(20, "everyone", true),
        role(21, "Leads", false),
        role(22, "Engineers", false),
    ];
    let voice = |number, name: &str| channel(number, workspace_id, ChannelKind::Voice, name);
    let daily = voice(34, "Daily");
    let focus = voice(35, "Focus room");
    let mut hidden = voice(36, "HR");
    hidden.hides_occupants = true;
    studio.channels = vec![
        channel(30, workspace_id, ChannelKind::Text, "general"),
        channel(31, workspace_id, ChannelKind::Text, "client-alpha"),
        channel(32, workspace_id, ChannelKind::Text, "deploys"),
        channel(33, workspace_id, ChannelKind::Text, "random"),
        daily.clone(),
        focus.clone(),
        hidden.clone(),
    ];
    let place = |channel_id: ChannelId| CallPlace::Channel {
        workspace_id,
        channel_id,
    };
    let mut bruno = participant("Bruno");
    bruno.muted = true;
    let mut carla = participant("Carla");
    carla.screen_share = Some(ScreenShare {
        audio: ShareAudio::Application,
        started_at: at(),
    });
    let mut marta = participant("Marta");
    marta.muted = true;
    let daily_call = id(40);
    studio.calls = vec![
        ChannelCall {
            workspace_id,
            channel_id: daily.id,
            in_use: true,
            call: Some(Call {
                id: daily_call,
                place: place(daily.id),
                started_at: Some(at()),
                lock: None,
                meeting: None,
                participants: vec![participant("Ana"), bruno, carla, marta],
                recording: Some(Recording {
                    id: id(41),
                    workspace_id,
                    channel_id: daily.id,
                    call_id: daily_call,
                    state: RecordingState::Recording,
                    started_by: person("Carla"),
                    requested_at: at(),
                    recording_since: Some(at()),
                    stop: None,
                    ended_at: None,
                    failure: None,
                    language: "auto".to_owned(),
                    segments: 5,
                    failed_segments: 0,
                }),
            }),
        },
        ChannelCall {
            workspace_id,
            channel_id: focus.id,
            in_use: true,
            call: Some(Call {
                id: id(42),
                place: place(focus.id),
                started_at: Some(at()),
                lock: Some(CallLock {
                    by: person("Diego"),
                    at: at(),
                }),
                meeting: None,
                participants: vec![participant("Diego"), participant("Elisa")],
                recording: None,
            }),
        },
        ChannelCall {
            workspace_id,
            channel_id: hidden.id,
            in_use: true,
            call: None,
        },
    ];
    studio
}

fn snapshot(account: &str, workspaces: Vec<WorkspaceState>) -> Snapshot {
    Snapshot {
        cursor: Cursor {
            epoch: id(1),
            position: 40,
        },
        account: OwnAccount {
            id: person(account),
            name: account.to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: workspaces
                .iter()
                .map(|workspace| workspace.workspace.id)
                .collect(),
        },
        workspaces,
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        direct_mutes: Vec::new(),
    }
}

/// The state of "Acme Studio" as Luca sees it, with the members of "Studio" loaded and Ana
/// speaking.
fn acme_model(change: impl FnOnce(&mut WorkspaceState)) -> Arc<ServerModel> {
    let mut studio = studio();
    change(&mut studio);
    let mut model = ServerModel::from_snapshot(snapshot(
        "Luca",
        vec![
            studio,
            workspace(11, "Client Alpha", false),
            workspace(12, "Client Beta", false),
        ],
    ));
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    let roles: Vec<_> = studio.roles.iter().map(|role| role.id).collect();
    studio.members = Loadable::Loaded(
        PEOPLE
            .iter()
            .enumerate()
            .map(|(index, name)| WorkspaceMember {
                account_id: person(name),
                name: (*name).to_owned(),
                role_ids: match index {
                    1 => vec![roles[1]],
                    2 | 5 => vec![roles[2]],
                    3 => vec![roles[1], roles[2]],
                    _ => Vec::new(),
                },
            })
            .collect(),
    );
    for channel in &mut studio.channels {
        if let Some(call) = &mut channel.call
            && channel.channel.name == "Daily"
        {
            call.speaking = vec![person("Ana")];
        }
    }
    Arc::new(model)
}

fn logged_in(account: &str, recoveries: Vec<RecoveryNotice>) -> AccessState {
    AccessState::LoggedIn {
        account_id: person(account),
        device_id: id(300),
        session_expires_at: at(),
        recoveries,
        previous_account: None,
    }
}

fn acme() -> ServerSnapshot {
    ServerSnapshot {
        id: id(1),
        name: "Acme Studio".to_owned(),
        address: "acme.example:7443".to_owned(),
        keep_connected: true,
        reach: reachable(),
        access: logged_in("Luca", Vec::new()),
        in_progress: None,
        error: None,
        stream: StreamStatus::Live,
        model: Some(acme_model(|_| {})),
    }
}

fn harbor() -> ServerSnapshot {
    ServerSnapshot {
        id: id(2),
        name: "Harbor".to_owned(),
        address: "harbor.example:7443".to_owned(),
        keep_connected: true,
        reach: reachable(),
        access: logged_in("Luca", Vec::new()),
        in_progress: None,
        error: None,
        stream: StreamStatus::Live,
        model: Some(Arc::new(ServerModel::from_snapshot(snapshot(
            "Luca",
            vec![workspace(13, "Lounge", true)],
        )))),
    }
}

/// "Harbor" in a state without lists.
fn harbor_as(access: AccessState, reach: Reach, error: Option<FlowError>) -> ServerSnapshot {
    ServerSnapshot {
        access,
        reach,
        error,
        stream: StreamStatus::Off,
        model: None,
        ..harbor()
    }
}

/// A transport that answers as the server "Harbor" and reaches nothing else.
struct OneServer;

impl Transport for OneServer {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        Box::pin(async move {
            if request.url.path() != "/api/server" {
                return Err(TransportError::new(
                    TransportErrorKind::Connect,
                    "no server in the screen tool",
                ));
            }
            let info = ServerInfo {
                server_id: id(2),
                name: "Harbor".to_owned(),
                logo_path: None,
                api_versions: vec![SupportedVersions::THIS_BUILD.current()],
            };
            let body = serde_json::to_vec(&info).map_err(|error| {
                TransportError::new(TransportErrorKind::Interrupted, error.to_string())
            })?;
            Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: SecretBytes::from_vec(body),
            })
        })
    }
}

struct Studio {
    cx: HeadlessAppContext,
    out: PathBuf,
    scratch: PathBuf,
    drawn: usize,
}

/// One screen: the servers it shows and the interface scale.
struct Screen {
    name: &'static str,
    servers: Vec<ServerSnapshot>,
    scale: u32,
    /// The name of the profile preset. Empty: the first start.
    preset: &'static str,
}

impl Screen {
    fn new(name: &'static str, servers: Vec<ServerSnapshot>) -> Self {
        Self {
            name,
            servers,
            scale: 100,
            preset: "Luca",
        }
    }

    fn scale(mut self, scale: u32) -> Self {
        self.scale = scale;
        self
    }

    fn preset(mut self, preset: &'static str) -> Self {
        self.preset = preset;
        self
    }
}

type Prepare = Box<dyn FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>)>;

impl Studio {
    fn new(out: PathBuf) -> Result<Self, String> {
        let scratch = std::env::temp_dir().join(format!("junto-screens-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
        let text_system = gpui_platform::current_platform(true).text_system();
        let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), || {
            gpui_platform::current_headless_renderer()
        });
        cx.update(app::init).map_err(|error| format!("{error:#}"))?;
        Ok(Self {
            cx,
            out,
            scratch,
            drawn: 0,
        })
    }

    /// Draws one screen. `prepare` opens what the screen shows beyond its servers.
    fn draw(
        &mut self,
        screen: Screen,
        prepare: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
    ) -> Result<(), String> {
        self.draw_after(screen, Box::new(prepare), |_| true)
    }

    /// Like [`Studio::draw`], and waits until `ready` holds before the screen is captured.
    fn draw_after(
        &mut self,
        screen: Screen,
        prepare: Prepare,
        ready: impl Fn(&Shell) -> bool,
    ) -> Result<(), String> {
        self.drawn += 1;
        let data_dir = self.scratch.join(format!("data-{}", self.drawn));
        let mut options = ClientOptions::new(data_dir);
        options.transport = Some(Arc::new(OneServer));
        let client = Client::open(options).map_err(|error| error.to_string())?;
        if !screen.preset.is_empty() {
            client
                .profile()
                .set_name(screen.preset)
                .map_err(|error| error.to_string())?;
        }
        let start = WindowStart {
            scale_percent: Some(screen.scale),
            activate: false,
            ..WindowStart::default()
        };
        let servers = screen.servers;
        let state: Entity<AppState> = self.cx.update(|cx| {
            let state = app::state(client, start, cx);
            if !servers.is_empty() {
                state.update(cx, |state, cx| state.show_fixture(servers, cx));
            }
            state
        });
        let scale = screen.scale;
        let built: std::rc::Rc<std::cell::RefCell<Option<Entity<Shell>>>> = std::rc::Rc::default();
        let slot = built.clone();
        let window = self
            .cx
            .open_window(size(px(WINDOW.0), px(WINDOW.1)), move |window, cx| {
                window.set_rem_size(theme::rem_size(scale));
                let shell = cx.new(|cx| {
                    let mut shell = Shell::new(state, window, cx);
                    prepare(&mut shell, window, cx);
                    shell
                });
                *slot.borrow_mut() = Some(shell.clone());
                cx.new(|cx| Root::new(shell, window, cx))
            })
            .map_err(|error| format!("{error:#}"))?;
        let shell = built
            .borrow_mut()
            .take()
            .ok_or("the window was not built")?;
        let started = Instant::now();
        loop {
            self.cx.run_until_parked();
            if self.cx.update(|cx| ready(shell.read(cx))) {
                break;
            }
            if started.elapsed() > Duration::from_secs(10) {
                return Err(format!(
                    "{}: the screen did not reach its state",
                    screen.name
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Two frames: the first lays the layers out, the second draws what they opened.
        for _ in 0..2 {
            self.cx
                .update_window(window.into(), |_, window, _| window.refresh())
                .map_err(|error| format!("{error:#}"))?;
            self.cx.run_until_parked();
        }
        let image = self
            .cx
            .capture_screenshot(window.into())
            .map_err(|error| format!("{}: {error:#}", screen.name))?;
        let file = self.out.join(format!("{}.png", screen.name));
        image.save(&file).map_err(|error| error.to_string())?;
        println!("{}", file.display());
        self.cx
            .update_window(window.into(), |_, window, _| window.remove_window())
            .map_err(|error| format!("{error:#}"))?;
        self.cx.run_until_parked();
        Ok(())
    }
}

fn two_servers() -> Vec<ServerSnapshot> {
    vec![acme(), harbor()]
}

fn recovery() -> RecoveryNotice {
    RecoveryNotice {
        id: id(500),
        device_id: Some(id(301)),
        performed_by: Some(person("Ana")),
        performed_at: at(),
    }
}

fn select_channel(shell: &mut Shell, name: &str, cx: &mut gpui::Context<Shell>) {
    let state = shell.state().clone();
    let view = state.read(cx).view();
    let Some(workspace) = view.workspace() else {
        return;
    };
    let found = workspace
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .find(|row| row.name == name)
        .map(|row| row.id);
    if let Some(channel_id) = found {
        let workspace_id = workspace.id;
        state.update(cx, |state, cx| {
            state.select_channel(workspace_id, channel_id, cx);
        });
    }
}

fn select_harbor(shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>) {
    shell
        .state()
        .clone()
        .update(cx, |state, cx| state.select_server(id(2), cx));
}

/// "Acme Studio" with meeting mode in Daily, a full room, a category and a person whose
/// sound is off.
fn acme_in_a_meeting() -> ServerSnapshot {
    let mut meeting = acme();
    meeting.model = Some(acme_model(|studio| {
        let workspace_id = studio.workspace.id;
        let category = Category {
            id: id(60),
            workspace_id,
            name: "Projects".to_owned(),
            position: 0,
        };
        for channel in &mut studio.channels {
            if matches!(channel.name.as_str(), "client-alpha" | "deploys") {
                channel.category_id = Some(category.id);
            }
            if channel.name == "Focus room" {
                channel.user_limit = Some(2);
            }
        }
        // More channels than the column shows at 150 %, so that the list scrolls.
        for number in 0..6 {
            let mut more = channel(
                70 + number,
                workspace_id,
                ChannelKind::Text,
                &format!("project-{}", number + 1),
            );
            more.category_id = Some(category.id);
            studio.channels.push(more);
        }
        studio.categories = vec![category];
        for shown in &mut studio.calls {
            let Some(call) = &mut shown.call else {
                continue;
            };
            if call.participants.len() == 4 {
                call.meeting = Some(Meeting {
                    presenter: person("Carla"),
                    started_at: at(),
                });
                for participant in &mut call.participants {
                    if participant.account_id != person("Carla") {
                        participant.microphone_block = Some(MicrophoneBlock {
                            by: person("Carla"),
                            reason: BlockReason::Meeting,
                        });
                    }
                }
                call.participants[0].hand_raised = true;
                call.participants[3].deafened = true;
            } else {
                call.lock = None;
            }
        }
    }));
    meeting
}

fn waiting() -> ServerSnapshot {
    harbor_as(
        AccessState::WaitingForApproval {
            join_request_id: Some(id(700)),
            name: "Luca".to_owned(),
        },
        reachable(),
        None,
    )
}

fn logged_out() -> AccessState {
    AccessState::LoggedOut {
        account_id: person("Luca"),
    }
}

fn draw_all(studio: &mut Studio) -> Result<(), String> {
    // The main layout. The design has a mockup for these (view A).
    studio.draw(
        Screen::new("main-text-channel", two_servers()),
        |_, _, _| {},
    )?;
    studio.draw(
        Screen::new("main-scale-150", two_servers()).scale(150),
        |_, _, _| {},
    )?;
    studio.draw(
        Screen::new("main-scale-150-members", two_servers()).scale(150),
        |shell, window, cx| shell.open(Open::Members, window, cx),
    )?;
    studio.draw(
        Screen::new("main-scale-90", two_servers()).scale(90),
        |_, _, _| {},
    )?;
    studio.draw(
        Screen::new("main-voice-channel", two_servers()),
        |shell, _, cx| select_channel(shell, "Daily", cx),
    )?;
    studio.draw(
        Screen::new("main-voice-channel-locked", two_servers()),
        |shell, _, cx| select_channel(shell, "Focus room", cx),
    )?;
    studio.draw(
        Screen::new("main-voice-channel-hidden", two_servers()),
        |shell, _, cx| select_channel(shell, "HR", cx),
    )?;
    studio.draw(
        Screen::new("main-members", two_servers()),
        |shell, window, cx| shell.open(Open::Members, window, cx),
    )?;
    studio.draw(
        Screen::new("server-book-collapsed-menu", two_servers()),
        |shell, window, cx| {
            shell
                .state()
                .clone()
                .update(cx, |state, cx| state.set_server_book_collapsed(true, cx));
            shell.open(Open::ServerHoverMenu, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("server-menu", two_servers()),
        |shell, window, cx| shell.open(Open::ServerMenu, window, cx),
    )?;
    studio.draw(
        Screen::new("settings", two_servers()),
        |shell, window, cx| shell.open(Open::Settings, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "main-meeting-mode-full-room-categories",
            vec![acme_in_a_meeting(), harbor()],
        ),
        |shell, _, cx| select_channel(shell, "Daily", cx),
    )?;

    studio.draw(
        Screen::new(
            "main-scale-150-list-scrolls",
            vec![acme_in_a_meeting(), harbor()],
        )
        .scale(150),
        |_, _, _| {},
    )?;

    // Joining a server. The design has a mockup for the three steps (view I).
    studio.draw(
        Screen::new("join-step-1-address", vec![acme()]),
        |shell, window, cx| shell.type_address("harbor.example", window, cx),
    )?;
    studio.draw_after(
        Screen::new("join-step-2-server-found", vec![acme()]),
        Box::new(|shell, window, cx| {
            shell.type_address("harbor.example", window, cx);
            shell.submit_address(window, cx);
        }),
        Shell::join_is_open,
    )?;
    studio.draw(
        Screen::new(
            "join-step-2-code-refused",
            vec![
                acme(),
                harbor_as(
                    AccessState::NoAccount { notice: None },
                    reachable(),
                    Some(FlowError::refused(ErrorCode::InviteNotFound)),
                ),
            ],
        ),
        |shell, window, cx| {
            select_harbor(shell, window, cx);
            shell.open_join_for(id(2), window, cx);
            shell.type_code("K7QM-2XHD-9PWA-4RTC", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("join-step-3-request-sent", vec![acme(), waiting()]),
        |shell, window, cx| shell.open_join_for(id(2), window, cx),
    )?;
    studio.draw_after(
        Screen::new("join-address-refused", vec![acme()]),
        Box::new(|shell, window, cx| {
            shell.type_address("harbor.example/chat", window, cx);
            shell.submit_address(window, cx);
        }),
        Shell::address_was_refused,
    )?;

    // The first start and a window without a server.
    studio.draw(
        Screen::new("first-start-name", Vec::new()).preset(""),
        |_, _, _| {},
    )?;
    studio.draw(Screen::new("no-server", Vec::new()), |_, _, _| {})?;

    // The states of a server.
    let states: Vec<(&'static str, ServerSnapshot)> = vec![
        ("server-waiting-for-approval", waiting()),
        (
            "server-not-joined",
            harbor_as(AccessState::NoAccount { notice: None }, reachable(), None),
        ),
        (
            "server-removed",
            harbor_as(
                AccessState::NoAccount {
                    notice: Some(Notice::Removed),
                },
                reachable(),
                None,
            ),
        ),
        (
            "server-request-denied",
            harbor_as(
                AccessState::NoAccount {
                    notice: Some(Notice::RequestDenied),
                },
                reachable(),
                None,
            ),
        ),
        (
            "server-banned",
            harbor_as(
                AccessState::Banned {
                    account_id: Some(person("Luca")),
                },
                reachable(),
                None,
            ),
        ),
        (
            "server-unreachable",
            harbor_as(
                logged_out(),
                Reach::Unreachable,
                Some(FlowError::Unreachable(TransportError::new(
                    TransportErrorKind::Connect,
                    "connection refused",
                ))),
            ),
        ),
        (
            "server-connecting",
            harbor_as(logged_out(), Reach::Unknown, None),
        ),
        (
            "server-login-refused",
            harbor_as(logged_out(), reachable(), Some(FlowError::OriginRejected)),
        ),
        (
            "server-needs-newer-client",
            harbor_as(
                logged_out(),
                Reach::Incompatible(Incompatibility::ClientTooOld { server_oldest: 3 }),
                None,
            ),
        ),
        (
            "server-other-server-answers",
            harbor_as(logged_out(), Reach::OtherServer { reported: id(9) }, None),
        ),
    ];
    for (name, server) in states {
        studio.draw(Screen::new(name, vec![acme(), server]), select_harbor)?;
    }
    let mut interrupted = acme();
    interrupted.stream = StreamStatus::Interrupted(StreamFault::Silent);
    studio.draw(
        Screen::new("server-reconnecting", vec![interrupted, harbor()]),
        |_, _, _| {},
    )?;
    let mut recovered = acme();
    recovered.access = logged_in("Luca", vec![recovery()]);
    studio.draw(
        Screen::new("recovery-notice", vec![recovered, harbor()]),
        |_, _, _| {},
    )?;
    studio.draw(
        Screen::new("remove-server-question", vec![acme(), waiting()]),
        |shell, window, cx| shell.ask_to_remove(id(2), window, cx),
    )?;
    Ok(())
}

/// Every component that no screen of this version shows in each of its states.
struct Components {
    field: Entity<InputState>,
    code: Entity<InputState>,
    focus: gpui::FocusHandle,
}

impl gpui::Render for Components {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{Hsla, ParentElement, Styled, div};
        use junto_client::assets::Icon;
        use junto_client::theme::u;
        use junto_client::ui::{
            Badge, Band, BandKind, Button, ButtonKind, Chip, Field, FieldKind, Flag, FlagKind,
            Menu, MenuItem, Meter, Mosaic, MosaicKind, Notice, RecordingMark, SectionLabel,
            Segmented, StyleExt, WaitBox, menu_separator, unread_dot,
        };
        let theme = theme::theme(cx);
        let colors = theme.colors;
        let row = || div().flex().items_center().gap(u(10.0)).flex_wrap();
        let label = |text: &'static str| {
            div()
                .w(u(150.0))
                .flex_none()
                .typeface(theme.types.label, &theme)
                .ink(colors.bronze)
                .child(text)
        };
        let buttons = |kind: ButtonKind, name: &'static str| {
            row()
                .child(label(name))
                .child(Button::new((name, 0usize)).kind(kind).label("Label"))
                .child(
                    Button::new((name, 1usize))
                        .kind(kind)
                        .icon(Icon::People)
                        .label("With icon"),
                )
                .child(
                    Button::new((name, 2usize))
                        .kind(kind)
                        .label("Disabled")
                        .disabled(true),
                )
        };
        let icon_buttons = |kind: ButtonKind, name: &'static str| {
            row()
                .child(label(name))
                .child(Button::new((name, 0usize)).kind(kind).icon(Icon::Gear))
                .child(
                    Button::new((name, 1usize))
                        .kind(kind)
                        .icon(Icon::Microphone)
                        .disabled(true),
                )
        };
        let dark = div()
            .flex()
            .flex_col()
            .gap(u(12.0))
            .p(u(14.0))
            .w(u(420.0))
            .bg(Hsla::from(colors.basalt))
            .child(icon_buttons(ButtonKind::Rail, "Rail button"))
            .child(icon_buttons(ButtonKind::RailAccent, "Rail accent"))
            .child(Field::new(&self.field, "Address").kind(FieldKind::RailAddress))
            .child(Band::new(BandKind::Rail));
        div()
            .size_full()
            .flex()
            .gap(u(24.0))
            .p(u(24.0))
            .bg(Hsla::from(colors.travertine))
            .typeface(theme.types.body, &theme)
            .ink(colors.ink)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(14.0))
                    .w(u(640.0))
                    .child(buttons(ButtonKind::Default, "Button"))
                    .child(buttons(ButtonKind::Primary, "Primary button"))
                    .child(icon_buttons(ButtonKind::Square, "Square button"))
                    .child(icon_buttons(ButtonKind::Ghost, "Icon button"))
                    .child(
                        row()
                            .child(label("Field"))
                            .child(div().w(u(220.0)).child(Field::new(&self.field, "Field")))
                            .child(
                                div()
                                    .w(u(220.0))
                                    .child(Field::new(&self.code, "Code").kind(FieldKind::Code)),
                            ),
                    )
                    .child(
                        row().child(label("Segmented")).child(
                            Segmented::new(
                                "segments",
                                "Choice",
                                ["One", "Two", "Three"].map(gpui::SharedString::from),
                            )
                            .selected(Some(1)),
                        ),
                    )
                    .child(
                        row()
                            .child(label("Marks"))
                            .child(Badge::new("@ 2"))
                            .child(unread_dot(cx))
                            .child(Chip::new("Server found").icon(Icon::Check))
                            .child(RecordingMark::new("REC"))
                            .child(Meter::new(0))
                            .child(Meter::new(3))
                            .child(Meter::new(5)),
                    )
                    .child(
                        row()
                            .child(label("Flags"))
                            .child(Flag::new(FlagKind::Warning, "Reconnecting…").icon(Icon::Retry))
                            .child(
                                Flag::new(FlagKind::Error, "Recording failed").icon(Icon::Warning),
                            ),
                    )
                    .child(
                        row()
                            .child(label("Avatars"))
                            .children(
                                (1..=8u128)
                                    .map(|seed| Mosaic::new(MosaicKind::Large, seed * 7919, 34.0)),
                            )
                            .children((1..=4u128).map(|seed| {
                                Mosaic::new(MosaicKind::Small, seed * 104_729, 15.0).radius(2.0)
                            })),
                    )
                    .child(Band::new(BandKind::Tessera))
                    .child(Notice::new("A fact that the person should read."))
                    .child(Notice::error("A failure, with what the person can do."))
                    .child(WaitBox::new(
                        "Something waits for another person",
                        "The box says who must act and what happens then.",
                    ))
                    .child(SectionLabel::new("Section label").count(3)),
            )
            .child(
                div().flex().flex_col().gap(u(18.0)).child(dark).child(
                    Menu::new("menu", "Menu", &self.focus)
                        .child(
                            MenuItem::new("one", "A switch")
                                .trailing("On")
                                .detail("A second line says what the item does."),
                        )
                        .child(menu_separator(cx))
                        .child(MenuItem::new("two", "An action").icon(Icon::Trash))
                        .child(div().h(u(4.0))),
                ),
            )
    }
}

fn draw_components(studio: &mut Studio) -> Result<(), String> {
    let window = studio
        .cx
        .open_window(size(px(WINDOW.0), px(WINDOW.1)), |window, cx| {
            window.set_rem_size(theme::rem_size(100));
            let components = cx.new(|cx| {
                let field = junto_client::ui::Field::state("Placeholder", window, cx);
                let code = junto_client::ui::Field::state("", window, cx);
                code.update(cx, |code, cx| code.set_value("K7QM-2XHD", window, cx));
                Components {
                    field,
                    code,
                    focus: cx.focus_handle(),
                }
            });
            cx.new(|cx| Root::new(components, window, cx))
        })
        .map_err(|error| format!("{error:#}"))?;
    for _ in 0..2 {
        studio
            .cx
            .update_window(window.into(), |_, window, _| window.refresh())
            .map_err(|error| format!("{error:#}"))?;
        studio.cx.run_until_parked();
    }
    let image = studio
        .cx
        .capture_screenshot(window.into())
        .map_err(|error| format!("components: {error:#}"))?;
    let file = studio.out.join("components.png");
    image.save(&file).map_err(|error| error.to_string())?;
    println!("{}", file.display());
    studio
        .cx
        .update_window(window.into(), |_, window, _| window.remove_window())
        .map_err(|error| format!("{error:#}"))?;
    studio.cx.run_until_parked();
    Ok(())
}

fn run(out: &Path) -> Result<(), String> {
    let mut studio = Studio::new(out.to_owned())?;
    let drawn = draw_all(&mut studio).and_then(|()| draw_components(&mut studio));
    let scratch = studio.scratch.clone();
    drop(studio);
    // The data directories of the screens hold no server and no key of a real server.
    let removed = std::fs::remove_dir_all(&scratch).map_err(|error| error.to_string());
    drawn.and(removed)
}

fn main() -> std::process::ExitCode {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: junto-screens <directory for the PNG files>");
        return std::process::ExitCode::from(2);
    };
    match run(Path::new(&out)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("junto-screens: {problem}");
            std::process::ExitCode::FAILURE
        }
    }
}
