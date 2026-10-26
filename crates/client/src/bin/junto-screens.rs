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
//! JUNTO_SCREENS_THEME=dark cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
//! JUNTO_SCREENS_FILES=0 cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
//! ```
//!
//! Every run measures the empty bands of each screen and ends with an error when a screen
//! has one, or when a screen does not settle (ADR 0730, ADR 0731). With
//! `JUNTO_SCREENS_FILES=0` it writes no file and only measures.

// The program is a tool of the developers that writes the files of the screens that it
// draws; the refusals of the file system in `clippy.toml` are for the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, HeadlessAppContext, px, size};
use gpui_base::Root;
use gpui_base::input::InputState;
use junto_client::app::{self, WindowStart};
use junto_client::assets::Assets;
use junto_client::shell::call::CallTab;
use junto_client::shell::share::SoundKind;
use junto_client::shell::{Open, SettingsSection, Shell};
use junto_client::state::AppState;
use junto_client::state::fixtures::from_roles;
use junto_client::state::view::{EntryState, MediaSnapshot, OwnEntry, ServerSnapshot, ShownLevels};
use junto_client::theme;
use junto_client_core::api::{
    HttpRequest, HttpResponse, Transport, TransportError, TransportErrorKind, TransportFuture,
};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessState, Notice};
use junto_client_core::live::{StreamFault, StreamStatus};
use junto_client_core::model::{Loadable, ServerModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::account::{OwnAccount, RecoveryNotice};
use junto_client_core::protocol::call::{
    BlockReason, Call, CallInvitation, CallLock, CallParticipant, CallPlace, ChannelCall,
    EntryRequest, LeaveReason, Meeting, MicrophoneBlock, ScreenShare, ShareAudio,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::Cursor;
use junto_client_core::protocol::ids::{AccountId, ChannelId, WorkspaceId};
use junto_client_core::protocol::recording::{
    Recording, RecordingFailure, RecordingState, RecordingStop, RecordingStopReason,
};
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::server::ServerInfo;
use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
use junto_client_core::protocol::structure::{
    Category, Channel, ChannelKind, Workspace, WorkspaceMember,
};
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::secret::SecretBytes;
use junto_client_core::settings::ThemeChoice;
use junto_client_core::version::{Incompatibility, ServerVersion, SupportedVersions};
use junto_client_core::voice::{InputMode, VoiceSettings};
use junto_client_core::{Client, ClientOptions, Reach};
use junto_media::audio::synthetic::SyntheticHost;
use junto_media::audio::{AudioError, DeviceId, DeviceInfo, Devices};
use junto_media::calls::{
    ActiveCall, CallTarget, CallTiming, CallsView, EndReason, MediaOptions, Outcome, Phase,
};
use junto_media::engine::{
    MicrophoneState, OutputState, SeenShare, ShareId, ShareInfo, ShareState,
};
use junto_media::permission::{FixedPermission, MicrophoneAccess};
use junto_media::share::picture::PictureFrame;
use junto_media::share::synthetic::SyntheticCapture;
use junto_media::share::{
    ApplicationId, ApplicationSource, CaptureError, DisplaySource, NoCapture, PictureSource,
    ScreenAccess, ScreenCapture, ShareKind, ShareSources, WindowSource,
};
use time::OffsetDateTime;

/// The window of the design: 1440 by 900 pixels at 100 %.
#[path = "junto-screens/account.rs"]
mod account;
#[path = "junto-screens/admin.rs"]
mod admin;
#[path = "junto-screens/book.rs"]
mod book;
#[path = "junto-screens/cameras.rs"]
mod cameras;
#[path = "junto-screens/direct.rs"]
mod direct;
#[path = "junto-screens/discovery.rs"]
mod discovery;
#[path = "junto-screens/frames.rs"]
mod frames;
#[path = "junto-screens/gaps.rs"]
mod gaps;
#[path = "junto-screens/logo.rs"]
mod logo;
#[path = "junto-screens/messages.rs"]
mod messages;
#[path = "junto-screens/people.rs"]
mod people;
#[path = "junto-screens/recorded.rs"]
mod recorded;
#[path = "junto-screens/roles.rs"]
mod roles;
#[path = "junto-screens/threads.rs"]
mod threads;

const WINDOW: (f32, f32) = (1440.0, 900.0);

/// The most frames that a screen is drawn before its areas stay the same.
const SETTLE_FRAMES: usize = 8;
/// The longest time that the drawing of one screen takes before the program ends with an
/// error.
const SCREEN_TIME: Duration = Duration::from_secs(120);

/// Ends the program with the name of the screen when the drawing of one screen takes
/// longer than [`SCREEN_TIME`]: a task that never ends keeps `run_until_parked` from
/// returning, and the program would draw without an end.
fn watch_the_drawing(drawing: Arc<std::sync::Mutex<Option<(&'static str, Instant)>>>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let late = drawing
                .lock()
                .ok()
                .and_then(|drawing| *drawing)
                .filter(|(_, started)| started.elapsed() > SCREEN_TIME);
            if let Some((name, _)) = late {
                eprintln!(
                    "junto-screens: the screen {name} did not settle within {} seconds",
                    SCREEN_TIME.as_secs()
                );
                std::process::exit(1);
            }
        }
    });
}

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
        avatar: None,
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
        permissions: Vec::new(),
        channel_permissions: Vec::new(),
        notification_default: None,
    }
}

/// The workspace "Studio" of the design: four text channels, and three voice channels of
/// which one is recorded, one is locked and one hides who is inside.
fn studio() -> WorkspaceState {
    let mut studio = workspace(10, "Studio", true);
    let workspace_id = studio.workspace.id;
    let role = |number, name: &str, position| WorkspaceRole {
        position,
        id: id(number),
        workspace_id,
        name: name.to_owned(),
        permissions: Vec::new(),
        is_everyone: position == 0,
    };
    // The permissions that the "everyone" role of a new workspace has.
    let mut everyone = role(20, "everyone", 0);
    everyone.permissions = vec![
        WorkspacePermission::ViewChannel,
        WorkspacePermission::LockRoom,
        WorkspacePermission::RunMeetingMode,
    ];
    studio.roles = vec![everyone, role(21, "Leads", 2), role(22, "Engineers", 1)];
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
                hides_occupants: false,
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
                    bytes: None,
                    timeline_started_at: None,
                    transcript: TranscriptState::None,
                    transcript_lines: 0,
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
                hides_occupants: false,
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
    }
}

/// The state of "Acme Studio" as Luca sees it, with the members of "Studio" loaded and Ana
/// speaking.
fn acme_model(change: impl FnOnce(&mut WorkspaceState)) -> Arc<ServerModel> {
    acme_model_of(change, |_| {})
}

/// [`acme_model`] with a change of the snapshot of Luca beyond the workspace.
fn acme_model_of(
    change: impl FnOnce(&mut WorkspaceState),
    more: impl FnOnce(&mut Snapshot),
) -> Arc<ServerModel> {
    let mut studio = studio();
    change(&mut studio);
    let mut stated_snapshot = snapshot(
        "Luca",
        vec![
            studio,
            workspace(11, "Client Alpha", false),
            workspace(12, "Client Beta", false),
        ],
    );
    more(&mut stated_snapshot);
    let mut model = ServerModel::from_snapshot(from_roles(stated_snapshot));
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    let roles: Vec<_> = studio.roles.iter().map(|role| role.id).collect();
    studio.members = Loadable::Loaded(
        PEOPLE
            .iter()
            .enumerate()
            .map(|(index, name)| WorkspaceMember {
                avatar: None,
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
        server_version: ServerVersion::parse("0.3.1"),
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
        model: Some(Arc::new(ServerModel::from_snapshot(from_roles(snapshot(
            "Luca",
            vec![workspace(13, "Lounge", true)],
        ))))),
        server_version: ServerVersion::parse("0.3.0"),
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
        server_version: None,
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
                receipt: junto_client_core::api::Receipt::NotStated,
            })
        })
    }
}

struct Studio {
    cx: HeadlessAppContext,
    out: PathBuf,
    scratch: PathBuf,
    drawn: usize,
    /// The theme of every screen (`JUNTO_SCREENS_THEME`: `light`, the default, or `dark`).
    theme: ThemeChoice,
    /// The empty bands of the screens drawn so far ([`gaps::empty_bands`]).
    bands: Vec<String>,
    /// The screens are written as files; `JUNTO_SCREENS_FILES=0` only measures them.
    files: bool,
    /// The screen that is drawn and when its drawing started, for the watch of
    /// [`watch_the_drawing`].
    drawing: Arc<std::sync::Mutex<Option<(&'static str, Instant)>>>,
}

/// One screen: the servers it shows, the state of the calls and the interface scale.
struct Screen {
    size: (f32, f32),
    name: &'static str,
    servers: Vec<ServerSnapshot>,
    media: Option<MediaSnapshot>,
    scale: u32,
    /// The name of the profile preset. Empty: the first start.
    preset: &'static str,
    /// The screen capture of the screen. It exists only in the program.
    capture: Arc<dyn ScreenCapture>,
}

impl Screen {
    fn new(name: &'static str, servers: Vec<ServerSnapshot>) -> Self {
        Self {
            size: WINDOW,
            name,
            servers,
            media: None,
            scale: 100,
            preset: "Luca",
            capture: Arc::new(capture()),
        }
    }

