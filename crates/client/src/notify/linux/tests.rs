#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::zvariant::OwnedValue;

use super::*;

struct Bus {
    process: Child,
    address: String,
    directory: std::sync::Arc<tempfile::TempDir>,
}

impl Bus {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("junto-notify-")
            .tempdir_in("/tmp")
            .unwrap();
        Self::at(std::sync::Arc::new(directory))
    }

    /// A bus daemon whose address is the socket in this directory.
    fn at(directory: std::sync::Arc<tempfile::TempDir>) -> Self {
        let socket = directory.path().join("bus");
        let _ = std::fs::remove_file(&socket);
        let mut process = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}", socket.display()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        // The daemon prints its address when it listens. The printed address names this
        // one daemon; the address of the socket alone also reaches its successor.
        let mut printed = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut printed)
            .unwrap();
        Self {
            process,
            address: format!("unix:path={}", socket.display()),
            directory,
        }
    }

    /// The worker of the application on this bus, as `DesktopNotifier` starts it.
    fn worker(
        &self,
        commands: mpsc::UnboundedReceiver<Command>,
        actions: futures_channel::mpsc::UnboundedSender<Press>,
    ) -> tokio::task::JoinHandle<()> {
        let address = self.address.clone();
        tokio::spawn(worker(move || connect(address.clone()), commands, actions))
    }
}

async fn connect(address: String) -> zbus::Result<Connection> {
    zbus::connection::Builder::address(address.as_str())?
        .method_timeout(WAIT)
        .build()
        .await
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[derive(Debug)]
enum Told {
    Show {
        replaced: u32,
        title: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
    },
    Close(u32),
}

struct Service {
    told: mpsc::UnboundedSender<Told>,
    next: AtomicU32,
    /// What the service answers about itself. `None`: it does not answer the question.
    capabilities: Option<&'static [&'static str]>,
}

const MARKUP: Option<&[&str]> = Some(&["actions", "body", "body-markup"]);
const PLAIN: Option<&[&str]> = Some(&["actions", "body"]);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Service {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        assert_eq!(app_name, "Junto");
        assert_eq!(app_icon, "");
        assert_eq!(expire_timeout, -1);
        self.told
            .send(Told::Show {
                replaced: replaces_id,
                title: summary.to_owned(),
                body: body.to_owned(),
                actions,
                hints,
            })
            .unwrap();
        if replaces_id == 0 {
            self.next.fetch_add(1, Ordering::Relaxed)
        } else {
            replaces_id
        }
    }

    fn close_notification(&self, id: u32) {
        self.told.send(Told::Close(id)).unwrap();
    }

    async fn get_capabilities(&self) -> Vec<String> {
        let Some(capabilities) = self.capabilities else {
            tokio::time::sleep(Duration::from_secs(120)).await;
            return Vec::new();
        };
        capabilities.iter().map(|name| (*name).to_owned()).collect()
    }
}

async fn service(bus: &Bus) -> (Connection, mpsc::UnboundedReceiver<Told>) {
    service_with(bus, MARKUP).await
}

