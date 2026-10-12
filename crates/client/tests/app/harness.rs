//! One application window on the test platform of gpui, and the administrator of a server.

use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use gpui_base::Root;
use junto_client::app::{self, WindowStart};
use junto_client::shell::Shell;
use junto_client::state::view::{ChannelRow, Members, ServerStatus, ShellView};
use junto_client::state::{AppState, Timing};
use junto_client_core::protocol::invite::CreateInvite;
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::version::SupportedVersions;
use junto_client_core::{Client, ServerHandle};
use junto_media::audio::synthetic::SyntheticHost;
use junto_media::calls::{CallTiming, MediaOptions};
use junto_media::permission::FixedPermission;
use junto_media::share::synthetic::SyntheticCapture;

use crate::support::{Device, TestServer, claim, logged_in};

/// How long a step of a test may take. It bounds one step, never a whole server.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// The tokio runtime of a test. The window runs on the executor of gpui; the server
/// process, the proxy and the client of the administrator need tokio.
pub struct World {
    runtime: tokio::runtime::Runtime,
}

impl World {
    pub fn new() -> Self {
        Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    pub fn run<F: Future>(&self, work: F) -> F::Output {
        self.runtime.block_on(work)
    }

    pub fn server(&self, name: &str) -> TestServer {
        self.run(TestServer::start(name))
    }

    pub fn server_with_media(&self, name: &str) -> TestServer {
        self.run(TestServer::start_with_media(name))
    }

    /// A device that owns the server: it claimed it with the owner code.
    pub fn owner(&self, server: &TestServer) -> Admin {
        self.run(async {
            let device = Device::new();
            let handle = device.add(&server.address()).await;
            handle.dispatch(claim(server, "Ada")).await.unwrap();
            logged_in(&handle);
            Admin { device, handle }
        })
    }
}

/// The frequency of the tone that the microphone of the administrator gives.
pub const ADMIN_TONE_HZ: f32 = 440.0;

/// The client of the administrator of one server.
pub struct Admin {
    /// The data directory of the administrator lives as long as the client.
    pub device: Device,
    pub handle: ServerHandle,
}

/// The administrator in calls: a tone as the microphone and a sink that measures.
pub struct Caller {
    pub calls: junto_media::Calls,
    pub sound: SyntheticHost,
}

impl Admin {
    /// The calls of the administrator. The event stream of the device starts, so that its
    /// calls follow the state of the server.
    pub fn caller(&self, world: &World) -> Caller {
        world.run(async {
            self.device.client.start_event_streams().unwrap();
            self.device.client.select(Some(self.handle.id()));
            let sound = SyntheticHost::with_tone(ADMIN_TONE_HZ);
            let calls = junto_media::Calls::new(
                self.device.client.clone(),
                MediaOptions {
                    camera: std::sync::Arc::new(
                        junto_media::camera::synthetic::SyntheticCamera::new(),
                    ),
                    host: std::sync::Arc::new(sound.clone()),
                    permission: std::sync::Arc::new(FixedPermission::allowed()),
                    capture: std::sync::Arc::new(SyntheticCapture::new()),
                    timing: CallTiming::default(),
                },
            );
            // A steady tone passes unchanged only without the processing of a voice.
            calls.set_voice(junto_media::engine::VoiceConfig {
                processing: junto_media::audio::Processing::OFF,
                ..junto_media::engine::VoiceConfig::default()
            });
            Caller { calls, sound }
        })
    }
}

/// The level of a tone in the last half second of what a sink was given.
pub fn heard(sound: &SyntheticHost, hz: f32) -> f32 {
    let (played, rate) = sound.played(Duration::from_millis(500));
    junto_media::audio::synthetic::tone_level(&played, rate, hz)
}

/// A tone that arrives has about the level at which it was sent: -12 dBFS.
pub const HEARD_DBFS: f32 = -20.0;
/// A tone that does not arrive is below this.
pub const ABSENT_DBFS: f32 = -55.0;

impl Admin {
    pub fn invite(&self, world: &World, uses: u32) -> InviteCode {
        world
            .run(self.handle.api().create_invite(&CreateInvite {
                max_uses: Some(uses),
                ..CreateInvite::default()
            }))
            .unwrap()
            .code
    }
}

/// One window of the application with its state.
pub struct Window {
    pub cx: VisualTestContext,
    pub shell: Entity<Shell>,
    pub state: Entity<AppState>,
    /// The sound devices of the application: a tone as the microphone and a sink that
    /// measures. No test opens a real device.
    pub sound: SyntheticHost,
    /// The screen capture of the application: a picture and a tone that the program
    /// makes. No test captures a screen.
    pub capture: SyntheticCapture,
    pub camera: junto_media::camera::synthetic::SyntheticCamera,
}

/// The frequency of the tone that the microphone of a window gives.
pub const WINDOW_TONE_HZ: f32 = 660.0;

/// The times of the application in a test: a waiting request is checked often, so that a
/// test does not wait for the 5 seconds of the product.
fn timing() -> Timing {
    Timing {
        decision_interval: Duration::from_millis(200),
        retry_interval: Duration::from_secs(20),
    }
}

/// Opens the application on a data directory, as a start of the binary does.
pub fn open(cx: &mut TestAppContext, data_dir: &Path) -> Window {
    open_with(cx, data_dir, None)
}

/// Opens the application with the start argument for the interface scale.
pub fn open_with(cx: &mut TestAppContext, data_dir: &Path, scale: Option<u32>) -> Window {
    // The client core answers from threads of its own runtime, at real times. The
    // scheduler of the test accepts that only when it is told so.
    cx.executor().allow_parking();
    let client: Client = Device::open(data_dir, SupportedVersions::THIS_BUILD);
    let start = WindowStart {
        scale_percent: scale,
        activate: false,
        timing: timing(),
    };
    let sound = SyntheticHost::with_tone(WINDOW_TONE_HZ);
    let capture = SyntheticCapture::new();
    let camera = junto_media::camera::synthetic::SyntheticCamera::new();
    let media = MediaOptions {
        camera: std::sync::Arc::new(camera.clone()),
        host: std::sync::Arc::new(sound.clone()),
        permission: std::sync::Arc::new(FixedPermission::allowed()),
        capture: std::sync::Arc::new(capture.clone()),
        timing: CallTiming::default(),
    };
    let state = cx.update(|cx| {
        if !cx.has_global::<Initialized>() {
            app::init(cx).unwrap();
            cx.set_global(Initialized);
        }
        app::state_with_media(client, start, media, cx)
    });
    let built: std::rc::Rc<std::cell::RefCell<Option<Entity<Shell>>>> = std::rc::Rc::default();
    let (slot, shown) = (built.clone(), state.clone());
    let window = cx.add_window(move |window, cx| {
        let shell = gpui::AppContext::new(cx, |cx| Shell::new(shown, window, cx));
        *slot.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let shell = built.borrow_mut().take().unwrap();
    let window = Window {
        camera,
        cx: VisualTestContext::from_window(window.into(), cx),
        shell,
        state,
        sound,
        capture,
    };
    window.cx.run_until_parked();
    window
}

struct Initialized;

impl gpui::Global for Initialized {}

impl Window {
    /// Ends the application, as closing its window does: the window is removed, and the
    /// state of the application ends with it, with its tasks and its client. A test that
    /// starts the application again on the same data directory closes it first, so that it
    /// tests a second start and not two running applications.
    #[track_caller]
    pub fn close(mut self) {
        let state = self.state.downgrade();
        self.cx.update(|window, _| window.remove_window());
        let Self {
            cx,
            shell,
            state: held,
            ..
        } = self;
        drop((shell, held));
        // The entities of a removed window are released when the application has handled
        // what the removal started. That is done before any waiting work runs, so that
        // work that the window started runs without its window.
        for _ in 0..3 {
            TestAppContext::update(&cx, |_| {});
        }
        let released = state.upgrade().is_none();
        cx.run_until_parked();
        assert!(released, "the state of the application outlives its window");
        assert!(
            state.upgrade().is_none(),
            "the state of the application outlives its window"
        );
    }

    /// What the window drew last. The client core can be ahead of the window for a
    /// moment; a test reads what a person sees.
    pub fn view(&mut self) -> ShellView {
        self.cx.run_until_parked();
        self.shell
            .read_with(&self.cx, |shell, _| shell.drawn().cloned())
            .expect("the window was drawn")
    }

    /// Waits until `holds`, and draws the window while it waits, as a running application
    /// does.
    #[track_caller]
    pub fn until(&mut self, what: &str, mut holds: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.cx.run_until_parked();
            if holds(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "after {PATIENCE:?} this did not hold: {what}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// True when an element with this name is drawn now.
    pub fn shows(&mut self, selector: &'static str) -> bool {
        self.cx.run_until_parked();
        self.cx.debug_bounds(selector).is_some()
    }

    /// Clicks the middle of the element with this name.
    #[track_caller]
    pub fn click(&mut self, selector: &'static str) {
        self.cx.run_until_parked();
        let bounds = self
            .cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"));
        self.cx.simulate_click(bounds.center(), Modifiers::none());
        self.cx.run_until_parked();
    }

    /// Clicks the element with this name when it is drawn now. A test uses it for an
    /// element that a reload of the state takes away for a moment.
    pub fn click_if_shown(&mut self, selector: &'static str) -> bool {
        self.cx.run_until_parked();
        let Some(bounds) = self.cx.debug_bounds(selector) else {
            return false;
        };
        self.cx.simulate_click(bounds.center(), Modifiers::none());
        self.cx.run_until_parked();
        true
    }

    /// True when an element with a name that the test builds is drawn now.
    pub fn shows_named(&mut self, name: String) -> bool {
        self.shows(selector(name))
    }

    /// Clicks the element with a name that the test builds, such as the row of one person.
    #[track_caller]
    pub fn click_named(&mut self, name: String) {
        self.click(selector(name));
    }

    /// Presses the right button of the pointer in the middle of the element with this
    /// name, and releases it.
    #[track_caller]
    pub fn right_click_named(&mut self, name: String) {
        self.cx.run_until_parked();
        let name = selector(name);
        let bounds = self
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not drawn"));
        let at = bounds.center();
        self.cx
            .simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
        self.cx
            .simulate_mouse_up(at, gpui::MouseButton::Right, Modifiers::none());
        self.cx.run_until_parked();
    }

    /// Presses and releases keys, as a keyboard does: `press("down enter")`. A focused
    /// control is activated when Enter or Space is released.
    pub fn press(&mut self, keys: &str) {
        for key in keys.split(' ') {
            let keystroke = gpui::Keystroke::parse(key).unwrap();
            self.cx.simulate_keystrokes(key);
            self.cx.simulate_event(gpui::KeyUpEvent { keystroke });
            self.cx.run_until_parked();
        }
    }

    pub fn in_shell<R>(
        &mut self,
        act: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) -> R,
    ) -> R {
        let shell = self.shell.clone();
        let result = self
            .cx
            .update(|window, cx| shell.update(cx, |shell, cx| act(shell, window, cx)));
        self.cx.run_until_parked();
        result
    }

    /// The first start: the person types a name and continues.
    #[track_caller]
    pub fn choose_name(&mut self, name: &str) {
        assert!(self.shows("welcome-dialog"));
        self.in_shell(|shell, window, cx| shell.type_name(name, window, cx));
        self.click("continue");
    }

    /// Types an address into the server book and sends it.
    pub fn enter_address(&mut self, address: &str) {
        self.in_shell(|shell, window, cx| shell.type_address(address, window, cx));
        self.click("add-server");
    }

    /// Types an address and waits for the dialog of the server that answered.
    #[track_caller]
    pub fn find_server(&mut self, address: &str) {
        self.enter_address(address);
        self.until("the join dialog opens", |window| {
            window.shows("join-dialog")
        });
    }

    /// Types a code into the join dialog and presses "Join with code".
    #[track_caller]
    pub fn join_with_code(&mut self, code: &str) {
        self.in_shell(|shell, window, cx| shell.type_code(code, window, cx));
        self.click("join-with-code");
    }

    /// Waits until the selected server shows its lists.
    #[track_caller]
    pub fn until_ready(&mut self) {
        self.until("the selected server shows its lists", |window| {
            let view = window.view();
            view.selected
                .as_ref()
                .is_some_and(|server| server.status == ServerStatus::Ready)
                && view.workspace().is_some()
        });
    }

    /// The status of the selected server.
    pub fn status(&mut self) -> Option<ServerStatus> {
        self.view().selected.map(|server| server.status)
    }

    /// The channels of the shown workspace, in the order of the channel column.
    pub fn channels(&mut self) -> Vec<ChannelRow> {
        self.view()
            .workspace()
            .map(|workspace| {
                workspace
                    .sections
                    .iter()
                    .flat_map(|section| section.rows.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn channel_names(&mut self) -> Vec<String> {
        self.channels().into_iter().map(|row| row.name).collect()
    }

    /// The name of the drawn row of the channel with this name in the shown workspace. A
    /// row is named by the identifier of its channel, because two channels can have one
    /// name.
    pub fn channel_row(&mut self, name: &str) -> Option<&'static str> {
        let mut rows = self.channels().into_iter().filter(|row| row.name == name);
        let (row, more) = (rows.next()?, rows.next());
        assert!(more.is_none(), "two channels are named {name}");
        Some(selector(format!("channel-row:{}", row.id)))
    }

    /// True when the row of the channel with this name is drawn.
    pub fn shows_channel(&mut self, name: &str) -> bool {
        self.channel_row(name).is_some_and(|row| self.shows(row))
    }

    #[track_caller]
    pub fn click_channel(&mut self, name: &str) {
        let row = self
            .channel_row(name)
            .unwrap_or_else(|| panic!("no channel is named {name}"));
        self.click(row);
    }

    /// True when the row of the member with this name is drawn in the members panel.
    pub fn shows_member(&mut self, name: &str) -> bool {
        let view = self.view();
        let Some(Members::Loaded(members)) = view.workspace().map(|workspace| &workspace.members)
        else {
            return false;
        };
        let mut rows = members.iter().filter(|member| member.name == name);
        let (Some(row), more) = (rows.next(), rows.next()) else {
            return false;
        };
        assert!(more.is_none(), "two members are named {name}");
        self.shows(selector(format!("member-row:{}", row.account_id)))
    }
}

/// gpui finds a drawn element by a name that lives as long as the program. A test builds
/// few names, so each one is kept until the test process ends.
pub(crate) fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// A new data directory with a profile name, as after a first start.
pub fn named(cx: &mut TestAppContext, name: &str) -> (tempfile::TempDir, Window) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    window.choose_name(name);
    (directory, window)
}

/// Opens the application as a new member of a server: a first start, the address, and an
/// invite code of the administrator.
pub fn member(
    cx: &mut TestAppContext,
    world: &World,
    address: &str,
    admin: &Admin,
    name: &str,
) -> (tempfile::TempDir, Window) {
    let (directory, mut window) = named(cx, name);
    let code = admin.invite(world, 1);
    window.find_server(address);
    window.join_with_code(code.expose());
    window.until_ready();
    (directory, window)
}
