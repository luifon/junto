#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::mpsc;
use zbus::message::Header;

use super::*;

struct Bus {
    child: Child,
    address: String,
}
impl Bus {
    fn new() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            child,
            address: address.trim().into(),
        }
    }
    async fn connection(&self) -> Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .method_timeout(WAIT)
            .build()
            .await
            .unwrap()
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
enum Event {
    Select(u32, bool, u32, u32),
    /// The start, with the window over which the dialog shows.
    Start(String),
    CloseRequest,
    CloseSession,
    Remote,
}

struct State {
    events: mpsc::UnboundedSender<Event>,
    mode: AtomicU32,
}
/// The portal never answers the request that selects the sources.
const SILENT_SELECTION: u32 = 5;
/// The portal process ends while the person's dialog is open.
const ENDS_DURING_START: u32 = 6;
struct ScreenCast(Arc<State>);
struct Session(Arc<State>);
struct Request(Arc<State>);

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    fn close(&self) {
        self.0.events.send(Event::CloseSession).unwrap();
    }
}
#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl Request {
    fn close(&self) {
        self.0.events.send(Event::CloseRequest).unwrap();
    }
}

fn path(header: &Header<'_>, category: &str, token: &str) -> OwnedObjectPath {
    let name = header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    format!("/org/freedesktop/portal/desktop/{category}/{name}/{token}")
        .try_into()
        .unwrap()
}

async fn response(
    connection: &Connection,
    request: &OwnedObjectPath,
    code: u32,
    results: HashMap<&str, Value<'_>>,
) {
    connection
        .emit_signal(
            None::<&str>,
            request.clone(),
            REQUEST,
            "Response",
            &(code, results),
        )
        .await
        .unwrap();
}

#[zbus::interface(name = "org.freedesktop.portal.ScreenCast")]
impl ScreenCast {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        6
    }
    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        3
    }

    async fn create_session(
        &self,
        options: Results,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> OwnedObjectPath {
        let handle = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
        let token = <&str>::try_from(options.get("session_handle_token").unwrap()).unwrap();
        let request = path(&header, "request", handle);
        let session = path(&header, "session", token);
        connection
            .object_server()
            .at(session.clone(), Session(self.0.clone()))
            .await
            .unwrap();
        response(
            connection,
            &request,
            0,
            HashMap::from([("session_handle", Value::from(session.as_str()))]),
        )
        .await;
        request
    }

    async fn select_sources(
        &self,
        session_handle: OwnedObjectPath,
        options: Results,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> OwnedObjectPath {
        assert!(session_handle.as_str().contains("/session/"));
        let request = path(
            &header,
            "request",
            <&str>::try_from(options.get("handle_token").unwrap()).unwrap(),
        );
        self.0
            .events
            .send(Event::Select(
                u32::try_from(options.get("types").unwrap()).unwrap(),
                bool::try_from(options.get("multiple").unwrap()).unwrap(),
                u32::try_from(options.get("cursor_mode").unwrap()).unwrap(),
                u32::try_from(options.get("persist_mode").unwrap()).unwrap(),
            ))
            .unwrap();
        if self.0.mode.load(Ordering::Relaxed) != SILENT_SELECTION {
            response(connection, &request, 0, HashMap::new()).await;
        }
        request
    }

    async fn start(
        &self,
        session_handle: OwnedObjectPath,
        parent_window: &str,
        options: Results,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> OwnedObjectPath {
        assert!(session_handle.as_str().contains("/session/"));
        let request = path(
            &header,
            "request",
            <&str>::try_from(options.get("handle_token").unwrap()).unwrap(),
        );
        connection
            .object_server()
            .at(request.clone(), Request(self.0.clone()))
            .await
            .unwrap();
        self.0
            .events
            .send(Event::Start(parent_window.to_owned()))
            .unwrap();
        let mode = self.0.mode.load(Ordering::Relaxed);
        if mode == 0 {
            let streams = vec![(
                42_u32,
                HashMap::from([
                    ("pipewire-serial", Value::from(123_456_u64)),
                    ("size", Value::from((320_i32, 240_i32))),
                ]),
            )];
            response(
                connection,
                &request,
                0,
                HashMap::from([("streams", Value::new(streams))]),
            )
            .await;
        } else if mode == 1 {
            response(connection, &request, 1, HashMap::new()).await;
        } else if mode == 4 {
            response(connection, &request, 2, HashMap::new()).await;
        } else if mode == ENDS_DURING_START {
            connection.release_name(SERVICE).await.unwrap();
        } else if mode == 3 {
            connection
                .emit_signal(
                    None::<&str>,
                    session_handle,
                    SESSION,
                    "Closed",
                    &(HashMap::<&str, Value<'_>>::new(),),
                )
                .await
                .unwrap();
        }
        request
    }

    fn open_pipe_wire_remote(
        &self,
        session_handle: OwnedObjectPath,
        options: Results,
    ) -> zbus::zvariant::OwnedFd {
        assert!(session_handle.as_str().contains("/session/"));
        assert!(options.is_empty());
        self.0.events.send(Event::Remote).unwrap();
        let (socket, _other) = std::os::unix::net::UnixStream::pair().unwrap();
        let fd: OwnedFd = socket.into();
        fd.into()
    }
}

async fn setup(bus: &Bus, mode: u32) -> (Connection, Portal, mpsc::UnboundedReceiver<Event>) {
    let (events, receive) = mpsc::unbounded_channel();
    let state = Arc::new(State {
        events,
        mode: AtomicU32::new(mode),
    });
    let service = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(OBJECT, ScreenCast(state))
        .unwrap()
        .build()
        .await
        .unwrap();
    let portal = Portal {
        connection: bus.connection().await,
        session: None,
        pending: std::sync::Mutex::new(None),
        limit: Duration::from_secs(1),
    };
    (service, portal, receive)
}

async fn event(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(30), events.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn the_portal_keeps_the_explicit_kind_and_receives_an_early_response() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, 0).await;
    assert_eq!(
        portal.capabilities().await.unwrap().kinds,
        [ShareKind::Screen, ShareKind::Window]
    );
    let (video, _closed) = portal
        .start(ShareKind::Window, "x11:2a00004")
        .await
        .unwrap();
    assert_eq!(video.node, 42);
    assert_eq!(video.serial, Some(123_456));
    assert!(std::fs::File::from(video.fd).metadata().is_ok());
    assert!(matches!(
        event(&mut events).await,
        Event::Select(2, false, 2, 0)
    ));
    // The dialog of the desktop shows over the window of the client.
    assert!(matches!(
        event(&mut events).await,
        Event::Start(parent) if parent == "x11:2a00004"
    ));
    assert!(matches!(event(&mut events).await, Event::Remote));
    drop(portal);
    assert!(matches!(event(&mut events).await, Event::CloseSession));
}

#[tokio::test]
async fn cancellation_never_opens_the_video_remote() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, 1).await;
    assert!(matches!(
        portal.start(ShareKind::Screen, "").await,
        Err(CaptureError::Cancelled)
    ));
    assert!(matches!(
        event(&mut events).await,
        Event::Select(1, false, 2, 0)
    ));
    assert!(matches!(event(&mut events).await, Event::Start(_)));
    drop(portal);
    assert!(matches!(event(&mut events).await, Event::CloseSession));
}