async fn service_with(
    bus: &Bus,
    capabilities: Option<&'static [&'static str]>,
) -> (Connection, mpsc::UnboundedReceiver<Told>) {
    let (told, receive) = mpsc::unbounded_channel();
    let connection = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(
            OBJECT,
            Service {
                told,
                next: AtomicU32::new(7),
                capabilities,
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    (connection, receive)
}

fn alert(tag: &str) -> Alert {
    Alert {
        tag: tag.to_owned(),
        title: "A message".into(),
        body: "<b> & friends".into(),
    }
}

fn show(tag: &str) -> Command {
    Command::Show(alert(tag), Instant::now())
}

async fn told(receive: &mut mpsc::UnboundedReceiver<Told>) -> Told {
    tokio::time::timeout(Duration::from_secs(20), receive.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn action(connection: &Connection, id: u32, action: &str) {
    connection
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "ActionInvoked",
            &(id, action),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn the_service_replaces_dismisses_and_activates_only_known_notifications() {
    let bus = Bus::new();
    let (service, mut seen) = service(&bus).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    send.send(show("one")).unwrap();
    match told(&mut seen).await {
        Told::Show {
            replaced,
            title,
            body,
            actions,
            hints,
        } => {
            assert_eq!(replaced, 0);
            assert_eq!(title, "A message");
            assert_eq!(body, "&lt;b&gt; &amp; friends");
            assert_eq!(actions, ["default", "Open"]);
            assert_eq!(
                <&str>::try_from(hints.get("desktop-entry").unwrap()).unwrap(),
                crate::app::APP_ID
            );
            assert!(bool::try_from(hints.get("suppress-sound").unwrap()).unwrap());
        }
        other => panic!("{other:?}"),
    }
    send.send(show("one")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 7, .. }
    ));
    action(&service, 999, "default").await;
    action(&service, 7, "unrelated").await;
    action(&service, 7, "default").await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), pressed.next())
            .await
            .unwrap()
            .map(|press| press.tag)
            .as_deref(),
        Some("one")
    );
    send.send(Command::Dismiss("one".into())).unwrap();
    assert!(matches!(told(&mut seen).await, Told::Close(7)));
    send.send(show("two")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    action(&service, 7, "default").await;
    action(&service, 8, "default").await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), pressed.next())
            .await
            .unwrap()
            .map(|press| press.tag)
            .as_deref(),
        Some("two")
    );
    service
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "NotificationClosed",
            &(8_u32, 2_u32),
        )
        .await
        .unwrap();
    send.send(show("three")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    action(&service, 8, "default").await;
    action(&service, 9, "default").await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), pressed.next())
            .await
            .unwrap()
            .map(|press| press.tag)
            .as_deref(),
        Some("three")
    );
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(pressed.next().await.is_none());
}

async fn token(connection: &Connection, id: u32, token: &str) {
    connection
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "ActivationToken",
            &(id, token),
        )
        .await
        .unwrap();
}

