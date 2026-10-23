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
use junto_media::share::ScreenCapture;
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
        log_into_test_output();
        Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    /// Runs work on the runtime of the test, with the time limit of a live test: work
    /// that waits for the network or another process ends.
    pub fn run<F: Future>(&self, work: F) -> F::Output {
        self.runtime.block_on(crate::limit::limited_step(work))
    }

    /// Runs work that takes longer than a live test may, with its own limit. The native
    /// targets use it to write a long channel.
    #[allow(dead_code)]
    pub fn run_for<F: Future>(&self, limit: Duration, work: F) -> F::Output {
        self.runtime
            .block_on(crate::limit::limited_for(limit, work))
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
    /// The server of the administrator, for the state that the calls follow.
    pub handle: ServerHandle,
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
            Caller {
                calls,
                sound,
                handle: self.handle.clone(),
            }
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
    /// makes. Only the native stage of a share (ADR 0913) gives the application the capture
    /// of the system in its place; this one is then unused.
    pub capture: SyntheticCapture,
    pub camera: junto_media::camera::synthetic::SyntheticCamera,
    /// The microphone permission of the system, which a test changes as a person does in
    /// the settings of the system. It allows the microphone at the start.
    pub permission: FixedPermission,
    /// How long the application held the thread of the windows in one piece, in each of
    /// the calls of the harness that run its work.
    pub held: Held,
    holding: usize,
}

/// The pieces of work of the application on the thread of the windows: the work of one
/// event of the pointer or the keyboard with the work that its handlers started, or a
/// frame of the display with the work before and after it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Held {
    pub count: usize,
    /// The longest piece.
    pub longest: Duration,
    /// The pieces longer than [`Held::SLOW`].
    pub slow: usize,
}

impl Held {
    /// A piece of work after which a display of 60 frames a second has missed at least
    /// two frames.
    pub const SLOW: Duration = Duration::from_millis(50);

    fn add(&mut self, time: Duration) {
        self.count += 1;
        self.longest = self.longest.max(time);
        self.slow += usize::from(time > Self::SLOW);
    }
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
    log_into_test_output();
    let client: Client = Device::open(data_dir, SupportedVersions::THIS_BUILD);
    open_client(cx, client, scale, None)
}

/// Opens the application with this screen capture in the place of the synthetic one.
// The native window stage of macOS (ADR 0913) and the share test of Windows (ADR 0706).
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "native-device-tests")
))]
pub fn open_with_capture(
    cx: &mut TestAppContext,
    data_dir: &Path,
    capture: std::sync::Arc<dyn ScreenCapture>,
) -> Window {
    log_into_test_output();
    let client: Client = Device::open(data_dir, SupportedVersions::THIS_BUILD);
    open_client(cx, client, None, Some(capture))
}

/// Opens the application with a source of releases, and with the version that the build
/// that runs is taken to have. The application itself has no source of releases yet.
pub fn open_with_updates(
    cx: &mut TestAppContext,
    data_dir: &Path,
    source: std::sync::Arc<junto_client_core::updates::FakeSource>,
    build: junto_client_core::updates::Version,
) -> Window {
    log_into_test_output();
    let mut options = junto_client_core::ClientOptions::new(data_dir.join("junto"));
    options.update_source = Some(source);
    options.build = build;
    open_client(cx, Client::open(options).unwrap(), None, None)
}

/// Sends the log of the client into the output of the test, which the test runner keeps for
/// each test and prints when the test fails. The threads that a test starts, also those of
/// the runtime of its client, write into the output of that test. The level is that of
/// `JUNTO_LOG_LEVEL`, as for the binary, with `info` when it is not set.
fn log_into_test_output() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let level = std::env::var("JUNTO_LOG_LEVEL")
            .ok()
            .and_then(|level| level.parse::<tracing::Level>().ok())
            .unwrap_or(tracing::Level::INFO);
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(level)
            .try_init();
    });
}

