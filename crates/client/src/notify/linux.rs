//! The desktop notification service (ADR 0752).

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use gpui::App;
use tokio::sync::mpsc;
use zbus::zvariant::Value;
use zbus::{Connection, Proxy};

use super::SystemNotifier;
use crate::state::notices::Alert;

const SERVICE: &str = "org.freedesktop.Notifications";
const OBJECT: &str = "/org/freedesktop/Notifications";
const WAIT: Duration = Duration::from_secs(5);

pub(super) struct DesktopNotifier {
    commands: mpsc::UnboundedSender<Command>,
}

enum Command {
    Show(Alert),
    Dismiss(String),
}

impl DesktopNotifier {
    pub(super) fn new(cx: &mut App, pressed: impl Fn(&str, &mut App) + 'static) -> Self {
        let (commands, receive) = mpsc::unbounded_channel();
        let (actions, mut take_actions) = futures_channel::mpsc::unbounded::<String>();
        cx.spawn(async move |cx| {
            while let Some(tag) = take_actions.next().await {
                cx.update(|cx| pressed(&tag, cx));
            }
        })
        .detach();
        let result = std::thread::Builder::new()
            .name("desktop-notifications".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::warn!(%error, "could not start desktop notifications");
                        return;
                    }
                };
                runtime.block_on(async move {
                    match session().await {
                        Ok(connection) => {
                            if let Err(error) = run(connection, receive, actions).await {
                                tracing::warn!(%error, "desktop notifications ended");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "could not connect desktop notifications")
                        }
                    }
                });
            });
        if let Err(error) = result {
            tracing::warn!(%error, "could not start desktop notifications");
        }
        Self { commands }
    }
}

impl SystemNotifier for DesktopNotifier {
    fn show(&self, alert: &Alert, _: &mut App) {
        let _ = self.commands.send(Command::Show(alert.clone()));
    }

    fn dismiss(&self, tag: &str, _: &mut App) {
        let _ = self.commands.send(Command::Dismiss(tag.to_owned()));
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn session() -> zbus::Result<Connection> {
    zbus::connection::Builder::session()?
        .method_timeout(WAIT)
        .build()
        .await
}

async fn run(
    connection: Connection,
    mut commands: mpsc::UnboundedReceiver<Command>,
    actions: futures_channel::mpsc::UnboundedSender<String>,
) -> zbus::Result<()> {
    let proxy = Proxy::new(&connection, SERVICE, OBJECT, SERVICE).await?;
    let mut signals = proxy.receive_all_signals().await?;
    let mut owners = proxy.receive_owner_changed().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let mut owner: Option<String> = None;
    let mut shown = HashMap::<String, u32>::new();
    loop {
        tokio::select! {
            biased;
            changed = owners.next() => {
                let Some(changed) = changed else { break };
                let changed = changed.map(|name| name.to_string());
                if owner != changed {
                    shown.clear();
                    owner = changed;
                }
            }
            signal = signals.next() => {
                let Some(signal) = signal else { break };
                let header = signal.header();
                if header.sender().map(|name| name.as_str()) != owner.as_deref() { continue; }
                match header.member().map(|member| member.as_str()) {
                    Some("ActionInvoked") => {
                        if let Ok((id, action)) = signal.body().deserialize::<(u32, String)>()
                            && action == "default"
                            && let Some((tag, _)) = shown.iter().find(|(_, held)| **held == id) {
                            let _ = actions.unbounded_send(tag.clone());
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((id, _reason)) = signal.body().deserialize::<(u32, u32)>() {
                            shown.retain(|_, held| *held != id);
                        }
                    }
                    _ => {}
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                // IDs belong to one service instance. Address that unique owner even if
                // the well-known name changes while a method is in flight.
                let mut current = bus.get_name_owner(SERVICE.try_into()?).await.ok().map(|name| name.to_string());
                if current.is_none() && matches!(&command, Command::Show(_)) {
                    if let Err(error) = bus.start_service_by_name(SERVICE.try_into()?, 0).await {
                        tracing::warn!(%error, "desktop notification service is unavailable");
                    }
                    current = bus.get_name_owner(SERVICE.try_into()?).await.ok().map(|name| name.to_string());
                }
                if current != owner {
                    shown.clear();
                    owner = current;
                }
                let Some(owner) = owner.as_ref() else { continue };
                let destination = Proxy::new(&connection, owner.as_str(), OBJECT, SERVICE).await?;
                match command {
                    Command::Show(alert) => {
                        let id = shown.get(&alert.tag).copied().unwrap_or(0);
                        let hints = HashMap::from([
                            ("desktop-entry", Value::from(crate::app::APP_ID)),
                            ("suppress-sound", Value::from(true)),
                        ]);
                        let body = ("Junto", id, "", alert.title.as_str(), escape(&alert.body),
                            vec!["default", "Open"], hints, -1_i32);
                        match tokio::time::timeout(WAIT, destination.call::<_, _, u32>("Notify", &body)).await {
                            Ok(Ok(id)) if id != 0 => { shown.insert(alert.tag, id); }
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => tracing::warn!(%error, "could not show desktop notification"),
                            Err(_) => tracing::warn!("desktop notification service did not answer"),
                        }
                    }
                    Command::Dismiss(tag) => {
                        if let Some(id) = shown.remove(&tag) {
                            let _ = tokio::time::timeout(WAIT, destination.call::<_, _, ()>("CloseNotification", &id)).await;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