    fn capture(mut self, capture: Arc<dyn ScreenCapture>) -> Self {
        self.capture = capture;
        self
    }

    /// The screen of a call: "Acme Studio" with Luca in Daily, and this state of the
    /// calls.
    fn call(name: &'static str, media: MediaSnapshot) -> Self {
        Self {
            media: Some(media),
            ..Self::new(name, vec![acme_with_luca(|_| {}), harbor()])
        }
    }

    fn servers(mut self, servers: Vec<ServerSnapshot>) -> Self {
        self.servers = servers;
        self
    }

    fn size(mut self, width: f32, height: f32) -> Self {
        self.size = (width, height);
        self
    }

    fn scale(mut self, scale: u32) -> Self {
        self.scale = scale;
        self
    }

    fn media(mut self, media: MediaSnapshot) -> Self {
        self.media = Some(media);
        self
    }

    fn preset(mut self, preset: &'static str) -> Self {
        self.preset = preset;
        self
    }
}

type Prepare = Box<dyn FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>)>;

/// An open window with its shell and the state of the application.
type Opened = (gpui::WindowHandle<Root>, Entity<Shell>, Entity<AppState>);

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
        // The screens show the emoji as the fonts of the system draw them (ADR 0758).
        cx.update(junto_client::emoji::measure_now);
        let theme = match std::env::var("JUNTO_SCREENS_THEME").as_deref() {
            Ok("dark") => ThemeChoice::Dark,
            Ok("light") | Err(_) => ThemeChoice::Light,
            Ok(other) => return Err(format!("JUNTO_SCREENS_THEME is light or dark, not {other}")),
        };
        let drawing = Arc::new(std::sync::Mutex::new(None));
        watch_the_drawing(drawing.clone());
        Ok(Self {
            cx,
            out,
            scratch,
            drawn: 0,
            theme,
            bands: Vec::new(),
            files: std::env::var("JUNTO_SCREENS_FILES").as_deref() != Ok("0"),
            drawing,
        })
    }

    /// Notes the screen that is drawn from now on.
    fn drawing(&self, name: Option<&'static str>) {
        if let Ok(mut drawing) = self.drawing.lock() {
            *drawing = name.map(|name| (name, Instant::now()));
        }
    }

    /// Draws one screen. `prepare` opens what the screen shows beyond its servers.
    fn draw(
        &mut self,
        screen: Screen,
        prepare: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
    ) -> Result<(), String> {
        self.draw_after(screen, Box::new(prepare), |_| true)
    }

    /// Like [`Studio::draw`], and runs `answer` after the screen was drawn once, as an
    /// answer of the server that arrives while the screen is shown.
    fn draw_answered(
        &mut self,
        screen: Screen,
        prepare: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
        answer: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
    ) -> Result<(), String> {
        self.draw_with(screen, Box::new(prepare), |_| true, Some(Box::new(answer)))
    }

    /// Like [`Studio::draw`], and waits until `ready` holds before the screen is captured.
    fn draw_after(
        &mut self,
        screen: Screen,
        prepare: Prepare,
        ready: impl Fn(&Shell) -> bool,
    ) -> Result<(), String> {
        self.draw_with(screen, prepare, ready, None)
    }

    fn draw_with(
        &mut self,
        screen: Screen,
        prepare: Prepare,
        ready: impl Fn(&Shell) -> bool,
        answer: Option<Prepare>,
    ) -> Result<(), String> {
        let name = screen.name;
        let (width, scale) = (screen.size.0, screen.scale);
        self.drawing(Some(name));
        let (window, shell, _) = self.open(screen, prepare)?;
        let started = Instant::now();
        loop {
            self.cx.run_until_parked();
            if self.cx.update(|cx| ready(shell.read(cx))) {
                break;
            }
            if started.elapsed() > Duration::from_secs(10) {
                return Err(format!("{name}: the screen did not reach its state"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(answer) = answer {
            for _ in 0..2 {
                self.cx
                    .update_window(window.into(), |_, window, _| window.refresh())
                    .map_err(|error| format!("{error:#}"))?;
                self.cx.run_until_parked();
            }
            self.cx
                .update_window(window.into(), |_, window, cx| {
                    shell.update(cx, |shell, cx| answer(shell, window, cx));
                })
                .map_err(|error| format!("{error:#}"))?;
            self.cx.run_until_parked();
        }
        // The first frame lays the layers out, the second draws what they opened, and a
        // part that takes the height of a late content draws it in the frame after its
        // measure (ADR 0730). The screen is drawn until two frames lay out the same areas.
        let mut areas = None;
        let mut settled = false;
        for _ in 0..SETTLE_FRAMES {
            junto_client::ui::record_areas();
            self.cx
                .update_window(window.into(), |_, window, _| window.refresh())
                .map_err(|error| format!("{error:#}"))?;
            self.cx.run_until_parked();
            let drawn = junto_client::ui::take_areas();
            if areas.as_ref() == Some(&drawn) {
                settled = true;
                break;
            }
            areas = Some(drawn);
        }
        let areas = areas.unwrap_or_default();
        if !settled {
            return Err(format!(
                "{name}: the screen did not settle: its areas changed in each of \
                 {SETTLE_FRAMES} frames"
            ));
        }
        let image = self
            .cx
            .capture_screenshot(window.into())
            .map_err(|error| format!("{name}: {error:#}"))?;
        self.bands.extend(
            gaps::empty_bands(&image, &areas, width, scale)
                .into_iter()
                .map(|band| format!("{name}: {band}")),
        );
        if self.files {
            let file = self.out.join(format!("{name}.png"));
            image.save(&file).map_err(|error| error.to_string())?;
            println!("{}", file.display());
        } else {
            println!("{name}");
        }
        self.cx
            .update_window(window.into(), |_, window, _| window.remove_window())
            .map_err(|error| format!("{error:#}"))?;
        self.cx.run_until_parked();
        self.drawing(None);
        Ok(())
    }

    /// Opens the window of one screen on a data directory of its own.
    fn open(&mut self, screen: Screen, prepare: Prepare) -> Result<Opened, String> {
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
        let (servers, media) = (screen.servers, screen.media);
        // The devices of a screen exist only in the program: the tool opens no microphone,
        // plays no sound and captures no screen.
        let devices = MediaOptions {
            camera: Arc::new(junto_media::camera::synthetic::SyntheticCamera::new()),
            host: Arc::new(SyntheticHost::with_tone(440.0)),
            permission: Arc::new(FixedPermission::allowed()),
            capture: screen.capture,
            timing: CallTiming::default(),
        };
        let chosen = self.theme;
        let state: Entity<AppState> = self.cx.update(|cx| {
            let state = app::state_with_media(client, start, devices, cx);
            state.update(cx, |state, cx| state.set_theme(chosen, cx));
            if !servers.is_empty() {
                state.update(cx, |state, cx| state.show_fixture(servers, cx));
            }
            if let Some(media) = media {
                state.update(cx, |state, cx| state.show_media_fixture(media, cx));
            }
            state
        });
        let scale = screen.scale;
        let held = state.clone();
        let built: std::rc::Rc<std::cell::RefCell<Option<Entity<Shell>>>> = std::rc::Rc::default();
        let slot = built.clone();
        let window = self
            .cx
            .open_window(
                size(px(screen.size.0), px(screen.size.1)),
                move |window, cx| {
                    window.set_rem_size(theme::rem_size(scale));
                    let shell = cx.new(|cx| {
                        let mut shell = Shell::new(state, window, cx);
                        prepare(&mut shell, window, cx);
                        shell
                    });
                    *slot.borrow_mut() = Some(shell.clone());
                    cx.new(|cx| Root::new(shell, window, cx))
                },
            )
            .map_err(|error| format!("{error:#}"))?;
        let shell = built
            .borrow_mut()
            .take()
            .ok_or("the window was not built")?;
        Ok((window, shell, held))
    }
}

fn two_servers() -> Vec<ServerSnapshot> {
    vec![acme(), harbor()]
}

/// The call of Daily on "Acme Studio" as the place of the call of this device.
fn daily() -> CallTarget {
    CallTarget {
        server_id: id(1),
        place: CallPlace::Channel {
            workspace_id: id(10),
            channel_id: id(34),
        },
    }
}

/// "Acme Studio" with Luca in the call of Daily. `change` changes that call.
fn acme_with_luca(change: impl Fn(&mut Call)) -> ServerSnapshot {
    let mut server = acme();
    server.model = Some(acme_model(|studio| {
        for shown in &mut studio.calls {
            if let Some(call) = &mut shown.call
                && shown.channel_id == id::<ChannelId>(34)
            {
                // The design shows no share in a call without a share view.
                for participant in &mut call.participants {
                    participant.screen_share = None;
                }
                call.participants.insert(3, participant("Luca"));
                call.recording = None;
                change(call);
            }
        }
    }));
    server
}

fn device(id: &str, name: &str, is_default: bool) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id.to_owned()),
        name: name.to_owned(),
        is_default,
    }
}