/// Opens the window of the application on an opened client. Without `system_capture` the
/// application captures the synthetic screen of the window.
fn open_client(
    cx: &mut TestAppContext,
    client: Client,
    scale: Option<u32>,
    system_capture: Option<std::sync::Arc<dyn ScreenCapture>>,
) -> Window {
    log_into_test_output();
    // The client core answers from threads of its own runtime, at real times. The
    // scheduler of the test accepts that only when it is told so.
    cx.executor().allow_parking();
    let start = WindowStart {
        scale_percent: scale,
        activate: false,
        timing: timing(),
    };
    let sound = SyntheticHost::with_tone(WINDOW_TONE_HZ);
    let capture = SyntheticCapture::new();
    let camera = junto_media::camera::synthetic::SyntheticCamera::new();
    let permission = FixedPermission::allowed();
    let media = MediaOptions {
        camera: std::sync::Arc::new(camera.clone()),
        host: std::sync::Arc::new(sound.clone()),
        permission: std::sync::Arc::new(permission.clone()),
        capture: system_capture.unwrap_or_else(|| std::sync::Arc::new(capture.clone())),
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
        permission,
        cx: VisualTestContext::from_window(window.into(), cx),
        shell,
        state,
        sound,
        capture,
        held: Held::default(),
        holding: 0,
    };
    window.cx.run_until_parked();
    window
}

struct Initialized;

impl gpui::Global for Initialized {}

impl Window {
    /// Runs `work` and counts it as one piece of work of the application in [`Window::held`].
    /// A piece inside another is a part of the outer one.
    pub fn holding<T>(&mut self, work: impl FnOnce(&mut Self) -> T) -> T {
        self.holding += 1;
        let started = Instant::now();
        let result = work(self);
        self.holding -= 1;
        if self.holding == 0 {
            self.held.add(started.elapsed());
        }
        result
    }

    /// Runs the work of the application until it has none. A wait of a test has a limit:
    /// an application that always has work fails the test here, and does not hold it.
    #[track_caller]
    pub fn park(&mut self) {
        let deadline = Instant::now() + PATIENCE;
        self.holding(|window| {
            while window.cx.executor().tick() {
                assert!(
                    Instant::now() < deadline,
                    "after {PATIENCE:?} the application still has work: it does not come to rest"
                );
            }
        });
    }

    /// Gives every window one frame of the display, as the system does at each refresh.
    /// The test platform has no display: what a window hands to its next frame is taken
    /// here. Returns how many windows had something for the frame.
    pub fn frame(&mut self) -> usize {
        use gpui::AppContext as _;
        self.holding(|this| {
            let mut took = 0;
            for handle in this.cx.windows() {
                let ran = this
                    .cx
                    .update_window(handle, |_, window, cx| window.simulate_next_frame(cx));
                took += usize::from(ran.is_ok_and(|ran| ran > 0));
            }
            took
        })
    }

    /// Waits until every window has taken the pictures of the call that arrived: the
    /// follower of a window converts them on a thread of the runtime of the client, and the
    /// next frame of the display takes them (ADR 0793).
    #[track_caller]
    pub fn until_pictures_taken(&mut self) {
        use gpui::AppContext as _;
        self.until("every window took the pictures of the call", |window| {
            window.cx.windows().into_iter().all(|handle| {
                window
                    .cx
                    .update_window(handle, |root, _, cx| {
                        let shell = root
                            .downcast::<Root>()
                            .ok()
                            .and_then(|root| root.read(cx).view().clone().downcast::<Shell>().ok());
                        shell.is_none_or(|shell| !shell.read(cx).pictures_pending())
                    })
                    .unwrap_or(true)
            })
        });
    }

    /// Runs the application until it has no work, with one frame of the display.
    #[track_caller]
    pub fn settle(&mut self) {
        self.holding(|window| {
            window.park();
            if window.frame() > 0 {
                window.park();
            }
        });
    }

    /// Presses and releases the pointer on the element with this name as it is drawn now.
    /// The application does no other work before the press: the control is the one that
    /// was drawn, also when the state of the application has changed since.
    #[track_caller]
    pub fn press_as_drawn(&mut self, selector: &'static str) {
        let bounds = self
            .cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"));
        let (down, up) = pointer_press(bounds.center(), gpui::MouseButton::Left);
        self.holding(|this| {
            this.cx.update(|window, cx| {
                window.dispatch_event(down, cx);
                window.dispatch_event(up, cx);
            });
        });
    }

