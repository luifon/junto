//! The global shortcuts portal of a Wayland desktop (ADR 0776). A Wayland session gives no
//! program the keys of another one; the desktop gives a program the presses of the shortcuts
//! that the program binds through the portal, after the person confirms them in a dialog of
//! the desktop, which can also change their keys. Junto binds the key of push to talk and the
//! key that mutes, with its own keys as the suggestion, when a call first needs them, and
//! binds them again when they change.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::sync::watch;
use zbus::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::{CallKeys, Key, KeyCombo, KeyEvent};
use crate::keybinds::KeyAction;
/// How long Junto waits for the answer of the desktop, which can show a dialog.
const ANSWER: Duration = Duration::from_secs(120);

#[derive(Clone, Default)]
pub struct Requested {
    pub keys: Option<CallKeys>,
    pub boundary: Option<u64>,
}

impl From<Option<CallKeys>> for Requested {
    fn from(keys: Option<CallKeys>) -> Self {
        Self {
            keys,
            boundary: None,
        }
    }
}

/// True in a Wayland session. Whether the desktop has the portal shows when a call binds the
/// keys.
pub fn available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
}

#[zbus::proxy(
    interface = "org.freedesktop.portal.GlobalShortcuts",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait GlobalShortcuts {
    fn create_session(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;

    fn bind_shortcuts(
        &self,
        session_handle: &ObjectPath<'_>,
        shortcuts: Vec<(&str, HashMap<&str, Value<'_>>)>,
        parent_window: &str,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;

    #[zbus(signal)]
    fn activated(
        &self,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    fn deactivated(
        &self,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.portal.Request",
    default_service = "org.freedesktop.portal.Desktop"
)]
trait Request {
    #[zbus(signal)]
    fn response(&self, response: u32, results: HashMap<String, OwnedValue>) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.portal.Session",
    default_service = "org.freedesktop.portal.Desktop"
)]
trait Session {
    fn close(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn closed(&self, details: HashMap<String, OwnedValue>) -> zbus::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortalError {
    #[error("the desktop has no global shortcuts portal: {0}")]
    Missing(String),
    #[error("the person did not confirm the shortcuts")]
    Refused,
    #[error("the desktop did not answer within {} seconds", ANSWER.as_secs())]
    NoAnswer,
    #[error("the global shortcuts portal failed: {0}")]
    Failed(String),
}

fn failed(error: zbus::Error) -> PortalError {
    match &error {
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod"
                || name.as_str() == "org.freedesktop.DBus.Error.UnknownInterface"
                || name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            PortalError::Missing(error.to_string())
        }
        _ => PortalError::Failed(error.to_string()),
    }
}

/// The suggestion of a key of a call in the form of the shortcuts of the desktop: the
/// modifiers in capitals, then the name of the key symbol.
pub fn trigger(combo: &KeyCombo) -> String {
    combo
        .keys()
        .iter()
        .map(|key| match key {
            Key::Control => "CTRL".to_owned(),
            Key::Alt => "ALT".to_owned(),
            Key::Shift => "SHIFT".to_owned(),
            Key::Meta => "LOGO".to_owned(),
            Key::Letter(letter) => letter.to_ascii_lowercase().to_string(),
            Key::Digit(digit) => digit.to_string(),
            Key::Function(number) => format!("F{number}"),
            Key::Up => "Up".to_owned(),
            Key::Down => "Down".to_owned(),
            Key::Left => "Left".to_owned(),
            Key::Right => "Right".to_owned(),
            Key::Enter => "Return".to_owned(),
            Key::Escape => "Escape".to_owned(),
            Key::Backspace => "BackSpace".to_owned(),
            Key::PageUp => "Page_Up".to_owned(),
            Key::PageDown => "Page_Down".to_owned(),
            Key::Comma => "comma".to_owned(),
            Key::Slash => "slash".to_owned(),
            Key::Equal => "equal".to_owned(),
            Key::Minus => "minus".to_owned(),
            Key::BracketLeft => "bracketleft".to_owned(),
            Key::BracketRight => "bracketright".to_owned(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// The path of the request object that the portal makes for a call with `token`.
fn request_path(connection: &Connection, token: &str) -> Result<OwnedObjectPath, PortalError> {
    let sender = connection
        .unique_name()
        .ok_or_else(|| PortalError::Failed("no unique name on the bus".to_owned()))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    OwnedObjectPath::try_from(format!(
        "/org/freedesktop/portal/desktop/request/{sender}/{token}"
    ))
    .map_err(|error| PortalError::Failed(error.to_string()))
}

/// Makes a call that the portal answers with a request, and waits for the answer.
async fn ask(
    connection: &Connection,
    token: &str,
    call: impl Future<Output = zbus::Result<OwnedObjectPath>>,
) -> Result<HashMap<String, OwnedValue>, PortalError> {
    let path = request_path(connection, token)?;
    let request = RequestProxy::builder(connection)
        .path(path)
        .map_err(failed)?
        .build()
        .await
        .map_err(failed)?;
    // The answer can come before the call returns, so its wait starts first.
    let mut answers = request.receive_response().await.map_err(failed)?;
    call.await.map_err(failed)?;
    let answer = tokio::time::timeout(ANSWER, answers.next())
        .await
        .map_err(|_| PortalError::NoAnswer)?
        .ok_or(PortalError::NoAnswer)?;
    let args = answer.args().map_err(failed)?;
    if args.response != 0 {
        return Err(PortalError::Refused);
    }
    Ok(args.results)
}

/// Makes a session and binds the shortcuts of `keys` in it. Returns the session.
async fn bind(
    connection: &Connection,
    portal: &GlobalShortcutsProxy<'_>,
    keys: &CallKeys,
    number: u32,
) -> Result<OwnedObjectPath, PortalError> {
    let token = format!("junto_keys_{number}");
    let created = ask(
        connection,
        &token,
        portal.create_session(HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("session_handle_token", Value::from(token.as_str())),
        ])),
    )
    .await?;
    let session = created
        .get("session_handle")
        .and_then(|value| {
            OwnedObjectPath::try_from(value.clone()).ok().or_else(|| {
                String::try_from(value.clone())
                    .ok()
                    .and_then(|text| OwnedObjectPath::try_from(text).ok())
            })
        })
        .ok_or_else(|| PortalError::Failed("the session has no handle".to_owned()))?;
    // Each shortcut is named by its action, as the settings file names it.
    let names: Vec<String> = keys
        .bindings
        .iter()
        .map(|(action, _)| action.name())
        .collect();
    let shortcuts: Vec<(&str, HashMap<&str, Value<'_>>)> = keys
        .bindings
        .iter()
        .zip(&names)
        .map(|((action, combo), name)| {
            (
                name.as_str(),
                HashMap::from([
                    ("description", Value::from(action.description())),
                    ("preferred_trigger", Value::from(trigger(combo))),
                ]),
            )
        })
        .collect();
    let bound_token = format!("{token}_bind");
    let result = ask(
        connection,
        &bound_token,
        portal.bind_shortcuts(
            &session,
            shortcuts,
            "",
            HashMap::from([("handle_token", Value::from(bound_token.as_str()))]),
        ),
    )
    .await;
    if let Err(error) = result {
        close(connection, session).await;
        return Err(error);
    }
    Ok(session)
}

async fn close(connection: &Connection, session: OwnedObjectPath) {
    let proxy = SessionProxy::builder(connection)
        .path(session)
        .map(|builder| builder.build());
    if let Ok(building) = proxy
        && let Ok(proxy) = building.await
    {
        let _ = tokio::time::timeout(ANSWER, proxy.close()).await;
    }
}

struct Held<'a, F: Fn(KeyEvent)> {
    actions: Vec<KeyAction>,
    events: &'a F,
}

impl<F: Fn(KeyEvent)> Held<'_, F> {
    fn release(&mut self) {
        for action in std::mem::take(&mut self.actions) {
            (self.events)(KeyEvent::Held(action, false));
        }
    }
}

impl<F: Fn(KeyEvent)> Drop for Held<'_, F> {
    fn drop(&mut self) {
        self.release();
    }
}

/// Gives the events of the shortcuts while a call needs them, until `wanted` ends. The keys
/// are bound when a call first needs them and again when they change. A press outside a call
/// gives nothing; a hold of push to talk or push to mute ends with the call.
pub async fn run<T: Clone + Into<Requested>>(
    connection: Connection,
    mut wanted: watch::Receiver<T>,
    events: impl Fn(KeyEvent),
) -> Result<(), PortalError> {
    let portal = GlobalShortcutsProxy::new(&connection)
        .await
        .map_err(failed)?;
    let mut activated = portal.receive_activated().await.map_err(failed)?;
    let mut deactivated = portal.receive_deactivated().await.map_err(failed)?;
    let mut bound: Option<(CallKeys, OwnedObjectPath)> = None;
    let mut number = 0;
    let mut holding = Held {
        actions: Vec::new(),
        events: &events,
    };
    let mut closed = None;
    let mut ready = None;
    loop {
        let request: Requested = wanted.borrow_and_update().clone().into();
        let keys = request.keys.map(CallKeys::resolved);
        if let Some(keys) = &keys
            && bound.as_ref().map(|(held, _)| held) != Some(keys)
        {
            holding.release();
            if let Some((_, session)) = bound.take() {
                close(&connection, session).await;
            }
            number += 1;
            let session = bind(&connection, &portal, keys, number).await?;
            let proxy = SessionProxy::builder(&connection)
                .path(session.clone())
                .map_err(failed)?
                .build()
                .await
                .map_err(failed)?;
            closed = Some(proxy.receive_closed().await.map_err(failed)?);
            bound = Some((keys.clone(), session));
        }
        if let Some(boundary) = request.boundary
            && ready != Some(boundary)
        {
            ready = Some(boundary);
            events(KeyEvent::Ready(boundary));
        }
        let ours = |session: &ObjectPath<'_>| {
            bound
                .as_ref()
                .is_some_and(|(_, held)| held.as_str() == session.as_str())
        };
        tokio::select! {
            biased;
            changed = wanted.changed() => {
                if changed.is_err() {
                    if let Some((_, session)) = bound.take() {
                        close(&connection, session).await;
                    }
                    return Ok(());
                }
                let next: Requested = wanted.borrow().clone().into();
                if next.keys.map(CallKeys::resolved) != keys {
                    holding.release();
                }
            }
            _ = async {
                match &mut closed {
                    Some(stream) => stream.next().await,
                    None => std::future::pending().await,
                }
            } => return Err(PortalError::Failed("the desktop closed the shortcuts session".to_owned())),
            signal = activated.next() => {
                let signal = signal.ok_or_else(|| PortalError::Failed("the bus ended".to_owned()))?;
                let args = signal.args().map_err(failed)?;
                let action = keys.as_ref().filter(|_| ours(&args.session_handle)).and_then(|keys| {
                    keys.bindings
                        .iter()
                        .map(|(action, _)| *action)
                        .find(|action| action.name() == args.shortcut_id)
                });
                match action {
                    Some(action) if action.holds() => {
                        if !holding.actions.contains(&action) {
                            holding.actions.push(action);
                            events(KeyEvent::Held(action, true));
                        }
                    }
                    Some(action) => events(KeyEvent::Pressed(action)),
                    None => {}
                }
            }
            signal = deactivated.next() => {
                let signal = signal.ok_or_else(|| PortalError::Failed("the bus ended".to_owned()))?;
                let args = signal.args().map_err(failed)?;
                if ours(&args.session_handle)
                    && let Some(at) = holding.actions
                        .iter()
                        .position(|action| action.name() == args.shortcut_id)
                {
                    events(KeyEvent::Held(holding.actions.remove(at), false));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