/// The state of the calls while this device is in the call of Daily.
fn in_call(phase: Phase) -> MediaSnapshot {
    let mut voice = VoiceSettings::default();
    voice.set_volume(id(1), person("Bruno"), 140);
    voice.set_volume(id(1), person("Marta"), 60);
    MediaSnapshot {
        calls: CallsView {
            muted: false,
            deafened: false,
            joining: None,
            call: Some(ActiveCall {
                camera: junto_media::camera::CameraState::Off,
                cameras: Vec::new(),
                target: daily(),
                identity: "seat-Luca".to_owned(),
                phase,
                interrupted_at: (phase == Phase::Reconnecting)
                    .then(|| at() + time::Duration::minutes(37)),
                microphone: if phase == Phase::Connected {
                    MicrophoneState::Running
                } else {
                    MicrophoneState::Closed
                },
                access: MicrophoneAccess::Allowed,
                input_device: Some(device("desk", "Desk microphone", false)),
                output: OutputState::Running,
                output_device: Some(device("phones", "Headphones", false)),
                microphone_block: None,
                sound_block: None,
                share: ShareState::Off,
                shares: Vec::new(),
                watched: None,
            }),
            outcome: None,
            unused: None,
            closed: None,
        },
        levels: ShownLevels {
            input: 5,
            own: (0, false),
            people: [(person("Ana"), (4, true)), (person("Carla"), (1, false))]
                .into_iter()
                .collect(),
        },
        voice: VoiceSettings {
            input_device: Some("desk".to_owned()),
            output_device: Some("phones".to_owned()),
            ..voice
        },
        devices: Devices {
            inputs: vec![
                device("built-in", "Built-in microphone", true),
                device("desk", "Desk microphone", false),
            ],
            outputs: vec![
                device("speakers", "Built-in speakers", true),
                device("phones", "Headphones", false),
            ],
        },
        access: Some(MicrophoneAccess::Allowed),
        now: Some(at() + time::Duration::seconds(42 * 60 + 17)),
        can_share: true,
    }
}

/// The state of the calls of a person who is in no call.
fn no_call() -> MediaSnapshot {
    MediaSnapshot {
        access: Some(MicrophoneAccess::Allowed),
        can_share: true,
        ..MediaSnapshot::default()
    }
}

/// What the capture of a screen lists: one screen, two windows and their applications.
fn capture() -> SyntheticCapture {
    let application = |process_id: i32, bundle_id: &str| ApplicationId {
        process_id,
        bundle_id: bundle_id.to_owned(),
    };
    let (desktop, editor) = (
        application(501, "example.remote-desktop"),
        application(502, "example.editor"),
    );
    let capture = SyntheticCapture::new();
    capture.set_sources(ShareSources {
        portal: Vec::new(),
        system_sound: true,
        started_the_client: Vec::new(),
        creators_unknown: false,
        displays: vec![DisplaySource {
            id: 1,
            width: 1920,
            height: 1200,
        }],
        windows: vec![
            WindowSource {
                id: 11,
                title: "alpha-review-vm".to_owned(),
                application: desktop.clone(),
                application_name: "Remote Desktop".to_owned(),
                width: 1440,
                height: 900,
            },
            WindowSource {
                id: 12,
                title: "export.rs".to_owned(),
                application: editor.clone(),
                application_name: "Editor".to_owned(),
                width: 1280,
                height: 800,
            },
        ],
        applications: vec![
            ApplicationSource {
                id: editor,
                name: "Editor".to_owned(),
            },
            ApplicationSource {
                id: desktop,
                name: "Remote Desktop".to_owned(),
            },
        ],
    });
    capture
}

/// A picture that the program draws: rectangles of one colour each on a ground.
struct Canvas {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32, ground: u32) -> Self {
        let mut canvas = Self {
            width,
            height,
            bgra: vec![0; (width * height * 4) as usize],
        };
        canvas.fill((0, 0, width, height), ground);
        canvas
    }

    /// Fills the rectangle `(x, y, width, height)` with a colour given as 0xRRGGBB.
    fn fill(&mut self, at: (u32, u32, u32, u32), colour: u32) {
        let [_, red, green, blue] = colour.to_be_bytes();
        for y in at.1..(at.1 + at.3).min(self.height) {
            for x in at.0..(at.0 + at.2).min(self.width) {
                let index = ((y * self.width + x) * 4) as usize;
                self.bgra[index..index + 4].copy_from_slice(&[blue, green, red, 255]);
            }
        }
    }

    fn frame(self) -> PictureFrame {
        PictureFrame {
            width: self.width,
            height: self.height,
            bgra: self.bgra,
            source: (self.width, self.height),
        }
    }
}

/// The picture of a shared screen: a desktop with one window that holds four panels.
fn desktop_picture(width: u32, height: u32) -> PictureFrame {
    let mut canvas = Canvas::new(width, height, 0x38_4450);
    let unit = |share: u32, of: u32| of * share / 100;
    let (w, h) = (width, height);
    // The bar at the bottom of the desktop.
    canvas.fill((0, unit(94, h), w, unit(6, h)), 0x20_2830);
    for index in 0..6 {
        canvas.fill(
            (
                unit(2, w) + index * unit(3, w),
                unit(96, h),
                unit(2, w),
                unit(3, h),
            ),
            if index == 0 { 0x5B_9B_D5 } else { 0x4A_56_62 },
        );
    }
    // The window with its title and its four panels.
    let (left, top, wide, high) = (unit(7, w), unit(5, h), unit(90, w), unit(84, h));
    canvas.fill((left, top, wide, high), 0x16_1C_22);
    canvas.fill((left, top, wide, unit(4, h)), 0x10_15_1A);
    canvas.fill(
        (left + unit(1, w), top + unit(1, h), unit(1, w), unit(2, h)),
        0x5B_9B_D5,
    );
    let (panel_w, panel_h) = (unit(44, w), unit(33, h));
    for (index, (column, row)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        let x = left + unit(1, w) + column * (panel_w + unit(1, w));
        let y = top + unit(9, h) + row * (panel_h + unit(1, h));
        canvas.fill((x, y, panel_w, panel_h), 0x46_52_5E);
        if index == 0 {
            // The frame of the person who speaks in the shared call.
            canvas.fill((x, y, panel_w, 3), 0x5B_9B_D5);
            canvas.fill((x, y + panel_h - 3, panel_w, 3), 0x5B_9B_D5);
            canvas.fill((x, y, 3, panel_h), 0x5B_9B_D5);
            canvas.fill((x + panel_w - 3, y, 3, panel_h), 0x5B_9B_D5);
        }
        // A person: a head and shoulders.
        canvas.fill(
            (
                x + panel_w / 2 - unit(3, w),
                y + unit(7, h),
                unit(6, w),
                unit(9, h),
            ),
            0x9F_AC_B8,
        );
        canvas.fill(
            (
                x + panel_w / 2 - unit(7, w),
                y + unit(18, h),
                unit(14, w),
                panel_h - unit(18, h),
            ),
            0x9F_AC_B8,
        );
        canvas.fill(
            (
                x + unit(1, w),
                y + panel_h - unit(5, h),
                unit(5, w),
                unit(3, h),
            ),
            0x12_17_1C,
        );
    }
    canvas.fill(
        (
            left + wide / 2 - unit(2, w),
            top + high - unit(5, h),
            unit(5, w),
            unit(3, h),
        ),
        0xC0_45_3A,
    );
    canvas.frame()
}

/// The picture of a shared window: an editor with lines of text.
fn editor_picture(width: u32, height: u32) -> PictureFrame {
    let mut canvas = Canvas::new(width, height, 0x1E_22_28);
    let unit = |share: u32, of: u32| of * share / 100;
    let (w, h) = (width, height);
    canvas.fill((0, 0, w, unit(5, h)), 0x14_17_1B);
    canvas.fill((0, unit(5, h), unit(18, w), h), 0x19_1C_21);
    for line in 0..9 {
        canvas.fill(
            (
                unit(2, w),
                unit(9, h) + line * unit(5, h),
                unit(8 + (line * 5) % 7, w),
                unit(1, h) + 2,
            ),
            0x6A_74_80,
        );
    }
    let colours = [0xC5_86_C0, 0x9C_DC_FE, 0xCE_91_78, 0x6A_99_55, 0xDC_DC_AA];
    for line in 0..17u32 {
        let indent = [0, 3, 3, 6, 6, 6, 3, 0, 0, 3, 6, 6, 3, 3, 0, 0, 3][line as usize];
        let y = unit(9, h) + line * unit(5, h);
        let mut x = unit(21 + indent, w);
        for part in 0..(2 + line % 3) {
            let wide = unit(5 + (line * 7 + part * 11) % 14, w);
            canvas.fill(
                (x, y, wide, unit(1, h) + 3),
                colours[((line + part) % 5) as usize],
            );
            x += wide + unit(1, w);
        }
    }
    canvas.frame()
}

