//! The portal client against a portal of a desktop on a private session bus.

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;

use super::*;

struct Bus {
    process: Child,
    address: String,
    _directory: tempfile::TempDir,
}

impl Bus {
    fn new() -> Self {
        use std::io::BufRead as _;
        let directory = tempfile::Builder::new()
            .prefix("junto-keys-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("bus");
        // A bus without service activation: no portal of the machine starts on it.
        let config = directory.path().join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN\" \
                 \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\
                 <busconfig><type>session</type><listen>unix:path={}</listen>\
                 <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
                 <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
                socket.display()
            ),
        )
        .unwrap();
        let mut process = Command::new("dbus-daemon")
            .args(["--nofork", "--print-address=1"])
            .arg(format!("--config-file={}", config.display()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        // The daemon prints its address when it listens.
        let mut printed = String::new();
        std::io::BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut printed)
            .unwrap();
        Self {
            process,
            address: format!("unix:path={}", socket.display()),
            _directory: directory,
        }
    }

    async fn connect(&self) -> Connection {
        Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// What the desktop was asked: the shortcuts of each binding with their suggested keys.
type Bound = Arc<Mutex<Vec<Vec<(String, String)>>>>;

/// A portal of a desktop that answers each request at once.
struct Portal {
    bound: Bound,
    /// The bindings that the person refuses, by their number from 1.
    refuse: Option<usize>,
}

impl Portal {
    async fn answer(
        connection: &Connection,
        path: &str,
        response: u32,
        results: HashMap<&str, Value<'_>>,
    ) {
        connection
            .emit_signal(
                None::<&str>,
                path,
                "org.freedesktop.portal.Request",
                "Response",
                &(response, results),
            )
            .await
            .unwrap();
    }
}

#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl Portal {
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        let token = String::try_from(options["handle_token"].clone()).unwrap();
        let session = String::try_from(options["session_handle_token"].clone()).unwrap();
        let sender = header
            .sender()
            .unwrap()
            .trim_start_matches(':')
            .replace('.', "_");
        let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let session = format!("/org/freedesktop/portal/desktop/session/{sender}/{session}");
        let connection = connection.clone();
        let path = request.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let results = HashMap::from([("session_handle", Value::from(session))]);
            Portal::answer(&connection, &path, 0, results).await;
        });
        OwnedObjectPath::try_from(request).unwrap()
    }

    async fn bind_shortcuts(
        &self,
        _session_handle: OwnedObjectPath,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        let token = String::try_from(options["handle_token"].clone()).unwrap();
        let sender = header
            .sender()
            .unwrap()
            .trim_start_matches(':')
            .replace('.', "_");
        let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let list: Vec<(String, String)> = shortcuts
            .into_iter()
            .map(|(id, settings)| {
                let trigger = String::try_from(settings["preferred_trigger"].clone()).unwrap();
                (id, trigger)
            })
            .collect();
        let number = {
            let mut bound = self.bound.lock().unwrap();
            bound.push(list);
            bound.len()
        };
        let response = u32::from(self.refuse == Some(number));
        let connection = connection.clone();
        let path = request.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Portal::answer(&connection, &path, response, HashMap::new()).await;
        });
        OwnedObjectPath::try_from(request).unwrap()
    }

    #[zbus(signal)]
    async fn activated(
        emitter: &SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn deactivated(
        emitter: &SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
}

async fn desktop(bus: &Bus, refuse: Option<usize>) -> (Connection, Bound) {
    let bound: Bound = Arc::default();
    let connection = Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at(
            "/org/freedesktop/portal/desktop",
            Portal {
                bound: bound.clone(),
                refuse,
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    (connection, bound)
}

/// Sends a press or a release of a shortcut of the session of the client, as the desktop
/// does.
async fn press(desktop: &Connection, client: &Connection, id: &str, down: bool) {
    let sender = client
        .unique_name()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let session = format!("/org/freedesktop/portal/desktop/session/{sender}/junto_keys_1");
    let emitter = SignalEmitter::new(desktop, "/org/freedesktop/portal/desktop").unwrap();
    let path = ObjectPath::try_from(session.as_str()).unwrap();
    if down {
        Portal::activated(&emitter, path, id, 1, HashMap::new())
            .await
            .unwrap();
    } else {
        Portal::deactivated(&emitter, path, id, 1, HashMap::new())
            .await
            .unwrap();
    }
}

async fn next(events: &mut mpsc::UnboundedReceiver<KeyEvent>) -> KeyEvent {
    tokio::time::timeout(Duration::from_secs(20), events.recv())
        .await
        .unwrap()
        .unwrap()
}

fn keys() -> CallKeys {
    CallKeys {
        talk: Some("alt".parse().unwrap()),
        mute: Some("control+shift+m".parse().unwrap()),
    }
}

#[tokio::test]
async fn the_shortcuts_are_bound_when_a_call_needs_them_and_their_presses_reach_the_call() {
    let bus = Bus::new();
    let (desktop, bound) = desktop(&bus, None).await;
    let client = bus.connect().await;
    let (wanted, watched) = watch::channel(None);
    let (sender, mut events) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(client.clone(), watched, move |event| {
        sender.send(event).unwrap();
    }));
    // No call: nothing is bound.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(bound.lock().unwrap().is_empty());

    wanted.send(Some(keys())).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while bound.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        bound.lock().unwrap()[0],
        [
            (TALK.to_owned(), "ALT".to_owned()),
            (MUTE.to_owned(), "CTRL+SHIFT+m".to_owned())
        ]
    );
    // The binding has its answer before the presses come.
    tokio::time::sleep(Duration::from_millis(200)).await;
    press(&desktop, &client, TALK, true).await;
    assert_eq!(next(&mut events).await, KeyEvent::Talk(true));
    press(&desktop, &client, TALK, false).await;
    assert_eq!(next(&mut events).await, KeyEvent::Talk(false));
    press(&desktop, &client, MUTE, true).await;
    assert_eq!(next(&mut events).await, KeyEvent::ToggleMute);
    press(&desktop, &client, MUTE, false).await;

    // The call ends while the key of push to talk is held: the hold ends with it, and a
    // press outside a call gives nothing.
    press(&desktop, &client, TALK, true).await;
    assert_eq!(next(&mut events).await, KeyEvent::Talk(true));
    wanted.send(None).unwrap();
    assert_eq!(next(&mut events).await, KeyEvent::Talk(false));
    press(&desktop, &client, MUTE, true).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(events.try_recv().is_err());
    // The same keys in the next call are not bound again.
    wanted.send(Some(keys())).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(bound.lock().unwrap().len(), 1);

    drop(wanted);
    tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_refused_binding_ends_the_client_with_its_reason() {
    let bus = Bus::new();
    let (_desktop, _) = desktop(&bus, Some(1)).await;
    let client = bus.connect().await;
    let (wanted, watched) = watch::channel(Some(keys()));
    let ended = tokio::time::timeout(Duration::from_secs(20), run(client, watched, |_| {}))
        .await
        .unwrap();
    assert_eq!(ended, Err(PortalError::Refused));
    drop(wanted);
}

#[tokio::test]
async fn a_desktop_without_the_portal_is_told_as_missing() {
    let bus = Bus::new();
    let client = bus.connect().await;
    let (_wanted, watched) = watch::channel(Some(keys()));
    let ended = tokio::time::timeout(Duration::from_secs(20), run(client, watched, |_| {}))
        .await
        .unwrap();
    assert!(matches!(ended, Err(PortalError::Missing(_))), "{ended:?}");
}

#[test]
fn a_key_of_a_call_is_suggested_in_the_form_of_the_desktop() {
    let combo = |text: &str| text.parse::<KeyCombo>().unwrap();
    assert_eq!(trigger(&combo("alt")), "ALT");
    assert_eq!(trigger(&combo("meta+shift+f13")), "SHIFT+LOGO+F13");
    assert_eq!(trigger(&combo("control+7")), "CTRL+7");
}
