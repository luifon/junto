#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};

use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceId};
use junto_media::calls::{CallTarget, Seat};
use zbus::names::InterfaceName;
use zbus::zvariant::Value;

use super::*;
use crate::state::view::MicrophoneAction;
use crate::tray::CallMarks;

const ITEM: &str = "org.kde.StatusNotifierItem";

struct Bus {
    process: Child,
    address: String,
    _directory: tempfile::TempDir,
}

impl Bus {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("junto-tray-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("bus");
        let mut process = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}", socket.display()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        // The daemon prints its address when it listens.
        let mut printed = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut printed)
            .unwrap();
        Self {
            process,
            address: format!("unix:path={}", socket.display()),
            _directory: directory,
        }
    }

    /// The worker of the application on this bus, as `StatusNotifierTray` starts it.
    fn worker(
        &self,
        states: mpsc::UnboundedReceiver<TrayState>,
        presses: Presses,
    ) -> tokio::task::JoinHandle<()> {
        let address = self.address.clone();
        tokio::spawn(worker(
            move || Builder::address(address.as_str()),
            states,
            presses,
        ))
    }

    async fn client(&self) -> Connection {
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

/// A watcher of a desktop that only keeps the names of the items that registered.
struct Watcher {
    registered: mpsc::UnboundedSender<String>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String) {
        self.registered.send(service).unwrap();
    }
}

async fn watcher(bus: &Bus) -> (Connection, mpsc::UnboundedReceiver<String>) {
    let (registered, receive) = mpsc::unbounded_channel();
    let connection = Builder::address(bus.address.as_str())
        .unwrap()
        .name(WATCHER)
        .unwrap()
        .serve_at(WATCHER_OBJECT, Watcher { registered })
        .unwrap()
        .build()
        .await
        .unwrap();
    (connection, receive)
}

async fn next<T>(receive: &mut mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), receive.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn pressed(presses: &mut futures_channel::mpsc::UnboundedReceiver<TrayAction>) -> TrayAction {
    tokio::time::timeout(Duration::from_secs(20), presses.next())
        .await
        .unwrap()
        .unwrap()
}

fn seat() -> Seat {
    Seat {
        target: CallTarget {
            server_id: ServerId::generate(),
            place: CallPlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id: ChannelId::generate(),
            },
        },
        identity: "seat".to_owned(),
    }
}

fn in_call(muted: bool, seat: &Seat) -> TrayState {
    TrayState {
        call: Some(CallMarks {
            muted,
            held: false,
            deafened: false,
            seat: seat.clone(),
        }),
    }
}

