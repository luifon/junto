//! The count of the launcher entry of a Linux desktop: the signal `Update` of the interface
//! `com.canonical.Unity.LauncherEntry` on the session bus, which the docks of GNOME (Dash to
//! Dock, Ubuntu Dock), KDE Plasma and other desktops read for the entry of the desktop file
//! of the application. It is sent on a worker of its own, as the tray icon is (ADR 0759).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;
use zbus::connection::Builder;
use zbus::zvariant::Value;

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
const OBJECT: &str = "/chat/junto/Junto/LauncherEntry";
/// The pause before the worker connects to the session bus again.
const RETRY: Duration = Duration::from_secs(5);

/// The entry of the desktop file of the application that the signal names.
fn application() -> String {
    format!(
        "application://{}.desktop",
        junto_client_core::app_identity::APP_ID
    )
}

pub(super) struct Launcher {
    counts: mpsc::UnboundedSender<u64>,
}

impl Launcher {
    pub(super) fn set(&self, count: u64) {
        let _ = self.counts.send(count);
    }
}

/// The launcher entry of the application, started at its first use.
pub(super) fn launcher() -> &'static Launcher {
    static LAUNCHER: OnceLock<Launcher> = OnceLock::new();
    LAUNCHER.get_or_init(|| {
        let (counts, receive) = mpsc::unbounded_channel();
        let started = std::thread::Builder::new()
            .name("launcher-entry".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(worker(Builder::session, receive)),
                    Err(error) => tracing::warn!(%error, "could not start the launcher count"),
                }
            });
        if let Err(error) = started {
            tracing::warn!(%error, "could not start the launcher count");
        }
        Launcher { counts }
    })
}

/// Sends each count that it is given, and the last count again after a new connection.
async fn worker(
    connect: impl Fn() -> zbus::Result<Builder<'static>>,
    mut counts: mpsc::UnboundedReceiver<u64>,
) {
    let mut count = None;
    let mut reported = false;
    loop {
        let connection = match connect() {
            Ok(builder) => builder.build().await,
            Err(error) => Err(error),
        };
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) => {
                if !reported {
                    tracing::warn!(%error, "the launcher count has no session bus");
                    reported = true;
                }
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        if let Some(count) = count
            && send(&connection, count).await.is_err()
        {
            continue;
        }
        loop {
            let Some(next) = counts.recv().await else {
                return;
            };
            count = Some(next);
            if let Err(error) = send(&connection, next).await {
                tracing::debug!(%error, "the launcher count was not sent");
                break;
            }
        }
    }
}

async fn send(connection: &zbus::Connection, count: u64) -> zbus::Result<()> {
    let properties: HashMap<&str, Value<'_>> = HashMap::from([
        (
            "count",
            Value::from(i64::try_from(count).unwrap_or(i64::MAX)),
        ),
        ("count-visible", Value::from(count > 0)),
    ]);
    connection
        .emit_signal(
            None::<()>,
            OBJECT,
            INTERFACE,
            "Update",
            &(application(), properties),
        )
        .await
}

#[cfg(test)]
mod tests;