/// "Acme Studio" with Luca in the call of Daily, in which the named people share. Luca
/// holds "record calls" unless `may_record` is false.
fn acme_with_shares(
    sharers: &'static [(&'static str, ShareAudio)],
    may_record: bool,
    change: impl Fn(&mut Call),
) -> ServerSnapshot {
    let mut server = acme();
    server.model = Some(acme_model(|studio| {
        if may_record {
            studio.roles[0]
                .permissions
                .push(WorkspacePermission::RecordCalls);
        }
        for shown in &mut studio.calls {
            if let Some(call) = &mut shown.call
                && shown.channel_id == id::<ChannelId>(34)
            {
                call.participants.insert(3, participant("Luca"));
                call.recording = None;
                for participant in &mut call.participants {
                    participant.screen_share = sharers
                        .iter()
                        .find(|(name, _)| person(name) == participant.account_id)
                        .map(|(_, audio)| ScreenShare {
                            audio: *audio,
                            started_at: at(),
                        });
                }
                change(call);
            }
        }
    }));
    server
}

fn seen(name: &str, kind: ShareKind, size: (u32, u32)) -> SeenShare {
    SeenShare {
        account_id: person(name),
        kind: Some(kind),
        frame_rate: Some(30),
        size: Some(size),
    }
}

/// Draws a picture as the picture of the watched share, and small pictures of the shares.
fn show_pictures(
    main: (ShareId, PictureFrame),
    small: Vec<(ShareId, PictureFrame)>,
) -> impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) {
    move |shell, _, cx| {
        select_channel(shell, "Daily", cx);
        // The design draws a call with the server book as icons.
        shell
            .state()
            .update(cx, |state, cx| state.set_server_book_collapsed(true, cx));
        shell.show_pictures_fixture(Some(main), small, cx);
    }
}

/// A recording of the call of Daily.
fn recording(change: impl FnOnce(&mut Recording)) -> Recording {
    let mut recording = Recording {
        id: id(41),
        workspace_id: id(10),
        channel_id: id(34),
        call_id: id(40),
        state: RecordingState::Recording,
        started_by: person("Ana"),
        requested_at: at() + time::Duration::minutes(3),
        recording_since: Some(at() + time::Duration::seconds(3 * 60 + 15)),
        stop: None,
        ended_at: None,
        failure: None,
        language: "auto".to_owned(),
        segments: 5,
        failed_segments: 0,
        bytes: None,
        timeline_started_at: None,
        transcript: TranscriptState::None,
        transcript_lines: 0,
    };
    change(&mut recording);
    recording
}

/// The shares of a call, the dialog of a share, the settings of a share, and the recording
/// of a call (ADR 0738; views B, E and F of the design).
fn draw_shares(studio: &mut Studio) -> Result<(), String> {
    const CARLA: &[(&str, ShareAudio)] = &[("Carla", ShareAudio::System)];
    const TWO: &[(&str, ShareAudio)] =
        &[("Carla", ShareAudio::System), ("Bruno", ShareAudio::None)];
    const LUCA: &[(&str, ShareAudio)] = &[("Luca", ShareAudio::Application)];
    let screen_size = (1920, 1200);
    let carla = ShareId::Of(person("Carla"));
    let bruno = ShareId::Of(person("Bruno"));
    let watching = |shares: Vec<SeenShare>, watched: ShareId| {
        with_call(in_call(Phase::Connected), move |call| {
            call.shares = shares;
            call.watched = Some(watched);
        })
    };
    let one = || watching(vec![seen("Carla", ShareKind::Screen, screen_size)], carla);
    let recorded = |call: &mut Call| call.recording = Some(recording(|_| {}));

    // View B: one share with its sound, and the recording of the call.
    studio.draw(
        Screen::call("share", one())
            .servers(vec![acme_with_shares(CARLA, true, recorded), harbor()]),
        show_pictures((carla, desktop_picture(1568, 980)), Vec::new()),
    )?;
    studio.draw(
        Screen::call("share-scale-150", one())
            .scale(150)
            .servers(vec![acme_with_shares(CARLA, true, recorded), harbor()]),
        show_pictures((carla, desktop_picture(1568, 980)), Vec::new()),
    )?;
    studio.draw(
        Screen::call("share-picture-arrives", one())
            .servers(vec![acme_with_shares(CARLA, true, |_| {}), harbor()]),
        show_daily,
    )?;
    let mut hidden = one();
    hidden.voice.hide_people_without_video = true;
    studio.draw(
        Screen::call("share-people-without-video-hidden", hidden)
            .servers(vec![acme_with_shares(CARLA, true, recorded), harbor()]),
        show_pictures((carla, desktop_picture(1568, 980)), Vec::new()),
    )?;
    let mut quiet = one();
    quiet.voice.share_volume = 0;
    studio.draw(
        Screen::call("share-sound-off-for-the-viewer", quiet)
            .servers(vec![acme_with_shares(CARLA, true, |_| {}), harbor()]),
        show_pictures((carla, desktop_picture(1568, 980)), Vec::new()),
    )?;

    // View E: two shares. The viewer picks one, and the sound follows the choice.
    let two = |watched: ShareId| {
        watching(
            vec![
                seen("Carla", ShareKind::Screen, screen_size),
                seen("Bruno", ShareKind::Window, (1280, 800)),
            ],
            watched,
        )
    };
    let small = || {
        vec![
            (carla, desktop_picture(176, 110)),
            (bruno, editor_picture(176, 110)),
        ]
    };
    studio.draw(
        Screen::call("share-two", two(carla))
            .servers(vec![acme_with_shares(TWO, true, recorded), harbor()]),
        show_pictures((carla, desktop_picture(1568, 980)), small()),
    )?;
    studio.draw(
        Screen::call("share-two-watching-the-second", two(bruno))
            .servers(vec![acme_with_shares(TWO, true, recorded), harbor()]),
        show_pictures((bruno, editor_picture(1280, 800)), small()),
    )?;

    // The share of this device.
    let own = |sound_held: bool| {
        with_call(in_call(Phase::Connected), move |call| {
            call.share = ShareState::Running(ShareInfo {
                kind: ShareKind::Window,
                sound: ShareAudio::Application,
                sound_held,
                size: (1280, 800),
                frame_rate: 30,
            });
            call.watched = Some(ShareId::Own);
        })
    };
    studio.draw(
        Screen::call("share-own", own(false))
            .servers(vec![acme_with_shares(LUCA, true, |_| {}), harbor()]),
        show_pictures((ShareId::Own, editor_picture(1280, 800)), Vec::new()),
    )?;
    let block = MicrophoneBlock {
        by: person("Ana"),
        reason: BlockReason::Moderator,
    };
    let held = block.clone();
    let blocked = with_call(own(true), move |call| {
        call.microphone_block = Some(block);
        call.microphone = MicrophoneState::Closed;
    });
    studio.draw(
        Screen::call("share-own-sound-held-by-a-microphone-block", blocked).servers(vec![
            acme_with_shares(LUCA, true, move |call| {
                for participant in &mut call.participants {
                    if participant.account_id == person("Luca") {
                        participant.microphone_block = Some(held.clone());
                    }
                }
            }),
            harbor(),
        ]),
        show_pictures((ShareId::Own, editor_picture(1280, 800)), Vec::new()),
    )?;
    let starting = with_call(in_call(Phase::Connected), |call| {
        call.share = ShareState::Starting;
    });
    studio.draw(Screen::call("share-own-starting", starting), show_daily)?;
    let ended = with_call(in_call(Phase::Connected), |call| {
        call.share = ShareState::Ended(CaptureError::SourceGone);
    });
    studio.draw(Screen::call("share-own-ended-by-itself", ended), show_daily)?;

    // The dialog of a share.
    let picker = |prepare: fn(&mut Shell, &mut gpui::Context<Shell>)| -> Prepare {
        Box::new(move |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::SharePicker, window, cx);
            cx.spawn(async move |shell, cx| {
                // The lists arrive after the dialog is open.
                loop {
                    let listed = shell.update(cx, |shell, cx| {
                        let listed = shell.share_picker_is_listed();
                        if listed {
                            prepare(shell, cx);
                        }
                        listed
                    });
                    if !matches!(listed, Ok(false)) {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(5))
                        .await;
                }
            })
            .detach();
        })
    };
    studio.draw_after(
        Screen::call("share-dialog", in_call(Phase::Connected)),
        picker(|_, _| {}),
        Shell::share_picker_is_listed,
    )?;
    studio.draw_after(
        Screen::call(
            "share-dialog-window-with-its-application",
            in_call(Phase::Connected),
        ),
        picker(|shell, cx| {
            shell.pick_share_picture(PictureSource::Window(12), cx);
            shell.pick_share_sound(SoundKind::Application, cx);
        }),
        Shell::share_picker_is_listed,
    )?;
    studio.draw_after(
        Screen::call(
            "share-dialog-screen-with-one-application",
            in_call(Phase::Connected),
        ),
        picker(|shell, cx| {
            shell.pick_share_picture(PictureSource::Display(1), cx);
            shell.pick_share_sound(SoundKind::Application, cx);
            shell.pick_share_application(
                ApplicationId {
                    process_id: 501,
                    bundle_id: "example.remote-desktop".to_owned(),
                },
                cx,
            );
        }),
        Shell::share_picker_is_listed,
    )?;
    studio.draw_after(
        Screen::call(
            "share-dialog-screen-with-system-sound",
            in_call(Phase::Connected),
        ),
        picker(|shell, cx| {
            shell.pick_share_picture(PictureSource::Display(1), cx);
            shell.pick_share_sound(SoundKind::System, cx);
        }),
        Shell::share_picker_is_listed,
    )?;
    let refused = capture();
    refused.set_access(ScreenAccess::NotAllowed);
    studio.draw(
        Screen::call("share-dialog-no-permission", in_call(Phase::Connected))
            .capture(Arc::new(refused)),
        |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::SharePicker, window, cx);
        },
    )?;
    let mut unsupported = in_call(Phase::Connected);
    unsupported.can_share = false;
    studio.draw(
        Screen::call("share-not-on-this-system", unsupported).capture(Arc::new(NoCapture)),
        show_daily,
    )?;

    // The settings of a share and of the view.
    studio.draw(
        Screen::call("call-settings-share-and-view", in_call(Phase::Connected)),
        show_daily_tab(CallTab::Settings),
    )?;

    // The recording of the call (views B and F).
    let recording_screen = |name: &'static str, may_record: bool, change: fn(&mut Call)| {
        Screen::call(name, one())
            .servers(vec![acme_with_shares(CARLA, may_record, change), harbor()])
    };
    let with_picture = || show_pictures((carla, desktop_picture(1568, 980)), Vec::new());
    studio.draw(
        recording_screen("recording-none", true, |_| {}),
        with_picture(),
    )?;
    // "Record" opens the languages of the speech (ADR 0616).
    let picture = with_picture();
    studio.draw(
        recording_screen("recording-languages", true, |_| {}),
        move |shell, window, cx| {
            picture(shell, window, cx);
            shell.open_record_languages(window, cx);
        },
    )?;
    studio.draw(
        recording_screen("recording-starting", true, |call| {
            call.recording = Some(recording(|recording| {
                recording.state = RecordingState::Starting;
                recording.recording_since = None;
            }));
        }),
        with_picture(),
    )?;
    studio.draw(
        recording_screen("recording-stopping", true, |call| {
            call.recording = Some(recording(|recording| {
                recording.stop = Some(RecordingStop {
                    reason: RecordingStopReason::Stopped,
                    by: Some(person("Luca")),
                    at: at() + time::Duration::minutes(42),
                });
            }));
        }),
        with_picture(),
    )?;
    studio.draw(
        recording_screen("recording-seen-without-the-permission", false, |call| {
            call.recording = Some(recording(|_| {}));
        }),
        with_picture(),
    )?;
    studio.draw(
        recording_screen("recording-no-permission", false, |_| {}),
        with_picture(),
    )?;
    let lost = || {
        recording(|recording| {
            recording.state = RecordingState::Processing;
            recording.stop = Some(RecordingStop {
                reason: RecordingStopReason::RecorderLost,
                by: None,
                at: at() + time::Duration::minutes(39),
            });
            recording.ended_at = Some(at() + time::Duration::minutes(39));
        })
    };
    let failed = |problem: Recording, details: bool, request: Option<&'static str>| -> Prepare {
        Box::new(move |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell
                .state()
                .update(cx, |state, cx| state.set_server_book_collapsed(true, cx));
            shell.show_pictures_fixture(
                Some((ShareId::Of(person("Carla")), desktop_picture(1568, 980))),
                Vec::new(),
                cx,
            );
            shell.state().update(cx, |state, cx| {
                state.show_recording_fixture(
                    Some((id(1), problem)),
                    request.map(str::to_owned),
                    cx,
                );
            });
            if details {
                shell.open(Open::RecordingDetails, window, cx);
            }
        })
    };
    studio.draw_after(
        recording_screen("recording-failed", true, |_| {}),
        failed(lost(), false, None),
        |_| true,
    )?;
    studio.draw_after(
        recording_screen("recording-failed-details", true, |_| {}),
        failed(lost(), true, None),
        |_| true,
    )?;
    studio.draw_after(
        recording_screen("recording-stopped-by-the-storage-reserve", true, |_| {}),
        failed(
            recording(|recording| {
                recording.state = RecordingState::Processing;
                recording.stop = Some(RecordingStop {
                    reason: RecordingStopReason::StorageReserve,
                    by: None,
                    at: at() + time::Duration::minutes(39),
                });
            }),
            true,
            None,
        ),
        |_| true,
    )?;
    studio.draw_after(
        recording_screen("recording-not-taken-by-a-recorder", true, |_| {}),
        failed(
            recording(|recording| {
                recording.state = RecordingState::Failed;
                recording.failure = Some(RecordingFailure::RecorderUnavailable);
                recording.recording_since = None;
            }),
            true,
            None,
        ),
        |_| true,
    )?;
    studio.draw_after(
        recording_screen("recording-request-refused", true, |_| {}),
        Box::new(|shell, _, cx| {
            select_channel(shell, "Daily", cx);
            shell.state().update(cx, |state, cx| {
                state.show_recording_fixture(
                    None,
                    Some(
                        "The server has no free place for a further recording. Wait until \
                         a recording of another call has ended."
                            .to_owned(),
                    ),
                    cx,
                );
            });
        }),
        |_| true,
    )?;
    Ok(())
}