/// The service sends the activation token of a press before its action (version 1.2 of the
/// specification); the press carries it, so that a Wayland compositor gives the window the
/// focus. A token goes only with the press of its own notification and only once.
#[tokio::test]
async fn a_press_carries_the_activation_token_of_its_notification() {
    let bus = Bus::new();
    let (service, mut seen) = service(&bus).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let _worker = bus.worker(receive, actions);
    send.send(show("one")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    send.send(show("two")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    let next = async |pressed: &mut futures_channel::mpsc::UnboundedReceiver<Press>| {
        tokio::time::timeout(Duration::from_secs(20), pressed.next())
            .await
            .unwrap()
            .unwrap()
    };
    token(&service, 999, "token-unknown").await;
    token(&service, 8, "token-of-two").await;
    action(&service, 7, "default").await;
    assert_eq!(
        next(&mut pressed).await,
        Press {
            tag: "one".into(),
            token: None
        }
    );
    action(&service, 8, "default").await;
    assert_eq!(
        next(&mut pressed).await,
        Press {
            tag: "two".into(),
            token: Some("token-of-two".into())
        }
    );
    // A token of a press on another action of the notification is not kept.
    token(&service, 7, "token-of-another-action").await;
    action(&service, 7, "unrelated").await;
    action(&service, 7, "default").await;
    assert_eq!(
        next(&mut pressed).await,
        Press {
            tag: "one".into(),
            token: None
        }
    );
}

#[tokio::test]
async fn a_new_service_owner_never_receives_an_old_notification_identifier() {
    let bus = Bus::new();
    let (old, mut seen) = service(&bus).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    send.send(show("old")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    old.release_name(SERVICE).await.unwrap();
    let (new, mut seen) = service(&bus).await;
    send.send(Command::Dismiss("old".into())).unwrap();
    send.send(show("new")).unwrap();
    // A Close(7) here would withdraw an unrelated notification from the new service.
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    action(&old, 7, "default").await;
    action(&new, 7, "default").await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), pressed.next())
            .await
            .unwrap()
            .map(|press| press.tag)
            .as_deref(),
        Some("new")
    );
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_missing_desktop_service_does_not_prevent_worker_shutdown() {
    let bus = Bus::new();
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    send.send(show("one")).unwrap();
    send.send(Command::Dismiss("one".into())).unwrap();
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(pressed.next().await.is_none());
}

#[tokio::test]
async fn a_service_without_markup_receives_the_text_as_it_was_written() {
    let bus = Bus::new();
    let (_service, mut seen) = service_with(&bus, PLAIN).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, _pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    send.send(show("one")).unwrap();
    match told(&mut seen).await {
        Told::Show { body, .. } => assert_eq!(body, "<b> & friends"),
        other => panic!("{other:?}"),
    }
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn the_worker_serves_a_new_session_bus_at_the_same_address() {
    let bus = Bus::new();
    let (first, mut seen) = service(&bus).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    send.send(show("before")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    // The session bus ends, and another one takes its address.
    let directory = bus.directory.clone();
    drop(first);
    drop(bus);
    let bus = Bus::at(directory);
    let (second, mut seen) = service(&bus).await;
    // The alert is asked for while the worker has no connection. It has the tag of the
    // alert that the first bus showed: an identifier of that bus must not be used here.
    send.send(show("before")).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    action(&second, 7, "default").await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), pressed.next())
            .await
            .unwrap()
            .map(|press| press.tag)
            .as_deref(),
        Some("before")
    );
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_alert_that_waited_too_long_for_the_service_is_not_shown() {
    let bus = Bus::new();
    let (_service, mut seen) = service(&bus).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, _pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    let mut old = alert("old");
    old.title = "An old message".into();
    let asked = Instant::now()
        .checked_sub(STALE + Duration::from_secs(1))
        .unwrap();
    send.send(Command::Show(old, asked)).unwrap();
    send.send(show("fresh")).unwrap();
    match told(&mut seen).await {
        Told::Show { title, .. } => assert_eq!(title, "A message"),
        other => panic!("{other:?}"),
    }
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(seen.try_recv().is_err());
}

#[tokio::test]
async fn a_command_that_a_lost_connection_interrupted_is_served_first() {
    let (send, commands) = mpsc::unbounded_channel();
    let mut queue = Queue {
        commands,
        waiting: VecDeque::new(),
    };
    let mut later = alert("one");
    later.title = "The later text".into();
    // "one" was being shown when the connection was lost; its newer text arrived after.
    queue.keep(Command::Show(later, Instant::now()));
    queue.put_back(show("one"));
    queue.put_back(Command::Dismiss("other".into()));
    drop(send);
    let mut titles = Vec::new();
    while let Some(command) = queue.next().await {
        match command {
            Command::Show(alert, _) => titles.push(alert.title),
            Command::Dismiss(tag) => panic!("{tag}"),
        }
    }
    assert_eq!(titles, ["A message", "The later text"]);
    // A dismissal that arrives without a connection takes back the alert that waits.
    let (_send, commands) = mpsc::unbounded_channel();
    let mut queue = Queue {
        commands,
        waiting: VecDeque::new(),
    };
    queue.keep(show("one"));
    queue.keep(show("two"));
    queue.keep(Command::Dismiss("one".into()));
    assert!(matches!(
        queue.waiting.make_contiguous(),
        [Command::Show(alert, _)] if alert.tag == "two"
    ));
}

#[tokio::test]
async fn a_service_that_does_not_state_its_capabilities_is_asked_once() {
    let bus = Bus::new();
    let (_service, mut seen) = service_with(&bus, None).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, _pressed) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, actions);
    // The first alert waits for the answer that does not come, and is escaped.
    send.send(show("one")).unwrap();
    match told(&mut seen).await {
        Told::Show { body, .. } => assert_eq!(body, "&lt;b&gt; &amp; friends"),
        other => panic!("{other:?}"),
    }
    // The second alert does not wait again: the wait for an answer is 5 seconds.
    let asked = Instant::now();
    send.send(show("two")).unwrap();
    assert!(matches!(told(&mut seen).await, Told::Show { .. }));
    assert!(asked.elapsed() < WAIT / 2, "{:?}", asked.elapsed());
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
}
