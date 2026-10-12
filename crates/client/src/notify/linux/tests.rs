#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::zvariant::OwnedValue;

use super::*;

struct Bus {
    process: Child,
    address: String,
}

impl Bus {
    fn new() -> Self {
        let mut process = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            process,
            address: address.trim().to_owned(),
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
}

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
}

async fn service(bus: &Bus) -> (Connection, mpsc::UnboundedReceiver<Told>) {
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
    let worker = tokio::spawn(run(bus.connection().await, receive, actions));
    send.send(Command::Show(alert("one"))).unwrap();
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
    send.send(Command::Show(alert("one"))).unwrap();
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
            .as_deref(),
        Some("one")
    );
    send.send(Command::Dismiss("one".into())).unwrap();
    assert!(matches!(told(&mut seen).await, Told::Close(7)));
    send.send(Command::Show(alert("two"))).unwrap();
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
    send.send(Command::Show(alert("three"))).unwrap();
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
            .as_deref(),
        Some("three")
    );
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pressed.next().await.is_none());
}

#[tokio::test]
async fn a_new_service_owner_never_receives_an_old_notification_identifier() {
    let bus = Bus::new();
    let (old, mut seen) = service(&bus).await;
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let worker = tokio::spawn(run(bus.connection().await, receive, actions));
    send.send(Command::Show(alert("old"))).unwrap();
    assert!(matches!(
        told(&mut seen).await,
        Told::Show { replaced: 0, .. }
    ));
    old.release_name(SERVICE).await.unwrap();
    let (new, mut seen) = service(&bus).await;
    send.send(Command::Dismiss("old".into())).unwrap();
    send.send(Command::Show(alert("new"))).unwrap();
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
            .as_deref(),
        Some("new")
    );
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_missing_desktop_service_does_not_prevent_worker_shutdown() {
    let bus = Bus::new();
    let (send, receive) = mpsc::unbounded_channel();
    let (actions, mut pressed) = futures_channel::mpsc::unbounded();
    let worker = tokio::spawn(run(bus.connection().await, receive, actions));
    send.send(Command::Show(alert("one"))).unwrap();
    send.send(Command::Dismiss("one".into())).unwrap();
    drop(send);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pressed.next().await.is_none());
}