#[tokio::test]
async fn a_backend_failure_is_not_a_permission_refusal() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, 4).await;
    let result = portal.start(ShareKind::Screen, "").await;
    assert!(matches!(
        event(&mut events).await,
        Event::Select(1, false, 2, 0)
    ));
    assert!(matches!(event(&mut events).await, Event::Start(_)));
    drop(portal);
    assert!(matches!(event(&mut events).await, Event::CloseSession));
    assert!(matches!(result, Err(CaptureError::Failed(_))));
}

#[tokio::test]
async fn dropping_a_pending_chooser_closes_its_request_and_session() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, 2).await;
    let mut pending = Box::pin(portal.start(ShareKind::Screen, ""));
    tokio::select! {
        result = &mut pending => panic!("the chooser unexpectedly finished: {}", result.is_ok()),
        _ = async {
            assert!(matches!(event(&mut events).await, Event::Select(1, false, 2, 0)));
            assert!(matches!(event(&mut events).await, Event::Start(_)));
        } => {}
    }
    drop(pending);
    drop(portal);
    assert!(matches!(event(&mut events).await, Event::CloseRequest));
    assert!(matches!(event(&mut events).await, Event::CloseSession));
}

#[tokio::test]
async fn a_revoked_session_ends_even_when_the_chooser_has_not_responded() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, 3).await;
    let result = tokio::time::timeout(Duration::from_secs(30), portal.start(ShareKind::Screen, ""))
        .await
        .unwrap();
    assert!(matches!(result, Err(CaptureError::SourceGone)));
    assert!(matches!(
        event(&mut events).await,
        Event::Select(1, false, 2, 0)
    ));
    assert!(matches!(event(&mut events).await, Event::Start(_)));
    drop(portal);
    assert!(matches!(event(&mut events).await, Event::CloseRequest));
    assert!(matches!(event(&mut events).await, Event::CloseSession));
}

#[tokio::test]
async fn a_portal_that_does_not_answer_a_request_without_a_dialog_fails_the_start() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, SILENT_SELECTION).await;
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(30), portal.start(ShareKind::Screen, ""))
        .await
        .unwrap();
    assert!(
        matches!(&result, Err(CaptureError::Failed(reason)) if reason.contains("SelectSources")),
        "{:?}",
        result.err()
    );
    // The limit of the test is one second; the person's dialog was never reached.
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(matches!(
        event(&mut events).await,
        Event::Select(1, false, 2, 0)
    ));
    drop(portal);
    assert!(matches!(event(&mut events).await, Event::CloseSession));
}

#[tokio::test]
async fn a_portal_that_ends_during_the_dialog_fails_the_start() {
    let bus = Bus::new();
    let (_service, mut portal, mut events) = setup(&bus, ENDS_DURING_START).await;
    let result = tokio::time::timeout(Duration::from_secs(30), portal.start(ShareKind::Screen, ""))
        .await
        .unwrap();
    assert!(
        matches!(&result, Err(CaptureError::Failed(reason)) if reason.contains("ended")),
        "{:?}",
        result.err()
    );
    assert!(matches!(
        event(&mut events).await,
        Event::Select(1, false, 2, 0)
    ));
    assert!(matches!(event(&mut events).await, Event::Start(_)));
}
