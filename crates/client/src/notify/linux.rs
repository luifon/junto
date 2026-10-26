//! The desktop notification service (ADR 0752).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant};

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
/// The pause before the worker connects to the session bus again.
const RETRY: Duration = Duration::from_secs(5);
/// An alert that waited longer than this for the service is not shown any more.
const STALE: Duration = Duration::from_secs(30);

/// A press on a notification: its tag, and the activation token that the service gave for
/// the press (version 1.2 of the specification). A Wayland compositor gives the focus to a
/// window only with such a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Press {
    pub(super) tag: String,
    pub(super) token: Option<String>,
}

pub(super) struct DesktopNotifier {
    commands: mpsc::UnboundedSender<Command>,
}

enum Command {
    /// The alert and the time at which the application asked for it.
    Show(Alert, Instant),
    Dismiss(String),
}

/// The commands of the application, and those that wait for a connection.
struct Queue {
    commands: mpsc::UnboundedReceiver<Command>,
    waiting: VecDeque<Command>,
}

impl Queue {
    async fn next(&mut self) -> Option<Command> {
        match self.waiting.pop_front() {
            Some(command) => Some(command),
            None => self.commands.recv().await,
        }
    }

    /// Puts back the command that was being served when the connection was lost: it is
    /// served first on the next connection, before the commands that came after it. A
    /// dismissal is not put back: without a connection it has nothing to take away.
    fn put_back(&mut self, command: Command) {
        if matches!(command, Command::Show(..)) {
            self.waiting.push_front(command);
        }
    }

    /// Keeps a command that arrives while there is no connection. A dismissal takes back
    /// the alert of its tag that still waits.
    fn keep(&mut self, command: Command) {
        match command {
            Command::Show(..) => self.waiting.push_back(command),
            Command::Dismiss(tag) => self
                .waiting
                .retain(|held| !matches!(held, Command::Show(alert, _) if alert.tag == tag)),
        }
        self.waiting
            .retain(|held| !matches!(held, Command::Show(_, asked) if asked.elapsed() > STALE));
    }
}

