//! The tray icon of a Linux desktop: a StatusNotifierItem with a dbusmenu menu on the session
//! bus, which a StatusNotifierWatcher of the desktop shows (ADR 0757). It is driven on its own
//! worker, as the desktop notifications are (ADR 0752).

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::sync::mpsc;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str, Structure};
use zbus::{Connection, Proxy};

use super::{Picture, SystemTray, TrayAction, TrayState, renderings};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_OBJECT: &str = "/StatusNotifierWatcher";
const ITEM_OBJECT: &str = "/StatusNotifierItem";
const MENU_OBJECT: &str = "/MenuBar";
const WAIT: Duration = Duration::from_secs(5);
/// The pause before the worker connects to the session bus again.
const RETRY: Duration = Duration::from_secs(5);

type Presses = futures_channel::mpsc::UnboundedSender<TrayAction>;

pub(super) struct StatusNotifierTray {
    states: mpsc::UnboundedSender<TrayState>,
}

impl StatusNotifierTray {
    pub(super) fn new(presses: Presses) -> Self {
        let (states, receive) = mpsc::unbounded_channel();
        let result = std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::warn!(%error, "could not start the tray icon");
                        return;
                    }
                };
                runtime.block_on(worker(Builder::session, receive, presses));
            });
        if let Err(error) = result {
            tracing::warn!(%error, "could not start the tray icon");
        }
        Self { states }
    }
}

impl SystemTray for StatusNotifierTray {
    fn set(&self, state: &TrayState) {
        let _ = self.states.send(state.clone());
    }
}

/// Serves the tray for as long as the application runs, and connects again after a lost
/// connection.
async fn worker(
    connect: impl Fn() -> zbus::Result<Builder<'static>>,
    mut states: mpsc::UnboundedReceiver<TrayState>,
    presses: Presses,
) {
    let mut state = TrayState::default();
    let mut reported = false;
    loop {
        match serve(connect(), &mut states, &mut state, &presses).await {
            Ok(()) => return,
            Err(failure) => {
                if !reported {
                    tracing::warn!(%failure, "the tray icon is interrupted; connecting again");
                    reported = true;
                }
            }
        }
        let pause = tokio::time::sleep(RETRY);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                _ = &mut pause => break,
                next = states.recv() => match next {
                    Some(next) => state = next,
                    None => return,
                },
            }
        }
    }
}

fn lost() -> zbus::Error {
    zbus::Error::Failure("the session bus closed the connection".into())
}

/// Serves one connection. `Ok`: the application ended. `Err`: the connection was lost.
async fn serve(
    connect: zbus::Result<Builder<'static>>,
    states: &mut mpsc::UnboundedReceiver<TrayState>,
    state: &mut TrayState,
    presses: &Presses,
) -> zbus::Result<()> {
    let item = Item {
        state: state.clone(),
        presses: presses.clone(),
    };
    let menu = Menu {
        state: state.clone(),
        revision: 1,
        presses: presses.clone(),
    };
    let connection = connect?
        .method_timeout(WAIT)
        .serve_at(ITEM_OBJECT, item)?
        .serve_at(MENU_OBJECT, menu)?
        .build()
        .await?;
    // The name that the specification asks an item to own: the process and a number.
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    connection.request_name(name.as_str()).await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let mut owners = bus
        .receive_name_owner_changed_with_args(&[(0, WATCHER)])
        .await?;
    register(&connection, &bus, &name).await?;
    loop {
        tokio::select! {
            changed = owners.next() => {
                let changed = changed.ok_or_else(lost)?;
                // A watcher that starts after the application, or starts again, does not know
                // the item: the item registers with each new owner of the name.
                if changed.args().is_ok_and(|args| args.new_owner().is_some()) {
                    register(&connection, &bus, &name).await?;
                }
            }
            next = states.recv() => {
                let Some(next) = next else { return Ok(()) };
                if next != *state {
                    *state = next;
                    show(&connection, state).await?;
                }
            }
        }
    }
}

/// Registers the item with the watcher of the desktop. Without a watcher the desktop shows no
/// tray icon, which is stated once for each connection, and nothing fails.
async fn register(
    connection: &Connection,
    bus: &zbus::fdo::DBusProxy<'_>,
    name: &str,
) -> zbus::Result<()> {
    let watched = bus
        .name_has_owner(WATCHER.try_into()?)
        .await
        .map_err(zbus::Error::from)?;
    if !watched {
        tracing::info!(
            "no StatusNotifierWatcher on the session bus: the desktop shows no tray icon"
        );
        return Ok(());
    }
    let watcher = Proxy::new(connection, WATCHER, WATCHER_OBJECT, WATCHER).await?;
    match tokio::time::timeout(
        WAIT,
        watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &name),
    )
    .await
    {
        Ok(Ok(())) => tracing::debug!("the tray icon is registered"),
        Ok(Err(error)) => tracing::warn!(%error, "the watcher refused the tray icon"),
        Err(_) => tracing::warn!("the watcher of the tray icons did not answer"),
    }
    Ok(())
}