/// The call of Daily in the state of a workspace.
fn daily_call(studio: &mut WorkspaceState) -> Option<&mut Call> {
    studio
        .calls
        .iter_mut()
        .find(|shown| shown.channel_id == id::<ChannelId>(34))
        .and_then(|shown| shown.call.as_mut())
}

/// "Acme Studio" with Luca in the call of Daily, nobody sharing and no recording. Luca has
/// the role "Leads", which holds `permissions`. `change` changes the workspace.
fn acme_with_rules(
    permissions: &'static [WorkspacePermission],
    change: impl Fn(&mut WorkspaceState),
) -> ServerSnapshot {
    let mut server = acme();
    server.model = Some(acme_model(|studio| {
        studio.roles[1].permissions = permissions.to_vec();
        studio.own_role_ids = vec![studio.roles[1].id];
        if let Some(call) = daily_call(studio) {
            for participant in &mut call.participants {
                participant.screen_share = None;
            }
            call.participants.insert(3, participant("Luca"));
            call.recording = None;
        }
        change(studio);
    }));
    server
}

/// The rules of the room of a call (ADR 0739): meeting mode as presenter and as
/// participant (view D), a room that is locked or full and the request to enter (view F),
/// the invitation of a member (view B), the menu of a person (view K), the settings of the
/// room, and what the server tells or refuses.
fn draw_rules(studio: &mut Studio) -> Result<(), String> {
    const MEMBER: &[WorkspacePermission] = &[];
    const MODERATOR: &[WorkspacePermission] = &[
        WorkspacePermission::ManageChannel,
        WorkspacePermission::MuteMembers,
        WorkspacePermission::DeafenMembers,
        WorkspacePermission::DisconnectMembers,
        WorkspacePermission::MoveMembers,
    ];
    let share = || ScreenShare {
        audio: ShareAudio::Application,
        started_at: at(),
    };
    // Meeting mode that `presenter` runs: that person shares, and every other microphone
    // has the block of the meeting.
    fn meeting(studio: &mut WorkspaceState, presenter: &str, share: ScreenShare) {
        let Some(call) = daily_call(studio) else {
            return;
        };
        call.meeting = Some(Meeting {
            presenter: person(presenter),
            started_at: at(),
        });
        for participant in &mut call.participants {
            if participant.account_id == person(presenter) {
                participant.screen_share = Some(share.clone());
                participant.muted = false;
            } else {
                participant.microphone_block = Some(MicrophoneBlock {
                    by: person(presenter),
                    reason: BlockReason::Meeting,
                });
            }
        }
    }
    fn of<'a>(call: &'a mut Call, name: &str) -> Option<&'a mut CallParticipant> {
        call.participants
            .iter_mut()
            .find(|participant| participant.account_id == person(name))
    }
    let sharing = || {
        with_call(in_call(Phase::Connected), |call| {
            call.share = ShareState::Running(ShareInfo {
                kind: ShareKind::Window,
                sound: ShareAudio::Application,
                sound_held: false,
                size: (1280, 800),
                frame_rate: 30,
            });
            call.watched = Some(ShareId::Own);
        })
    };
    let own_picture = || show_pictures((ShareId::Own, editor_picture(1280, 800)), Vec::new());

    // View D, as the presenter: Ana raised a hand.
    let presenter = move |allowed: bool| {
        acme_with_rules(MEMBER, move |studio| {
            meeting(studio, "Luca", share());
            if let Some(ana) = daily_call(studio).and_then(|call| of(call, "Ana")) {
                if allowed {
                    ana.microphone_block = None;
                    ana.muted = true;
                } else {
                    ana.hand_raised = true;
                }
            }
        })
    };
    studio.draw(
        Screen::call("meeting-presenter", sharing()).servers(vec![presenter(false), harbor()]),
        own_picture(),
    )?;
    studio.draw(
        Screen::call("meeting-presenter-scale-150", sharing())
            .scale(150)
            .servers(vec![presenter(false), harbor()]),
        own_picture(),
    )?;
    studio.draw(
        Screen::call("meeting-presenter-allowed", sharing())
            .servers(vec![presenter(true), harbor()]),
        own_picture(),
    )?;
    studio.draw(
        Screen::call("meeting-presenter-members", sharing())
            .servers(vec![presenter(false), harbor()]),
        |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::CallTab(CallTab::Members), window, cx);
        },
    )?;
    // A person who shares and has not turned meeting mode on.
    studio.draw(
        Screen::call("meeting-can-start", sharing()).servers(vec![
            acme_with_rules(MEMBER, move |studio| {
                if let Some(luca) = daily_call(studio).and_then(|call| of(call, "Luca")) {
                    luca.screen_share = Some(share());
                }
            }),
            harbor(),
        ]),
        own_picture(),
    )?;

    // View D, as a participant: Carla runs meeting mode.
    let carla = ShareId::Of(person("Carla"));
    let participant_media = |held: bool, muted: bool| {
        let mut media = with_call(in_call(Phase::Connected), move |call| {
            call.shares = vec![seen("Carla", ShareKind::Screen, (1920, 1200))];
            call.watched = Some(carla);
            if held {
                call.microphone_block = Some(MicrophoneBlock {
                    by: person("Carla"),
                    reason: BlockReason::Meeting,
                });
                call.microphone = MicrophoneState::Closed;
            }
        });
        media.calls.muted = muted;
        media
    };
    let participant_server = move |hand: bool, allowed: bool| {
        acme_with_rules(MEMBER, move |studio| {
            meeting(studio, "Carla", share());
            if let Some(luca) = daily_call(studio).and_then(|call| of(call, "Luca")) {
                luca.hand_raised = hand;
                if allowed {
                    luca.microphone_block = None;
                    luca.muted = true;
                }
            }
        })
    };
    let carla_picture = || show_pictures((carla, desktop_picture(1568, 980)), Vec::new());
    studio.draw(
        Screen::call("meeting-participant", participant_media(true, false))
            .servers(vec![participant_server(false, false), harbor()]),
        carla_picture(),
    )?;
    studio.draw(
        Screen::call(
            "meeting-participant-hand-raised",
            participant_media(true, false),
        )
        .servers(vec![participant_server(true, false), harbor()]),
        carla_picture(),
    )?;
    studio.draw(
        Screen::call(
            "meeting-participant-may-speak",
            participant_media(false, true),
        )
        .servers(vec![participant_server(false, true), harbor()]),
        carla_picture(),
    )?;
    // The same state while the person looks at a text channel.
    studio.draw(
        Screen::call(
            "meeting-participant-may-speak-elsewhere",
            participant_media(false, true),
        )
        .servers(vec![participant_server(false, true), harbor()]),
        |_, _, _| {},
    )?;
    // The block of the microphone ended while the person's own setting is "open".
    studio.draw(
        Screen::call("microphone-open-again", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MEMBER, |_| {}), harbor()]),
        |shell, _, cx| {
            select_channel(shell, "Daily", cx);
            shell.state().update(cx, |state, cx| {
                state.show_call_rules_fixture(
                    None,
                    Some(junto_client::text::MICROPHONE_OPEN_AGAIN.to_owned()),
                    None,
                    cx,
                );
            });
        },
    )?;
    // A request that the server refused.
    studio.draw(
        Screen::call("call-request-refused", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MEMBER, |_| {}), harbor()]),
        |shell, _, cx| {
            select_channel(shell, "Daily", cx);
            shell.state().update(cx, |state, cx| {
                state.show_call_rules_fixture(
                    None,
                    None,
                    Some(junto_client::text::refusal(ErrorCode::Forbidden)),
                    cx,
                );
            });
        },
    )?;

    // The room: locked, with its settings, as a member and as a person with
    // "manage channel".
    let locked = |studio: &mut WorkspaceState| {
        for channel in &mut studio.channels {
            if channel.id == id::<ChannelId>(34) {
                channel.user_limit = Some(8);
            }
        }
        if let Some(call) = daily_call(studio) {
            call.lock = Some(CallLock {
                by: person("Luca"),
                at: at(),
            });
        }
    };
    studio.draw(
        Screen::call("room-locked", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MEMBER, locked), harbor()]),
        show_daily,
    )?;
    studio.draw(
        Screen::call("room-settings", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MODERATOR, locked), harbor()]),
        |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::RoomSettings, window, cx);
        },
    )?;
    studio.draw(
        Screen::call(
            "room-settings-without-permissions",
            in_call(Phase::Connected),
        )
        .servers(vec![acme_with_rules(MEMBER, |_| {}), harbor()]),
        |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::RoomSettings, window, cx);
        },
    )?;

    // View B: the invitation of a member. Elisa is invited and has not answered.
    let inviting = || {
        let mut server = acme_with_rules(MEMBER, |_| {});
        if let Some(model) = &mut server.model {
            Arc::make_mut(model).invitations.push(CallInvitation {
                id: id(60),
                call_id: id(40),
                place: daily().place,
                account_id: person("Elisa"),
                invited_by: person("Luca"),
                created_at: at(),
                expires_at: at() + time::Duration::seconds(60),
            });
        }
        server
    };
    studio.draw(
        Screen::call("call-members-invite", in_call(Phase::Connected))
            .servers(vec![inviting(), harbor()]),
        show_daily_tab(CallTab::Members),
    )?;
    studio.draw(
        Screen::call("call-members-invite-picker", in_call(Phase::Connected))
            .servers(vec![inviting(), harbor()]),
        |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::MemberPicker, window, cx);
        },
    )?;
    // A person outside asks to enter the call of this device.
    let asked = || {
        let mut server = acme_with_rules(MEMBER, locked);
        if let Some(model) = &mut server.model {
            Arc::make_mut(model).entry_requests.push(EntryRequest {
                id: id(61),
                call_id: id(40),
                place: daily().place,
                account_id: person("Diego"),
                created_at: at(),
                expires_at: at() + time::Duration::seconds(60),
            });
        }
        server
    };
    studio.draw(
        Screen::call("call-entry-request", in_call(Phase::Connected))
            .servers(vec![asked(), harbor()]),
        show_daily,
    )?;

    // View F, locked: a person outside chooses the locked room. Another room is full.
    let outside = || {
        let mut server = acme();
        server.model = Some(acme_model(|studio| {
            let workspace_id = studio.workspace.id;
            let mut all_hands = channel(37, workspace_id, ChannelKind::Voice, "All hands");
            all_hands.user_limit = Some(2);
            let place = CallPlace::Channel {
                workspace_id,
                channel_id: all_hands.id,
            };
            let mut moved = Vec::new();
            if let Some(call) = daily_call(studio) {
                call.recording = None;
                let (stay, go) = std::mem::take(&mut call.participants)
                    .into_iter()
                    .partition(|participant| {
                        participant.account_id != person("Bruno")
                            && participant.account_id != person("Marta")
                    });
                call.participants = stay;
                moved = go;
            }
            studio.calls.push(ChannelCall {
                workspace_id,
                channel_id: all_hands.id,
                in_use: true,
                call: Some(Call {
                    id: id(43),
                    place,
                    started_at: Some(at()),
                    lock: None,
                    meeting: None,
                    hides_occupants: false,
                    participants: moved,
                    recording: None,
                }),
            });
            studio.channels.insert(6, all_hands);
        }));
        server
    };
    let focus_entry = |state: Option<EntryState>| {
        move |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            select_channel(shell, "Focus room", cx);
            if let Some(state) = state {
                shell.state().update(cx, |app, cx| {
                    app.show_call_rules_fixture(
                        Some(OwnEntry {
                            server_id: id(1),
                            channel_id: id(35),
                            request_id: Some(id(62)),
                            state,
                        }),
                        None,
                        None,
                        cx,
                    );
                });
            }
        }
    };
    let out = |name: &'static str| Screen::new(name, vec![outside(), harbor()]).media(no_call());
    studio.draw(out("room-locked-outside"), focus_entry(None))?;
    studio.draw(
        out("room-locked-request-waits"),
        focus_entry(Some(EntryState::Waiting)),
    )?;
    studio.draw(
        out("room-locked-request-denied"),
        focus_entry(Some(EntryState::Denied)),
    )?;
    studio.draw(
        out("room-locked-request-expired"),
        focus_entry(Some(EntryState::Expired)),
    )?;
    studio.draw(out("room-full-outside"), |shell, _, cx| {
        select_channel(shell, "All hands", cx);
    })?;
    // An invitation that waits for the person.
    let invited = || {
        let mut server = outside();
        if let Some(model) = &mut server.model {
            Arc::make_mut(model).invitations.push(CallInvitation {
                id: id(63),
                call_id: id(40),
                place: daily().place,
                account_id: person("Luca"),
                invited_by: person("Carla"),
                created_at: at(),
                expires_at: at() + time::Duration::seconds(60),
            });
        }
        server
    };
    studio.draw(
        Screen::new("call-invitation", vec![invited(), harbor()]).media(no_call()),
        |_, _, _| {},
    )?;
    // An invitation into the Focus room while the person is in the call of Daily: the card
    // lies over the view of the call.
    let mut in_daily = acme_with_luca(|_| {});
    if let Some(model) = &mut in_daily.model {
        Arc::make_mut(model).invitations.push(CallInvitation {
            id: id(64),
            call_id: id(41),
            place: CallPlace::Channel {
                workspace_id: id(10),
                channel_id: id(35),
            },
            account_id: person("Luca"),
            invited_by: person("Carla"),
            created_at: at(),
            expires_at: at() + time::Duration::seconds(60),
        });
    }
    studio.draw(
        Screen::call("call-invitation-in-call", in_call(Phase::Connected))
            .servers(vec![in_daily, harbor()]),
        |shell, _, cx| select_channel(shell, "Daily", cx),
    )?;
    // The notices of the server in the panel of the call, in place of its tab.
    studio.draw(
        Screen::call("notices-in-call", in_call(Phase::Connected)),
        |shell, _, cx| {
            select_channel(shell, "Daily", cx);
            shell.toggle_notices(cx);
        },
    )?;

    // View K: the menu of a person in a voice channel, as a moderator and as a member.
    let menu_of_diego = |moving: bool| {
        move |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            select_channel(shell, "Daily", cx);
            shell.open_person_menu(id(35), person("Diego"), window, cx);
            if moving {
                shell.show_move_destinations(cx);
            }
        }
    };
    let ready = |shell: &Shell| shell.person_menu_of().is_some();
    studio.draw_after(
        Screen::call("person-menu", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MODERATOR, |_| {}), harbor()]),
        Box::new(menu_of_diego(false)),
        ready,
    )?;
    studio.draw_after(
        Screen::call("person-menu-move", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MODERATOR, |_| {}), harbor()]),
        Box::new(menu_of_diego(true)),
        ready,
    )?;
    studio.draw_after(
        Screen::call("person-menu-member", in_call(Phase::Connected))
            .servers(vec![acme_with_rules(MEMBER, |_| {}), harbor()]),
        Box::new(menu_of_diego(false)),
        ready,
    )?;
    Ok(())
}