impl DesktopNotifier {
    pub(super) fn new(
        cx: &mut App,
        pressed: impl Fn(&str, Option<&str>, &mut App) + 'static,
    ) -> Self {
        let (commands, receive) = mpsc::unbounded_channel();
        let (actions, mut take_actions) = futures_channel::mpsc::unbounded::<Press>();
        cx.spawn(async move |cx| {
            while let Some(press) = take_actions.next().await {
                cx.update(|cx| pressed(&press.tag, press.token.as_deref(), cx));
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
                runtime.block_on(worker(session, receive, actions));
            });
        if let Err(error) = result {
            tracing::warn!(%error, "could not start desktop notifications");
        }
        Self { commands }
    }
}

impl SystemNotifier for DesktopNotifier {
    fn show(&self, alert: &Alert, _: &mut App) {
        let _ = self
            .commands
            .send(Command::Show(alert.clone(), Instant::now()));
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

/// Whether the service interprets markup in a body. A service that does not answer is
/// treated as one that does: text of another person must not reach a service that
/// interprets it without the escape.
async fn body_markup(service: &Proxy<'_>) -> bool {
    let call = service.call::<_, _, Vec<String>>("GetCapabilities", &());
    match tokio::time::timeout(WAIT, call).await {
        Ok(Ok(capabilities)) => capabilities.iter().any(|name| name == "body-markup"),
        _ => true,
    }
}

/// Serves the commands for as long as the session bus is reachable.
async fn worker<F>(
    connect: impl Fn() -> F,
    commands: mpsc::UnboundedReceiver<Command>,
    actions: futures_channel::mpsc::UnboundedSender<Press>,
) where
    F: Future<Output = zbus::Result<Connection>>,
{
    let mut queue = Queue {
        commands,
        waiting: VecDeque::new(),
    };
    let mut known = Known::default();
    let mut reported = false;
    loop {
        let failure = match connect().await {
            Ok(connection) => {
                reported = false;
                match serve(connection, &mut queue, &mut known, &actions).await {
                    Ok(()) => return,
                    Err(failure) => failure,
                }
            }
            Err(failure) => failure,
        };
        if !reported {
            tracing::warn!(%failure, "desktop notifications are interrupted; connecting again");
            reported = true;
        }
        let pause = tokio::time::sleep(RETRY);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                _ = &mut pause => break,
                command = queue.commands.recv() => match command {
                    Some(command) => queue.keep(command),
                    None => return,
                },
            }
        }
    }
}

fn lost() -> zbus::Error {
    zbus::Error::Failure("the session bus closed the connection".into())
}

/// What the worker knows of the notification service. It is kept over a lost connection:
/// the identifiers of a service stay valid while the same service runs on the same bus.
#[derive(Default)]
struct Known {
    /// The identifier of the session bus on which the rest was learned.
    bus: Option<String>,
    /// The unique name of the service.
    owner: Option<String>,
    /// Whether the service interprets markup; asked once for each owner.
    markup: Option<bool>,
    /// The identifier that the service gave for each tag.
    shown: HashMap<String, u32>,
}

impl Known {
    /// Identifiers and capabilities belong to one owner on one bus.
    fn owned_by(&mut self, owner: Option<String>) {
        if self.owner != owner {
            self.shown.clear();
            self.markup = None;
            self.owner = owner;
        }
    }
}

/// Serves one connection. `Ok`: the application ended. `Err`: the connection was lost.
async fn serve(
    connection: Connection,
    queue: &mut Queue,
    known: &mut Known,
    actions: &futures_channel::mpsc::UnboundedSender<Press>,
) -> zbus::Result<()> {
    let proxy = Proxy::new(&connection, SERVICE, OBJECT, SERVICE).await?;
    let mut signals = proxy.receive_all_signals().await?;
    let mut owners = proxy.receive_owner_changed().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let identifier = bus.get_id().await.map_err(zbus::Error::from)?.to_string();
    if known.bus.as_deref() != Some(identifier.as_str()) {
        *known = Known {
            bus: Some(identifier),
            ..Known::default()
        };
    }
    known.owned_by(name_owner(&bus).await?);
    // The service sends the token of a press before the action of the press.
    let mut tokens: HashMap<u32, String> = HashMap::new();
    loop {
        tokio::select! {
            biased;
            changed = owners.next() => {
                changed.ok_or_else(lost)?;
                // The event can be older than what a command already learned: the bus is
                // asked who owns the name now.
                known.owned_by(name_owner(&bus).await?);
            }
            signal = signals.next() => {
                let signal = signal.ok_or_else(lost)?;
                let header = signal.header();
                if header.sender().map(|name| name.as_str()) != known.owner.as_deref() { continue; }
                match header.member().map(|member| member.as_str()) {
                    Some("ActivationToken") => {
                        if let Ok((id, token)) = signal.body().deserialize::<(u32, String)>()
                            && known.shown.values().any(|held| *held == id) {
                            tokens.insert(id, token);
                        }
                    }
                    Some("ActionInvoked") => {
                        if let Ok((id, action)) = signal.body().deserialize::<(u32, String)>() {
                            let token = tokens.remove(&id);
                            if action == "default"
                                && let Some((tag, _)) = known.shown.iter().find(|(_, held)| **held == id) {
                                let _ = actions.unbounded_send(Press { tag: tag.clone(), token });
                            }
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((id, _reason)) = signal.body().deserialize::<(u32, u32)>() {
                            known.shown.retain(|_, held| *held != id);
                            tokens.remove(&id);
                        }
                    }
                    _ => {}
                }
            }
            command = queue.next() => {
                let Some(command) = command else { return Ok(()) };
                if matches!(&command, Command::Show(_, asked) if asked.elapsed() > STALE) {
                    tracing::debug!("an alert waited too long for the desktop notification service");
                    continue;
                }
                // IDs belong to one service instance. Address that unique owner even if
                // the well-known name changes while a method is in flight.
                let mut current = match name_owner(&bus).await {
                    Ok(current) => current,
                    Err(failure) => {
                        queue.put_back(command);
                        return Err(failure);
                    }
                };
                if current.is_none() && matches!(&command, Command::Show(..)) {
                    if let Err(error) = bus.start_service_by_name(SERVICE.try_into()?, 0).await {
                        tracing::warn!(%error, "desktop notification service is unavailable");
                    }
                    current = name_owner(&bus).await.ok().flatten();
                }
                known.owned_by(current);
                let Some(owner) = known.owner.clone() else { continue };
                let destination = match Proxy::new(&connection, owner.as_str(), OBJECT, SERVICE).await {
                    Ok(destination) => destination,
                    Err(failure) => {
                        queue.put_back(command);
                        return Err(failure);
                    }
                };
                match command {
                    Command::Show(alert, _) => {
                        let markup = match known.markup {
                            Some(markup) => markup,
                            None => *known.markup.insert(body_markup(&destination).await),
                        };
                        let text = if markup { escape(&alert.body) } else { alert.body.clone() };
                        let id = known.shown.get(&alert.tag).copied().unwrap_or(0);
                        let hints = HashMap::from([
                            ("desktop-entry", Value::from(crate::app::APP_ID)),
                            ("suppress-sound", Value::from(true)),
                        ]);
                        let body = ("Junto", id, "", alert.title.as_str(), text,
                            vec!["default", "Open"], hints, -1_i32);
                        match tokio::time::timeout(WAIT, destination.call::<_, _, u32>("Notify", &body)).await {
                            Ok(Ok(id)) if id != 0 => { known.shown.insert(alert.tag, id); }
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => tracing::warn!(%error, "could not show desktop notification"),
                            Err(_) => tracing::warn!("desktop notification service did not answer"),
                        }
                    }
                    Command::Dismiss(tag) => {
                        if let Some(id) = known.shown.remove(&tag) {
                            let _ = tokio::time::timeout(WAIT, destination.call::<_, _, ()>("CloseNotification", &id)).await;
                        }
                    }
                }
            }
        }
    }
}

/// The unique name of the service. `Ok(None)`: the bus answered that no service has it.
async fn name_owner(bus: &zbus::fdo::DBusProxy<'_>) -> zbus::Result<Option<String>> {
    match bus.get_name_owner(SERVICE.try_into()?).await {
        Ok(name) => Ok(Some(name.to_string())),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => Ok(None),
        Err(failure) => Err(failure.into()),
    }
}

// The tests start and remove their own session bus, on no thread of the window (ADR 0730).
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