/// Gives the item and the menu the new state and tells the desktop.
async fn show(connection: &Connection, state: &TrayState) -> zbus::Result<()> {
    let server = connection.object_server();
    let item = server.interface::<_, Item>(ITEM_OBJECT).await?;
    item.get_mut().await.state = state.clone();
    Item::new_icon(item.signal_emitter()).await?;
    Item::new_tool_tip(item.signal_emitter()).await?;
    let menu = server.interface::<_, Menu>(MENU_OBJECT).await?;
    let revision = {
        let mut menu = menu.get_mut().await;
        menu.state = state.clone();
        menu.revision += 1;
        menu.revision
    };
    Menu::layout_updated(menu.signal_emitter(), revision, 0).await?;
    Ok(())
}

/// The pictures of the specification: width, height and ARGB32 pixels in network byte order.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

fn pixmaps(picture: Picture) -> Pixmaps {
    renderings(picture)
        .map(|(side, rgba)| {
            let argb = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[red, green, blue, alpha]| [alpha, red, green, blue])
                .collect();
            (side as i32, side as i32, argb)
        })
        .collect()
}

struct Item {
    state: TrayState,
    presses: Presses,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "Communications"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        crate::app::APP_ID
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        crate::text::APP_NAME
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        pixmaps(self.state.picture())
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (
            String::new(),
            Vec::new(),
            crate::text::APP_NAME.to_owned(),
            self.state.tooltip().to_owned(),
        )
    }

    /// The item has a menu of its own, and a primary press opens the application.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::from(zbus::zvariant::ObjectPath::from_static_str_unchecked(
            MENU_OBJECT,
        ))
    }

    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.presses.unbounded_send(TrayAction::Open);
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: String) {}

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// The properties of one entry of a dbusmenu menu.
type Properties = HashMap<String, OwnedValue>;
/// An entry with its children, as `GetLayout` gives it.
type Layout = (i32, Properties, Vec<OwnedValue>);

/// The identifier of the menu itself.
const ROOT: i32 = 0;

/// One entry of the menu as the desktop shows it. The identifier of an entry is the code of
/// its action (`TrayAction::code`); a separator has an identifier above the codes.
struct Shown {
    id: i32,
    properties: Properties,
}

/// The identifier of the separator before the entry with this identifier.
const SEPARATOR: i32 = 100;

fn text(value: &'static str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

fn shown(state: &TrayState) -> Vec<Shown> {
    let mut entries = Vec::new();
    for entry in state.menu() {
        let id = entry.action.code() as i32;
        if entry.separated {
            entries.push(Shown {
                id: SEPARATOR + id,
                properties: HashMap::from([("type".to_owned(), text("separator"))]),
            });
        }
        entries.push(Shown {
            id,
            properties: HashMap::from([
                ("label".to_owned(), text(entry.label)),
                ("enabled".to_owned(), OwnedValue::from(entry.enabled)),
            ]),
        });
    }
    entries
}

fn failed(error: zbus::zvariant::Error) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(error.to_string())
}

struct Menu {
    state: TrayState,
    revision: u32,
    presses: Presses,
}

impl Menu {
    fn properties(&self, id: i32) -> Option<Properties> {
        if id == ROOT {
            return Some(HashMap::from([(
                "children-display".to_owned(),
                text("submenu"),
            )]));
        }
        shown(&self.state)
            .into_iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.properties)
    }

    /// Takes an event of the entry `id`, and returns whether the menu has the entry.
    fn press(&self, id: i32, event: &str) -> bool {
        if !shown(&self.state).iter().any(|entry| entry.id == id) {
            return false;
        }
        if event == "clicked"
            && let Some(action) = u32::try_from(id)
                .ok()
                .and_then(|code| self.state.action(code))
        {
            let _ = self.presses.unbounded_send(action);
        }
        true
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let Some(properties) = self.properties(parent_id) else {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "no entry {parent_id}"
            )));
        };
        // The menu has one level: only the menu itself has children.
        let mut children = Vec::new();
        if parent_id == ROOT && recursion_depth != 0 {
            for entry in shown(&self.state) {
                let child: Layout = (entry.id, entry.properties, Vec::new());
                children.push(OwnedValue::try_from(Structure::from(child)).map_err(failed)?);
            }
        }
        Ok((self.revision, (parent_id, properties, children)))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, Properties)> {
        ids.into_iter()
            .filter_map(|id| self.properties(id).map(|properties| (id, properties)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        self.properties(id)
            .and_then(|mut properties| properties.remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name} of {id}")))
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        self.press(id, &event_id);
    }

    /// Returns the identifiers that name no entry.
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        events
            .into_iter()
            .filter(|(id, event, _, _)| !self.press(*id, event))
            .map(|(id, _, _, _)| id)
            .collect()
    }

    /// The menu does not change when it is shown.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

// The tests start and remove their own session bus, on no thread of the window (ADR 0730).
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