fn with_call(mut media: MediaSnapshot, change: impl FnOnce(&mut ActiveCall)) -> MediaSnapshot {
    if let Some(call) = &mut media.calls.call {
        change(call);
    }
    media
}

fn show_daily(shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>) {
    select_channel(shell, "Daily", cx);
}

fn show_daily_tab(
    tab: CallTab,
) -> impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) {
    move |shell, window, cx| {
        select_channel(shell, "Daily", cx);
        shell.open(Open::CallTab(tab), window, cx);
    }
}

/// The views and the states of a voice call (ADR 0735).
fn draw_calls(studio: &mut Studio) -> Result<(), String> {
    studio.draw(Screen::call("call", in_call(Phase::Connected)), show_daily)?;
    studio.draw(
        Screen::call("call-members", in_call(Phase::Connected)),
        show_daily_tab(CallTab::Members),
    )?;
    studio.draw(
        Screen::call("call-settings", in_call(Phase::Connected)),
        show_daily_tab(CallTab::Settings),
    )?;
    studio.draw(
        Screen::call("call-settings-input-devices", in_call(Phase::Connected)),
        |shell, window, cx| {
            select_channel(shell, "Daily", cx);
            shell.open(Open::InputDevices, window, cx);
        },
    )?;
    let mut muted = in_call(Phase::Connected);
    muted.calls.muted = true;
    muted.calls.deafened = true;
    studio.draw(
        Screen::call("call-own-microphone-and-sound-off", muted),
        show_daily_tab(CallTab::Members),
    )?;
    studio.draw(
        Screen::call("call-reconnecting", in_call(Phase::Reconnecting)),
        show_daily,
    )?;
    studio.draw(
        Screen::call("call-connecting", in_call(Phase::Connecting)),
        show_daily,
    )?;
    let held = |reason: BlockReason, by: &'static str| {
        let block = MicrophoneBlock {
            by: person(by),
            reason,
        };
        let shown = block.clone();
        let media = with_call(in_call(Phase::Connected), |call| {
            call.microphone_block = Some(block);
            call.microphone = MicrophoneState::Closed;
        });
        let servers = vec![
            acme_with_luca(move |call| {
                for participant in &mut call.participants {
                    if participant.account_id == person("Luca") {
                        participant.microphone_block = Some(shown.clone());
                    }
                }
            }),
            harbor(),
        ];
        (media, servers)
    };
    let (media, servers) = held(BlockReason::Moderator, "Ana");
    studio.draw(
        Screen::call("call-microphone-closed-by-a-moderator", media).servers(servers),
        show_daily_tab(CallTab::Members),
    )?;
    let (media, servers) = held(BlockReason::Meeting, "Carla");
    studio.draw(
        Screen::call("call-microphone-closed-by-meeting-mode", media).servers(servers),
        show_daily,
    )?;
    let refused = with_call(in_call(Phase::Connected), |call| {
        call.microphone = MicrophoneState::NoPermission;
        call.access = MicrophoneAccess::Refused;
        call.input_device = None;
    });
    studio.draw(
        Screen::call("call-no-microphone-access", refused),
        show_daily_tab(CallTab::Settings),
    )?;
    let asking = with_call(in_call(Phase::Connected), |call| {
        call.microphone = MicrophoneState::Asking;
        call.access = MicrophoneAccess::NotAsked;
        call.input_device = None;
    });
    studio.draw(
        Screen::call("call-microphone-access-not-asked", asking),
        show_daily_tab(CallTab::Settings),
    )?;
    let failed = with_call(in_call(Phase::Connected), |call| {
        call.microphone = MicrophoneState::Failed(AudioError::NoDevice);
        call.input_device = None;
    });
    studio.draw(Screen::call("call-microphone-failed", failed), show_daily)?;
    // The person is the first in the call.
    let mut alone = acme_with_luca(|call| {
        call.participants
            .retain(|participant| participant.account_id == person("Luca"));
    });
    alone.error = None;
    let mut quiet = in_call(Phase::Connected);
    quiet.levels.people.clear();
    studio.draw(
        Screen::call("call-alone", quiet).servers(vec![alone, harbor()]),
        show_daily,
    )?;
    // The call goes on while the window shows another channel, and another server.
    studio.draw(
        Screen::call(
            "call-while-a-text-channel-is-shown",
            in_call(Phase::Connected),
        ),
        |_, _, _| {},
    )?;
    studio.draw(
        Screen::call(
            "call-while-another-server-is-shown",
            in_call(Phase::Connected),
        ),
        select_harbor,
    )?;
    // A narrow window: the panel of the call starts closed and lies over the stage.
    studio.draw(
        Screen::call("call-scale-150", in_call(Phase::Connected)).scale(150),
        show_daily,
    )?;
    studio.draw(
        Screen::call("call-scale-150-members", in_call(Phase::Connected)).scale(150),
        show_daily_tab(CallTab::Members),
    )?;

    // A voice channel that this device is not in.
    let mut joining = no_call();
    joining.calls.joining = Some(daily());
    studio.draw(
        Screen::new("voice-channel-joining", two_servers()).media(joining),
        show_daily,
    )?;
    let refused = |error: FlowError| {
        let mut media = no_call();
        media.calls.outcome = Some(Outcome::JoinFailed {
            target: CallTarget {
                server_id: id(1),
                place: CallPlace::Channel {
                    workspace_id: id(10),
                    channel_id: id(35),
                },
            },
            error,
        });
        media
    };
    studio.draw(
        Screen::new("voice-channel-locked-join-refused", two_servers())
            .media(refused(FlowError::refused(ErrorCode::CallLocked))),
        |shell, _, cx| select_channel(shell, "Focus room", cx),
    )?;
    studio.draw(
        Screen::new("voice-channel-no-media-server", two_servers())
            .media(refused(FlowError::refused(ErrorCode::MediaUnavailable))),
        |shell, _, cx| select_channel(shell, "Focus room", cx),
    )?;
    let ended = |reason: EndReason| {
        let mut media = no_call();
        media.calls.outcome = Some(Outcome::Ended {
            target: daily(),
            reason,
        });
        media
    };
    studio.draw(
        Screen::new("call-ended-removed-by-a-moderator", two_servers())
            .media(ended(EndReason::Server(Some(LeaveReason::Disconnected)))),
        show_daily,
    )?;
    studio.draw(
        Screen::new("call-ended-on-another-device", two_servers())
            .media(ended(EndReason::Elsewhere)),
        |_, _, _| {},
    )?;

    // The sound settings of the application, with push-to-talk.
    let mut talk = no_call();
    talk.voice.input_mode = InputMode::PushToTalk;
    talk.voice.noise_suppression = false;
    studio.draw(
        Screen::new("settings-push-to-talk", two_servers()).media(talk.clone()),
        |shell, window, cx| {
            shell.open(Open::Settings, window, cx);
            shell.open_settings_section(SettingsSection::Voice, window, cx);
        },
    )?;
    // The question before the switch to push to talk.
    studio.draw(
        Screen::new("settings-push-to-talk-question", two_servers()).media(no_call()),
        |shell, window, cx| {
            shell.open(Open::Settings, window, cx);
            shell.open_settings_section(SettingsSection::Voice, window, cx);
            shell.ask_push_to_talk_fixture(cx);
        },
    )?;
    // The call strip states push to talk and its key.
    let mut talking = in_call(Phase::Connected);
    talking.voice.input_mode = InputMode::PushToTalk;
    studio.draw(Screen::call("call-push-to-talk", talking), show_daily)?;
    // The sensitivity of voice activation, with the live level of a call.
    let mut sensitive = in_call(Phase::Connected);
    sensitive.voice.voice_threshold = Some(-40);
    studio.draw(
        Screen::call("settings-voice-sensitivity", sensitive),
        |shell, window, cx| {
            show_daily(shell, window, cx);
            shell.open(Open::Settings, window, cx);
            shell.open_settings_section(SettingsSection::Voice, window, cx);
        },
    )?;
    Ok(())
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
    for (name, section) in [
        ("settings-picture", SettingsSection::Picture),
        ("settings-interface", SettingsSection::Interface),
        ("settings-voice", SettingsSection::Voice),
        ("settings-quiet", SettingsSection::Quiet),
        ("settings-notifications", SettingsSection::Notifications),
        ("settings-devices", SettingsSection::Devices),
        ("settings-about", SettingsSection::About),
    ] {
        studio.draw(
            Screen::new(name, two_servers()),
            move |shell, window, cx| {
                shell.open(Open::Settings, window, cx);
                shell.open_settings_section(section, window, cx);
            },
        )?;
    }
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
            "server-inactive",
            harbor_as(
                AccessState::Inactive {
                    account_id: Some(person("Luca")),
                },
                reachable(),
                None,
            ),
        ),
        (
            "server-deleted",
            harbor_as(
                AccessState::Deleted {
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
            Menu, MenuItem, Meter, Mosaic, MosaicKind, Notice, PersonTile, RecordingMark,
            SectionLabel, Segmented, SelectBox, StyleExt, Tab, TileMark, Toggle, WaitBox,
            menu_separator, unread_dot,
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
            .child(Band::new(BandKind::Rail))
            .child(
                div()
                    .flex()
                    .gap(u(8.0))
                    .child(
                        PersonTile::new("tile-speaking", "Ana", 7919, "Ana, speaking")
                            .speaking(true)
                            .marks(vec![TileMark::Level(4)]),
                    )
                    .child(
                        PersonTile::new("tile-held", "Luca (you)", 15_838, "Luca").marks(vec![
                            TileMark::HandRaised,
                            TileMark::SoundOff,
                            TileMark::MicrophoneHeld,
                        ]),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(5.0))
                    .p(u(10.0))
                    .bg(Hsla::from(colors.cypress))
                    .child(
                        Button::new("call-0")
                            .kind(ButtonKind::Call)
                            .icon(Icon::Screen),
                    )
                    .child(
                        Button::new("call-1")
                            .kind(ButtonKind::CallOn)
                            .icon(Icon::Camera),
                    )
                    .child(
                        Button::new("call-2")
                            .kind(ButtonKind::Call)
                            .icon(Icon::Hand)
                            .disabled(true),
                    )
                    .child(
                        Button::new("call-3")
                            .kind(ButtonKind::CallLeave)
                            .icon(Icon::PhoneOff)
                            .label("Leave"),
                    ),
            );
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
                    .child(
                        row()
                            .child(label("Square, off and held"))
                            .child(
                                Button::new("square-off")
                                    .kind(ButtonKind::SquareOff)
                                    .icon(Icon::MicrophoneOff),
                            )
                            .child(
                                Button::new("square-held")
                                    .kind(ButtonKind::SquareHeld)
                                    .icon(Icon::MicrophoneHeld),
                            )
                            .child(
                                Button::new("green")
                                    .kind(ButtonKind::Green)
                                    .icon(Icon::Speaker)
                                    .label("Join call"),
                            ),
                    )
                    .child(
                        row()
                            .child(label("Switch and choice"))
                            .child(Toggle::new("switch-on", "Switch", true, ("On", "Off")))
                            .child(Toggle::new("switch-off", "Switch", false, ("On", "Off")))
                            .child(div().w(u(220.0)).child(SelectBox::new(
                                "choice",
                                "Input device",
                                "Desk microphone",
                            ))),
                    )
                    .child(
                        row().child(label("Tabs")).child(
                            div()
                                .flex()
                                .h(u(42.0))
                                .px(u(8.0))
                                .bg(Hsla::from(colors.limestone))
                                .child(Tab::new("tab-0", Icon::Chat, "Chat"))
                                .child(Tab::new("tab-1", Icon::People, "Members").on(true))
                                .child(Tab::new("tab-2", Icon::Gear, "Settings")),
                        ),
                    )
                    .child(
                        row()
                            .child(label("Input meter"))
                            .child(Meter::wide(5))
                            .child(Meter::plate(3)),
                    )
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
    studio.drawing(Some("components"));
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
    studio.drawing(None);
    Ok(())
}

fn run(out: &Path) -> Result<(), String> {
    let mut studio = Studio::new(out.to_owned())?;
    let only = std::env::var("JUNTO_SCREENS_ONLY").ok();
    let drawn = match only.as_deref() {
        // The screens of one unit, while its screens are worked on.
        Some("cameras") => cameras::draw(&mut studio),
        Some("shares") => draw_shares(&mut studio),
        Some("rules") => draw_rules(&mut studio),
        Some("direct") => direct::draw(&mut studio),
        Some("messages") => messages::draw(&mut studio),
        Some("threads") => threads::draw(&mut studio),
        Some("discovery") => discovery::draw(&mut studio),
        Some("recorded") => recorded::draw(&mut studio),
        // Not a picture: the time of a frame with 10,000 loaded messages.
        Some("frames") => frames::measure(&mut studio),
        Some("audit-frames") => frames::measure_audit(&mut studio),
        Some("people-frames") => frames::measure_people(&mut studio),
        Some("admin") => admin::draw(&mut studio),
        Some("people") => people::draw(&mut studio),
        Some("roles") => roles::draw(&mut studio),
        Some("account") => account::draw(&mut studio),
        Some("book") => book::draw(&mut studio),
        Some("logo") => logo::draw(&mut studio),
        _ => draw_all(&mut studio)
            .and_then(|()| draw_calls(&mut studio))
            .and_then(|()| draw_shares(&mut studio))
            .and_then(|()| draw_rules(&mut studio))
            .and_then(|()| direct::draw(&mut studio))
            .and_then(|()| messages::draw(&mut studio))
            .and_then(|()| threads::draw(&mut studio))
            .and_then(|()| discovery::draw(&mut studio))
            .and_then(|()| recorded::draw(&mut studio))
            .and_then(|()| admin::draw(&mut studio))
            .and_then(|()| people::draw(&mut studio))
            .and_then(|()| roles::draw(&mut studio))
            .and_then(|()| account::draw(&mut studio))
            .and_then(|()| book::draw(&mut studio))
            .and_then(|()| logo::draw(&mut studio))
            .and_then(|()| draw_components(&mut studio)),
    };
    let scratch = studio.scratch.clone();
    let bands = std::mem::take(&mut studio.bands);
    drop(studio);
    // The data directories of the screens hold no server and no key of a real server.
    let removed = std::fs::remove_dir_all(&scratch).map_err(|error| error.to_string());
    drawn.and(removed)?;
    if bands.is_empty() {
        return Ok(());
    }
    for band in &bands {
        eprintln!("{band}");
    }
    Err(format!(
        "{} empty bands taller than {} design px (ADR 0730)",
        bands.len(),
        gaps::MOST_GAP
    ))
}

fn main() -> std::process::ExitCode {
    junto_client::panics::install();
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: junto-screens <directory for the PNG files>");
        return std::process::ExitCode::from(2);
    };
    match run(Path::new(&out)) {
        // A panic in a render stops in gpui and no longer ends the program; a picture of
        // an earlier frame is then no picture of the screen (ADR 0757).
        Ok(()) if junto_client::panics::panics_in_this_process() > 0 => {
            eprintln!(
                "junto-screens: {} panics happened: {:?}",
                junto_client::panics::panics_in_this_process(),
                junto_client::panics::messages_on_this_thread()
            );
            std::process::ExitCode::FAILURE
        }
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("junto-screens: {problem}");
            std::process::ExitCode::FAILURE
        }
    }
}
