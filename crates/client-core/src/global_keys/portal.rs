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

/// The name of the shortcut of push to talk.
pub const TALK: &str = "push-to-talk";
/// The name of the shortcut that mutes and unmutes.
pub const MUTE: &str = "toggle-mute";
/// How long Junto waits for the answer of the desktop, which can show a dialog.
const ANSWER: Duration = Duration::from_secs(120);

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
    let mut shortcuts: Vec<(&str, HashMap<&str, Value<'_>>)> = Vec::new();
    let described = |description: &'static str, combo: &KeyCombo| {
        HashMap::from([
            ("description", Value::from(description)),
            ("preferred_trigger", Value::from(trigger(combo))),
        ])
    };
    if let Some(talk) = &keys.talk {
        shortcuts.push((TALK, described("Push to talk in a call of Junto", talk)));
    }
    if let Some(mute) = &keys.mute {
        shortcuts.push((
            MUTE,
            described("Mute or unmute the microphone in a call of Junto", mute),
        ));
    }
    let bound_token = format!("{token}_bind");
    ask(
        connection,
        &bound_token,
        portal.bind_shortcuts(
            &session,
            shortcuts,
            "",
            HashMap::from([("handle_token", Value::from(bound_token.as_str()))]),
        ),
    )
    .await?;
    Ok(session)
}

async fn close(connection: &Connection, session: OwnedObjectPath) {
    let proxy = SessionProxy::builder(connection)
        .path(session)
        .map(|builder| builder.build());
    if let Ok(building) = proxy
        && let Ok(proxy) = building.await
    {
        let _ = proxy.close().await;
    }
}

/// Gives the events of the shortcuts while a call needs them, until `wanted` ends. The keys
/// are bound when a call first needs them and again when they change. A press outside a call
/// gives nothing; a hold of push to talk ends with the call.
pub async fn run(
    connection: Connection,
    mut wanted: watch::Receiver<Option<CallKeys>>,
    events: impl Fn(KeyEvent),
) -> Result<(), PortalError> {
    let portal = GlobalShortcutsProxy::new(&connection)
        .await
        .map_err(failed)?;
    let mut activated = portal.receive_activated().await.map_err(failed)?;
    let mut deactivated = portal.receive_deactivated().await.map_err(failed)?;
    let mut bound: Option<(CallKeys, OwnedObjectPath)> = None;
    let mut number = 0;
    let mut talking = false;
    loop {
        let keys = wanted.borrow_and_update().clone();
        if let Some(keys) = &keys
            && bound.as_ref().map(|(held, _)| held) != Some(keys)
        {
            if let Some((_, session)) = bound.take() {
                close(&connection, session).await;
            }
            number += 1;
            let session = bind(&connection, &portal, keys, number).await?;
            bound = Some((keys.clone(), session));
        }
        let ours = |session: &ObjectPath<'_>| {
            bound
                .as_ref()
                .is_some_and(|(_, held)| held.as_str() == session.as_str())
        };
        tokio::select! {
            changed = wanted.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                if wanted.borrow().is_none() && std::mem::take(&mut talking) {
                    events(KeyEvent::Talk(false));
                }
            }
            signal = activated.next() => {
                let signal = signal.ok_or_else(|| PortalError::Failed("the bus ended".to_owned()))?;
                let args = signal.args().map_err(failed)?;
                if keys.is_some() && ours(&args.session_handle) {
                    match args.shortcut_id {
                        TALK if !talking => {
                            talking = true;
                            events(KeyEvent::Talk(true));
                        }
                        MUTE => events(KeyEvent::ToggleMute),
                        _ => {}
                    }
                }
            }
            signal = deactivated.next() => {
                let signal = signal.ok_or_else(|| PortalError::Failed("the bus ended".to_owned()))?;
                let args = signal.args().map_err(failed)?;
                if ours(&args.session_handle) && args.shortcut_id == TALK && talking {
                    talking = false;
                    events(KeyEvent::Talk(false));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
