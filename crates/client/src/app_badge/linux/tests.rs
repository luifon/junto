#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};

use futures_util::StreamExt as _;
use zbus::MatchRule;
use zbus::zvariant::OwnedValue;

use super::*;

struct Bus {
    process: Child,
    address: String,
    _directory: tempfile::TempDir,
}

impl Bus {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("junto-launcher-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("bus");
        let mut process = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}", socket.display()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
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
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// The next update that a dock reads: the entry and its count and visibility.
async fn next_update(updates: &mut zbus::MessageStream) -> (String, i64, bool) {
    let message = tokio::time::timeout(Duration::from_secs(10), updates.next())
        .await
        .expect("the dock got no update in 10 seconds")
        .unwrap()
        .unwrap();
    let (entry, properties): (String, HashMap<String, OwnedValue>) =
        message.body().deserialize().unwrap();
    (
        entry,
        i64::try_from(&properties["count"]).unwrap(),
        bool::try_from(&properties["count-visible"]).unwrap(),
    )
}

/// The count reaches a dock that listens on the session bus, under the entry of the desktop
/// file of the application, and a count of none hides it.
#[tokio::test]
async fn the_count_reaches_a_dock_on_the_session_bus() {
    let bus = Bus::new();
    let dock = Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)
        .unwrap()
        .member("Update")
        .unwrap()
        .build();
    let mut updates = zbus::MessageStream::for_match_rule(rule, &dock, None)
        .await
        .unwrap();
    let (counts, receive) = watch::channel(0);
    let address = bus.address.clone();
    let worker = tokio::spawn(worker(move || Builder::address(address.as_str()), receive));
    let entry = "application://chat.junto.Junto.desktop".to_owned();
    counts.send(3).unwrap();
    assert_eq!(next_update(&mut updates).await, (entry.clone(), 3, true));
    counts.send(0).unwrap();
    assert_eq!(next_update(&mut updates).await, (entry, 0, false));
    worker.abort();
}

#[tokio::test]
async fn a_missing_bus_observes_worker_shutdown() {
    let (counts, receive) = watch::channel(0);
    let work = tokio::spawn(worker(
        || Builder::address("unix:path=/nonexistent/junto-test-bus"),
        receive,
    ));
    for count in 0..10_000 {
        counts.send(count).unwrap();
    }
    drop(counts);
    tokio::time::timeout(Duration::from_secs(1), work)
        .await
        .expect("shutdown while disconnected")
        .unwrap();
}

#[tokio::test]
async fn an_unresponsive_bus_has_a_connection_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bus");
    let socket = tokio::net::UnixListener::bind(&path).unwrap();
    let address = format!("unix:path={}", path.display());
    let (counts, receive) = watch::channel(0);
    let work = tokio::spawn(worker(move || Builder::address(address.as_str()), receive));
    let (_connection, _) = tokio::time::timeout(Duration::from_secs(1), socket.accept())
        .await
        .unwrap()
        .unwrap();
    drop(counts);
    tokio::time::timeout(Duration::from_secs(1), work)
        .await
        .expect("shutdown during authentication")
        .unwrap();
}

#[tokio::test]
async fn a_stalled_authentication_is_retried_within_the_connection_bound() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bus");
    let socket = tokio::net::UnixListener::bind(&path).unwrap();
    let address = format!("unix:path={}", path.display());
    let (counts, receive) = watch::channel(0);
    let work = tokio::spawn(worker(move || Builder::address(address.as_str()), receive));
    let (_first, _) = tokio::time::timeout(Duration::from_secs(1), socket.accept())
        .await
        .unwrap()
        .unwrap();
    let (_second, _) =
        tokio::time::timeout(ATTEMPT + RETRY + Duration::from_secs(2), socket.accept())
            .await
            .expect("connection attempt has a deadline")
            .unwrap();
    drop(counts);
    tokio::time::timeout(Duration::from_secs(1), work)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_recovered_bus_gets_only_the_latest_count() {
    let bus = Bus::new();
    let dock = Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)
        .unwrap()
        .member("Update")
        .unwrap()
        .build();
    let mut updates = zbus::MessageStream::for_match_rule(rule, &dock, None)
        .await
        .unwrap();
    let (counts, receive) = watch::channel(0);
    for count in 0..10_000 {
        counts.send(count).unwrap();
    }
    let address = bus.address.clone();
    let work = tokio::spawn(worker(move || Builder::address(address.as_str()), receive));
    assert_eq!(next_update(&mut updates).await.1, 9_999);
    drop(counts);
    tokio::time::timeout(Duration::from_secs(1), work)
        .await
        .unwrap()
        .unwrap();
}