fn fields(value: &Value<'_>) -> Vec<Value<'static>> {
    match value {
        Value::Structure(structure) => structure
            .fields()
            .iter()
            .map(|field| Value::from(field.try_to_owned().unwrap()))
            .collect(),
        Value::Value(inner) => fields(inner),
        other => panic!("not a structure: {other:?}"),
    }
}

fn items(value: &Value<'_>) -> Vec<Value<'static>> {
    match value {
        Value::Array(array) => array
            .iter()
            .map(|item| Value::from(item.try_to_owned().unwrap()))
            .collect(),
        Value::Value(inner) => items(inner),
        other => panic!("not an array: {other:?}"),
    }
}

fn text(value: &Value<'_>) -> String {
    match value {
        Value::Str(text) => text.to_string(),
        Value::ObjectPath(path) => path.to_string(),
        Value::Value(inner) => text(inner),
        other => panic!("not a text: {other:?}"),
    }
}

fn number(value: &Value<'_>) -> i32 {
    match value {
        Value::I32(number) => *number,
        Value::Value(inner) => number(inner),
        other => panic!("not a number: {other:?}"),
    }
}

fn bytes(value: &Value<'_>) -> Vec<u8> {
    items(value)
        .iter()
        .map(|byte| match byte {
            Value::U8(byte) => *byte,
            other => panic!("not a byte: {other:?}"),
        })
        .collect()
}

/// A property of the item, read from the item at each call.
async fn property(client: &Connection, name: &str, property: &str) -> Value<'static> {
    let properties = zbus::fdo::PropertiesProxy::builder(client)
        .destination(name.to_owned())
        .unwrap()
        .path(ITEM_OBJECT)
        .unwrap()
        .build()
        .await
        .unwrap();
    let value = properties
        .get(InterfaceName::from_static_str_unchecked(ITEM), property)
        .await
        .unwrap();
    Value::from(value)
}

async fn tooltip(client: &Connection, name: &str) -> String {
    text(&fields(&property(client, name, "ToolTip").await)[3])
}

/// The labels of the entries of the menu that the item gives, with their identifiers.
async fn labels(menu: &Proxy<'_>) -> Vec<(i32, String)> {
    let (_, (_, _, children)): (u32, Layout) = menu
        .call("GetLayout", &(0_i32, -1_i32, Vec::<String>::new()))
        .await
        .unwrap();
    children
        .into_iter()
        .filter_map(|child| {
            let child = fields(&Value::from(child));
            let id = number(&child[0]);
            let Value::Dict(properties) = &child[1] else {
                panic!("not a dictionary: {:?}", child[1]);
            };
            properties
                .iter()
                .find(|(key, _)| text(key) == "label")
                .map(|(_, label)| (id, text(label)))
        })
        .collect()
}

#[tokio::test]
async fn the_item_registers_shows_the_call_and_takes_presses_of_its_menu() {
    let bus = Bus::new();
    let (_watcher, mut registered) = watcher(&bus).await;
    let (states, receive) = mpsc::unbounded_channel();
    let (presses, mut presses_seen) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, presses);
    let name = next(&mut registered).await;
    assert!(name.starts_with("org.kde.StatusNotifierItem-"), "{name}");

    let client = bus.client().await;
    assert_eq!(
        tooltip(&client, &name).await,
        crate::text::tray::NOT_IN_A_CALL
    );
    assert_eq!(text(&property(&client, &name, "Menu").await), MENU_OBJECT);
    let item = Proxy::new(&client, name.clone(), ITEM_OBJECT, ITEM)
        .await
        .unwrap();
    let menu = Proxy::new(&client, name.clone(), MENU_OBJECT, "com.canonical.dbusmenu")
        .await
        .unwrap();
    assert_eq!(
        labels(&menu).await,
        [(4, "Open Junto".to_owned()), (5, "Quit".to_owned())]
    );

    // A call with a closed microphone: the picture, the text and the menu follow.
    let seat = seat();
    let mut icons = item.receive_signal("NewIcon").await.unwrap();
    states.send(in_call(true, &seat)).unwrap();
    tokio::time::timeout(Duration::from_secs(20), icons.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tooltip(&client, &name).await,
        crate::text::tray::IN_A_CALL_MUTED
    );
    let pixmaps = items(&property(&client, &name, "IconPixmap").await);
    let sides: Vec<i32> = renderings(Picture::Muted)
        .map(|(side, _)| side as i32)
        .collect();
    assert_eq!(
        pixmaps
            .iter()
            .map(|pixmap| number(&fields(pixmap)[0]))
            .collect::<Vec<_>>(),
        sides
    );
    for pixmap in &pixmaps {
        let pixmap = fields(pixmap);
        let side = number(&pixmap[0]);
        assert_eq!(number(&pixmap[1]), side);
        let argb = bytes(&pixmap[2]);
        assert_eq!(argb.len(), (side * side * 4) as usize);
        // ARGB in network order: the corner outside the rounded body is transparent, and a
        // point inside the body is opaque.
        assert_eq!(argb[0], 0);
        let inside = ((side / 2) * side + side / 4) as usize * 4;
        assert_eq!(argb[inside], 255);
    }
    assert_eq!(
        labels(&menu).await,
        [
            (1, "Unmute microphone".to_owned()),
            (2, "Turn sound off".to_owned()),
            (3, "Leave the call".to_owned()),
            (4, "Open Junto".to_owned()),
            (5, "Quit".to_owned()),
        ]
    );

    // A press on an entry is the action of that entry, with the seat of the call.
    let _: () = menu
        .call("Event", &(1_i32, "clicked", Value::from(0_i32), 0_u32))
        .await
        .unwrap();
    assert_eq!(
        pressed(&mut presses_seen).await,
        TrayAction::Microphone(MicrophoneAction::Open(Some(seat)))
    );
    // An event other than a press, and an identifier of no entry, do nothing.
    let _: () = menu
        .call("Event", &(3_i32, "hovered", Value::from(0_i32), 0_u32))
        .await
        .unwrap();
    let unknown: Vec<i32> = menu
        .call(
            "EventGroup",
            &(vec![
                (99_i32, "clicked", Value::from(0_i32), 0_u32),
                (3_i32, "clicked", Value::from(0_i32), 0_u32),
            ],),
        )
        .await
        .unwrap();
    assert_eq!(unknown, [99]);
    assert_eq!(pressed(&mut presses_seen).await, TrayAction::Leave);
    // The primary press on the icon opens the application.
    let _: () = item.call("Activate", &(0_i32, 0_i32)).await.unwrap();
    assert_eq!(pressed(&mut presses_seen).await, TrayAction::Open);

    drop(states);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn without_a_watcher_nothing_fails_and_a_later_watcher_gets_the_item() {
    let bus = Bus::new();
    let (states, receive) = mpsc::unbounded_channel();
    let (presses, _presses_seen) = futures_channel::mpsc::unbounded();
    let worker = bus.worker(receive, presses);
    states.send(in_call(false, &seat())).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!worker.is_finished());
    let (_watcher, mut registered) = watcher(&bus).await;
    let name = next(&mut registered).await;
    let client = bus.client().await;
    assert_eq!(tooltip(&client, &name).await, crate::text::tray::IN_A_CALL);
    drop(states);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap();
}
