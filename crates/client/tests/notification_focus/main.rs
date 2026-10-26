//! A press on a notification of the system gives the main window the focus on Wayland
//! (ADR 0742, ADR 0757). The application runs on the run loop of the system with the main
//! window of an empty data directory and a second window that has the focus. A notification
//! service on a private session bus shows the notification of the application, takes an
//! activation token from the compositor as a service does for a press, sends the token and
//! then the press. The compositor must give the main window the focus. Before the client
//! passed the token, the window asked the compositor for a token of its own, which a
//! compositor does not honour without a recent input event of the window.
//!
//! It runs under the sway of `tools/linux/display.sh`, on the main thread, so it is a test
//! without the test runner. It does nothing unless `JUNTO_TEST_NOTIFICATION_FOCUS` is `1`:
//!
//! ```sh
//! tools/linux/display.sh wayland sh -c \
//!     'JUNTO_TEST_NOTIFICATION_FOCUS=1 cargo test -p junto-client --test notification_focus'
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::var("JUNTO_TEST_NOTIFICATION_FOCUS").as_deref() != Ok("1") {
        println!("notification_focus: not run, JUNTO_TEST_NOTIFICATION_FOCUS is not 1");
        return ExitCode::SUCCESS;
    }
    #[cfg(target_os = "linux")]
    {
        linux::main()
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("notification_focus: not run, the test is for Wayland on Linux");
        ExitCode::SUCCESS
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, ExitCode, Stdio};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use gpui::{
        AppContext, AsyncApp, Context, IntoElement, Render, TitlebarOptions, Window, WindowOptions,
    };
    use junto_client::app::{self, WindowStart};
    use junto_client::assets::Assets;
    use junto_client::notify::SystemNotifier;
    use junto_client::state::notices::Alert;
    use junto_client_core::{Client, ClientOptions};
    use junto_media::calls::{CallTiming, MediaOptions};
    use tokio::sync::mpsc;
    use zbus::zvariant::OwnedValue;

    /// How long a step may wait for the application, the bus or the compositor.
    const PATIENCE: Duration = Duration::from_secs(20);
    /// How long the whole test may run.
    const WHOLE: Duration = Duration::from_secs(180);
    const OTHER_APP_ID: &str = "junto-test-other-window";
    const SERVICE: &str = "org.freedesktop.Notifications";
    const OBJECT: &str = "/org/freedesktop/Notifications";
    /// Set on the process that runs the application on the private bus.
    const ON_BUS: &str = "JUNTO_TEST_NOTIFICATION_FOCUS_ON_BUS";

    pub fn main() -> ExitCode {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() || std::env::var_os("SWAYSOCK").is_none() {
            eprintln!(
                "notification_focus: failed: the test needs the sway of tools/linux/display.sh"
            );
            return ExitCode::FAILURE;
        }
        let outcome = if std::env::var_os(ON_BUS).is_some() {
            application()
        } else {
            on_a_private_bus()
        };
        match outcome {
            Ok(()) => {
                println!("notification_focus: passed");
                ExitCode::SUCCESS
            }
            Err(failure) => {
                eprintln!("notification_focus: failed: {failure}");
                ExitCode::FAILURE
            }
        }
    }

    /// Runs this program again with a session bus of its own, so that no notification
    /// service of the machine takes part, and removes the bus after it.
    fn on_a_private_bus() -> Result<(), String> {
        let directory = tempfile::Builder::new()
            .prefix("junto-focus-")
            .tempdir_in("/tmp")
            .map_err(|error| format!("no folder for the bus: {error}"))?;
        let socket = directory.path().join("bus");
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}", socket.display()))
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| format!("dbus-daemon did not start: {error}"))?;
        let mut printed = String::new();
        let read = BufReader::new(bus.stdout.take().unwrap()).read_line(&mut printed);
        let outcome = match read {
            Ok(_) => run_on(&format!("unix:path={}", socket.display())),
            Err(error) => Err(format!("dbus-daemon printed no address: {error}")),
        };
        let _ = bus.kill();
        let _ = bus.wait();
        outcome
    }

    fn run_on(address: &str) -> Result<(), String> {
        let program = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut child = Command::new(program)
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .env(ON_BUS, "1")
            .spawn()
            .map_err(|error| format!("the application process did not start: {error}"))?;
        let deadline = Instant::now() + WHOLE;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("the application process ended: {status}")),
                Ok(None) if Instant::now() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("the application process ran longer than {WHOLE:?}"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(error) => return Err(format!("the application process is lost: {error}")),
            }
        }
    }

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    /// The application on the private bus: the main window, a second window with the
    /// focus, the notification service, and the press.
    fn application() -> Result<(), String> {
        // sway gives the focus for a valid token, as GNOME and KDE do; its default only
        // marks the window as urgent.
        swaymsg(&["focus_on_window_activation", "focus"])?;
        let data_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let directory = data_dir.path().join("junto");
        let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
        let result = outcome.clone();
        gpui_platform::application()
            .with_assets(Assets)
            .run(move |cx| {
                app::init(cx).unwrap();
                let client = Client::open(ClientOptions::new(directory.clone())).unwrap();
                let start = WindowStart {
                    activate: false,
                    ..WindowStart::default()
                };
                let state = app::state_with_media(client, start, synthetic_media(), cx);
                app::open_main_window(state, start, cx).unwrap();
                let other = WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("Another window".into()),
                        ..TitlebarOptions::default()
                    }),
                    app_id: Some(OTHER_APP_ID.to_owned()),
                    ..WindowOptions::default()
                };
                cx.open_window(other, |_, cx| cx.new(|_| Blank)).unwrap();
                let root = directory.clone();
                cx.spawn(async move |cx| {
                    let checked = stages(&root, cx).await;
                    *result.borrow_mut() = Some(checked);
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
        let outcome = outcome.borrow_mut().take();
        outcome.unwrap_or_else(|| Err("the application ended before the test did".into()))
    }

    async fn stages(data_dir: &std::path::Path, cx: &mut AsyncApp) -> Result<(), String> {
        until("the other window has the focus", cx, || {
            Ok(focused()?.as_deref() == Some(OTHER_APP_ID))
        })
        .await?;
        notification_service()?;
        let pressed: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
        let seen = pressed.clone();
        let notifier = cx.update(|cx| {
            junto_client::notify::platform(cx, data_dir, move |_, token, cx| {
                seen.borrow_mut().push(token.map(str::to_owned));
                junto_client::notify::bring_to_front(token, cx);
            })
        });
        let alert = Alert {
            tag: "focus".into(),
            title: "A message".into(),
            body: "A press opens the window".into(),
        };
        cx.update(|cx| SystemNotifier::show(&*notifier, &alert, cx));
        until("the press reached the application", cx, || {
            Ok(!pressed.borrow().is_empty())
        })
        .await?;
        let token = pressed.borrow()[0].clone();
        if token.is_none() {
            return Err("the press carried no activation token".into());
        }
        until("the main window has the focus", cx, || {
            Ok(focused()?.as_deref() == Some(app::APP_ID))
        })
        .await
    }

    async fn until(
        what: &str,
        cx: &mut AsyncApp,
        mut holds: impl FnMut() -> Result<bool, String>,
    ) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if holds()? {
                return Ok(());
            }
            if Instant::now() > deadline {
                let focus = focused()?;
                return Err(format!(
                    "after {PATIENCE:?} this did not hold: {what}; the focus is on {focus:?}"
                ));
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
    }

    fn swaymsg(arguments: &[&str]) -> Result<serde_json::Value, String> {
        let output = Command::new("swaymsg")
            .arg("--raw")
            .args(arguments)
            .output()
            .map_err(|error| format!("swaymsg did not run: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "swaymsg {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stdout)
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())
    }

    /// The application identifier of the window that has the focus in sway.
    fn focused() -> Result<Option<String>, String> {
        fn search(node: &serde_json::Value) -> Option<Option<String>> {
            if node["focused"] == true {
                return Some(node["app_id"].as_str().map(str::to_owned));
            }
            ["nodes", "floating_nodes"]
                .iter()
                .filter_map(|key| node[*key].as_array())
                .flatten()
                .find_map(search)
        }
        Ok(search(&swaymsg(&["-t", "get_tree"])?).flatten())
    }

    fn synthetic_media() -> MediaOptions {
        use junto_media::audio::synthetic::SyntheticHost;
        use junto_media::camera::synthetic::SyntheticCamera;
        use junto_media::permission::FixedPermission;
        use junto_media::share::synthetic::SyntheticCapture;
        use std::sync::Arc;
        MediaOptions {
            camera: Arc::new(SyntheticCamera::new()),
            host: Arc::new(SyntheticHost::with_tone(440.0)),
            permission: Arc::new(FixedPermission::allowed()),
            capture: Arc::new(SyntheticCapture::new()),
            timing: CallTiming::default(),
        }
    }

    /// The notification service, on a thread of its own: each notification is pressed at
    /// once, with a token that the compositor issued, as a service sends it (version 1.2 of
    /// the specification). It runs until the process ends.
    fn notification_service() -> Result<(), String> {
        let (ready, started) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("notification-service".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                runtime.block_on(serve(ready));
            })
            .map_err(|error| error.to_string())?;
        match started.recv_timeout(PATIENCE) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(failure)) => Err(format!("the notification service did not start: {failure}")),
            Err(_) => Err("the notification service did not start in time".into()),
        }
    }

    struct Service {
        notified: mpsc::UnboundedSender<u32>,
        next: std::sync::atomic::AtomicU32,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Service {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            _app_name: &str,
            replaces_id: u32,
            _app_icon: &str,
            _summary: &str,
            _body: &str,
            _actions: Vec<String>,
            _hints: HashMap<String, OwnedValue>,
            _expire_timeout: i32,
        ) -> u32 {
            let id = if replaces_id == 0 {
                self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            } else {
                replaces_id
            };
            let _ = self.notified.send(id);
            id
        }

        fn close_notification(&self, _id: u32) {}

        fn get_capabilities(&self) -> Vec<String> {
            vec!["actions".into(), "body".into()]
        }
    }

    async fn serve(ready: std::sync::mpsc::Sender<Result<(), String>>) {
        let (notified, mut notifications) = mpsc::unbounded_channel();
        let service = Service {
            notified,
            next: std::sync::atomic::AtomicU32::new(1),
        };
        let connection = async {
            zbus::connection::Builder::session()?
                .name(SERVICE)?
                .serve_at(OBJECT, service)?
                .build()
                .await
        };
        let connection = match connection.await {
            Ok(connection) => connection,
            Err(error) => {
                let _ = ready.send(Err(error.to_string()));
                return;
            }
        };
        let _ = ready.send(Ok(()));
        while let Some(id) = notifications.recv().await {
            // A service sends the press after the notification is drawn; the application
            // has the identifier of the notification by then.
            tokio::time::sleep(Duration::from_millis(500)).await;
            let token = match tokio::task::spawn_blocking(activation_token).await {
                Ok(Ok(token)) => token,
                Ok(Err(failure)) => {
                    eprintln!("notification_focus: the compositor gave no token: {failure}");
                    continue;
                }
                Err(error) => {
                    eprintln!("notification_focus: the token request ended: {error}");
                    continue;
                }
            };
            let signal = |member: &'static str, body: (u32, String)| {
                let connection = connection.clone();
                async move {
                    connection
                        .emit_signal(None::<&str>, OBJECT, SERVICE, member, &body)
                        .await
                }
            };
            let sent = async {
                signal("ActivationToken", (id, token)).await?;
                signal("ActionInvoked", (id, "default".to_owned())).await
            };
            if let Err(error) = sent.await {
                eprintln!("notification_focus: the press was not sent: {error}");
            }
        }
    }

    /// An activation token of the compositor, asked on a Wayland connection of its own as a
    /// notification service asks for one.
    fn activation_token() -> Result<String, String> {
        use wayland_client::globals::{GlobalListContents, registry_queue_init};
        use wayland_client::protocol::wl_registry;
        use wayland_client::{Connection, Dispatch, QueueHandle};
        use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
            self, XdgActivationTokenV1,
        };
        use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

        #[derive(Default)]
        struct Asked {
            token: Option<String>,
        }
        impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Asked {
            fn event(
                _: &mut Self,
                _: &wl_registry::WlRegistry,
                _: wl_registry::Event,
                _: &GlobalListContents,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
        impl Dispatch<XdgActivationV1, ()> for Asked {
            fn event(
                _: &mut Self,
                _: &XdgActivationV1,
                _: <XdgActivationV1 as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
        impl Dispatch<XdgActivationTokenV1, ()> for Asked {
            fn event(
                asked: &mut Self,
                _: &XdgActivationTokenV1,
                event: xdg_activation_token_v1::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let xdg_activation_token_v1::Event::Done { token } = event {
                    asked.token = Some(token);
                }
            }
        }

        let connection = Connection::connect_to_env().map_err(|error| error.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<Asked>(&connection).map_err(|error| error.to_string())?;
        let handle = queue.handle();
        let activation: XdgActivationV1 = globals
            .bind(&handle, 1..=1, ())
            .map_err(|error| format!("the compositor has no activation: {error}"))?;
        let request = activation.get_activation_token(&handle, ());
        request.commit();
        let mut asked = Asked::default();
        // The compositor answers the commit in the same round trip.
        for _ in 0..10 {
            queue
                .roundtrip(&mut asked)
                .map_err(|error| error.to_string())?;
            if let Some(token) = asked.token.take() {
                return Ok(token);
            }
        }
        Err("the compositor did not answer the token request".into())
    }
}