    /// Gives one event of the pointer or the keyboard to the window, as the input handler
    /// of the system does, and runs the work that it started. The test platform of gpui
    /// and a native window take events on this one path (ADR 0902).
    pub fn dispatch(&mut self, event: impl gpui::InputEvent) {
        let input = event.to_platform_input();
        self.holding(|this| {
            this.cx.update(|window, cx| {
                window.dispatch_event(input, cx);
            });
            this.cx.run_until_parked();
        });
    }

    /// Presses and releases a button of the pointer at a point of the window.
    pub fn press_at(&mut self, position: gpui::Point<gpui::Pixels>, button: gpui::MouseButton) {
        let (down, up) = pointer_press(position, button);
        for input in [down, up] {
            self.holding(|this| {
                this.cx.update(|window, cx| {
                    window.dispatch_event(input, cx);
                });
                this.cx.run_until_parked();
            });
        }
    }

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
        self.settle();
        self.shell
            .read_with(&self.cx, |shell, _| shell.drawn().cloned())
            .expect("the window was drawn")
    }

    /// Waits until `holds`, and draws the window while it waits, as a running application
    /// does.
    #[track_caller]
    pub fn until(&mut self, what: &str, mut holds: impl FnMut(&mut Self) -> bool) {
        crate::limit::step(what);
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.settle();
            if holds(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "after {PATIENCE:?} this did not hold: {what}; the calls of the window: {:?}",
                self.state
                    .read_with(&self.cx, |state, _| state.calls().view())
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until `value` gives a value, and returns it.
    #[track_caller]
    pub fn until_value<T>(
        &mut self,
        what: &str,
        mut value: impl FnMut(&mut Self) -> Option<T>,
    ) -> T {
        let mut found = None;
        self.until(what, |window| {
            found = value(window);
            found.is_some()
        });
        found.expect("the value that was waited for")
    }

    /// True when an element with this name is drawn now.
    pub fn shows(&mut self, selector: &'static str) -> bool {
        self.settle();
        self.cx.debug_bounds(selector).is_some()
    }

    /// Reveals a control by visiting the measured scroll range once, without clicking.
    #[track_caller]
    pub fn reveal(
        &mut self,
        scroll: &gpui::ScrollHandle,
        selector: &'static str,
    ) -> gpui::Bounds<gpui::Pixels> {
        self.settle();
        let visible = |this: &mut Self| {
            let page = scroll.bounds();
            this.cx
                .debug_bounds(selector)
                .filter(|bounds| bounds.top() >= page.top() && bounds.bottom() <= page.bottom())
        };
        if let Some(bounds) = visible(self) {
            return bounds;
        }
        let maximum = f32::from(scroll.max_offset().y);
        let step = (f32::from(scroll.bounds().size.height) * 0.75).max(1.0);
        for page in 0..=(maximum / step).ceil() as usize {
            let offset = (page as f32 * step).min(maximum);
            scroll.set_offset(gpui::point(gpui::px(0.0), gpui::px(-offset)));
            self.in_shell(|_, _, cx| cx.notify());
            if let Some(bounds) = visible(self) {
                return bounds;
            }
        }
        panic!("{selector} is not in the measured scroll range");
    }

    /// Clicks the middle of the element with this name.
    #[track_caller]
    pub fn click(&mut self, selector: &'static str) {
        self.settle();
        let bounds = self
            .cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"));
        self.press_at(bounds.center(), gpui::MouseButton::Left);
        self.settle();
    }

    /// Clicks the element with this name when it is drawn now. A test uses it for an
    /// element that a reload of the state takes away for a moment.
    pub fn click_if_shown(&mut self, selector: &'static str) -> bool {
        self.settle();
        let Some(bounds) = self.cx.debug_bounds(selector) else {
            return false;
        };
        self.press_at(bounds.center(), gpui::MouseButton::Left);
        self.settle();
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
        self.settle();
        let name = selector(name);
        let bounds = self
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not drawn"));
        self.press_at(bounds.center(), gpui::MouseButton::Right);
        self.settle();
    }

    /// Presses and releases keys, as a keyboard does: `press("down enter")`. A focused
    /// control is activated when Enter or Space is released.
    pub fn press(&mut self, keys: &str) {
        for key in keys.split(' ') {
            let keystroke = gpui::Keystroke::parse(key).unwrap();
            self.holding(|window| window.cx.simulate_keystrokes(key));
            self.dispatch(gpui::KeyUpEvent { keystroke });
            self.settle();
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
        self.settle();
        result
    }

    /// The call window of the application, with the state, the sound, the capture and the
    /// camera of this window: both windows hold one state (ADR 0772).
    #[track_caller]
    pub fn call_window(&mut self) -> Window {
        use gpui::AppContext as _;
        self.settle();
        let main = self.cx.update(|window, _| window.window_handle());
        let handle = self
            .cx
            .windows()
            .into_iter()
            .find(|handle| *handle != main)
            .expect("a call window is open");
        let shell = self
            .cx
            .update_window(handle, |root, _, cx| {
                root.downcast::<Root>()
                    .ok()
                    .and_then(|root| root.read(cx).view().clone().downcast::<Shell>().ok())
            })
            .ok()
            .flatten()
            .expect("the call window draws a shell");
        let mut window = Window {
            cx: VisualTestContext::from_window(handle, &self.cx),
            shell,
            state: self.state.clone(),
            sound: self.sound.clone(),
            capture: self.capture.clone(),
            camera: self.camera.clone(),
            permission: self.permission.clone(),
            held: Held::default(),
            holding: 0,
        };
        window.settle();
        window
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

/// The two events of one press of a pointer button at one point.
fn pointer_press(
    position: gpui::Point<gpui::Pixels>,
    button: gpui::MouseButton,
) -> (gpui::PlatformInput, gpui::PlatformInput) {
    use gpui::InputEvent;
    let modifiers = Modifiers::none();
    let down = gpui::MouseDownEvent {
        position,
        modifiers,
        button,
        click_count: 1,
        first_mouse: false,
    };
    let up = gpui::MouseUpEvent {
        position,
        modifiers,
        button,
        click_count: 1,
    };
    (down.to_platform_input(), up.to_platform_input())
}

/// gpui finds a drawn element by a name that lives as long as the program. A test builds
/// few names, so each one is kept until the test process ends.
pub(crate) fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// The lock of a data directory, held as another program that uses the directory holds it
/// (ADR 0710). The thread of the window must not wait for it (ADR 0730).
pub struct HeldDirectory {
    release: std::sync::mpsc::Sender<()>,
    holder: std::thread::JoinHandle<bool>,
}

/// Takes the lock of the data directory of the window and holds it until
/// [`HeldDirectory::release`], or for at most [`PATIENCE`]: a window that waits for the
/// lock is then let go, and the release fails the test.
pub fn hold_data_directory(window: &Window) -> HeldDirectory {
    let data_dir = window.state.read_with(&window.cx, |state, _| {
        state.client().data_dir().root().to_owned()
    });
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("lock"))
        .unwrap();
    file.lock().unwrap();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let waited = released.recv_timeout(PATIENCE).is_err();
        drop(file);
        waited
    });
    HeldDirectory { release, holder }
}

impl HeldDirectory {
    pub fn release(self) {
        let _ = self.release.send(());
        let waited = self.holder.join().unwrap();
        assert!(
            !waited,
            "the thread of the window waited for the lock of its data directory"
        );
    }
}

/// A new data directory with a profile name, as after a first start.
pub fn named(cx: &mut TestAppContext, name: &str) -> (tempfile::TempDir, Window) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    window.choose_name(name);
    (directory, window)
}

/// Opens the application with this screen capture as a new member of a server, as
/// [`member`] does.
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "native-device-tests")
))]
pub fn member_with_capture(
    cx: &mut TestAppContext,
    world: &World,
    address: &str,
    admin: &Admin,
    name: &str,
    capture: std::sync::Arc<dyn ScreenCapture>,
) -> (tempfile::TempDir, Window) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open_with_capture(cx, directory.path(), capture);
    window.choose_name(name);
    let code = admin.invite(world, 1);
    window.find_server(address);
    window.join_with_code(code.expose());
    window.until_ready();
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
