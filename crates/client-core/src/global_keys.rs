//! The keys of a call in every application (ADR 0776, ADR 0777): the key bindings that the
//! person lets work while another application is in front, such as push to talk and the key
//! that mutes. A reader of the keyboard of the system tells which keys are held, at a short
//! interval while a call needs them; [`Edges`] turns those readings into events. Where no
//! reader exists, the window gives the same events while it is in front.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::keybinds::KeyAction;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
pub mod x11;

/// One key of the keyboard that a key binding can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Control,
    /// Alt, the Option key on macOS.
    Alt,
    Shift,
    /// The Command key on macOS, the Windows key, Super on Linux.
    Meta,
    /// A letter from A to Z, in upper case.
    Letter(char),
    /// A digit of the main block of the keyboard.
    Digit(u8),
    /// A function key from F1 to F24.
    Function(u8),
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    PageUp,
    PageDown,
    Comma,
    Slash,
    Equal,
    Minus,
    BracketLeft,
    BracketRight,
}

/// The keys that are neither modifiers nor letters, digits or function keys.
const NAMED: [(Key, &str); 15] = [
    (Key::Up, "up"),
    (Key::Down, "down"),
    (Key::Left, "left"),
    (Key::Right, "right"),
    (Key::Enter, "enter"),
    (Key::Escape, "escape"),
    (Key::Backspace, "backspace"),
    (Key::PageUp, "pageup"),
    (Key::PageDown, "pagedown"),
    (Key::Comma, "comma"),
    (Key::Slash, "slash"),
    (Key::Equal, "equal"),
    (Key::Minus, "minus"),
    (Key::BracketLeft, "bracketleft"),
    (Key::BracketRight, "bracketright"),
];

impl Key {
    pub fn is_modifier(self) -> bool {
        matches!(self, Key::Control | Key::Alt | Key::Shift | Key::Meta)
    }

    /// The keys that a reader reads: the modifiers, the letters, the digits, the function
    /// keys and the named keys.
    pub fn all() -> impl Iterator<Item = Key> {
        [Key::Control, Key::Alt, Key::Shift, Key::Meta]
            .into_iter()
            .chain(('A'..='Z').map(Key::Letter))
            .chain((0..=9).map(Key::Digit))
            .chain((1..=24).map(Key::Function))
            .chain(NAMED.iter().map(|(key, _)| *key))
    }

    fn name(self) -> String {
        match self {
            Key::Control => "control".to_owned(),
            Key::Alt => "alt".to_owned(),
            Key::Shift => "shift".to_owned(),
            Key::Meta => "meta".to_owned(),
            Key::Letter(letter) => letter.to_ascii_lowercase().to_string(),
            Key::Digit(digit) => digit.to_string(),
            Key::Function(number) => format!("f{number}"),
            named => NAMED
                .iter()
                .find(|(key, _)| *key == named)
                .map_or_else(String::new, |(_, name)| (*name).to_owned()),
        }
    }
}

impl FromStr for Key {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, KeyError> {
        let text = text.trim().to_ascii_lowercase();
        let key = match text.as_str() {
            "control" | "ctrl" => Key::Control,
            "alt" | "option" => Key::Alt,
            "shift" => Key::Shift,
            "meta" | "cmd" | "command" | "super" | "win" | "platform" => Key::Meta,
            // The names of gpui for the keys of a key press.
            "return" => Key::Enter,
            "esc" => Key::Escape,
            "," => Key::Comma,
            "/" => Key::Slash,
            "=" => Key::Equal,
            "-" => Key::Minus,
            "[" => Key::BracketLeft,
            "]" => Key::BracketRight,
            named if NAMED.iter().any(|(_, name)| *name == named) => NAMED
                .iter()
                .find(|(_, name)| *name == named)
                .map_or(Key::Escape, |(key, _)| *key),
            _ => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(letter @ 'a'..='z'), None) => Key::Letter(letter.to_ascii_uppercase()),
                    (Some(digit @ '0'..='9'), None) => {
                        Key::Digit(digit.to_digit(10).map_or(0, |digit| digit as u8))
                    }
                    (Some('f'), Some(_)) => {
                        let number: u8 = text[1..].parse().map_err(|_| KeyError(text.clone()))?;
                        if !(1..=24).contains(&number) {
                            return Err(KeyError(text));
                        }
                        Key::Function(number)
                    }
                    _ => return Err(KeyError(text)),
                }
            }
        };
        Ok(key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a key of a call: {0}")]
pub struct KeyError(String);

/// A key of a call: the modifiers and at most one other key, held together. The key of push
/// to talk is one modifier; the key that mutes needs at least one modifier and one other
/// key, so that it does not fire while a person types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    /// The keys in the order of [`Key`]: the modifiers first.
    keys: Vec<Key>,
}

impl KeyCombo {
    /// The combination of these keys. Empty, with two keys that are not modifiers, or with a
    /// key twice, it is no combination.
    pub fn new(keys: impl IntoIterator<Item = Key>) -> Result<Self, KeyError> {
        let mut keys: Vec<Key> = keys.into_iter().collect();
        keys.sort();
        let text = || {
            keys.iter()
                .map(|key| key.name())
                .collect::<Vec<_>>()
                .join("+")
        };
        let others = keys.iter().filter(|key| !key.is_modifier()).count();
        let mut unique = keys.clone();
        unique.dedup();
        if keys.is_empty() || others > 1 || unique.len() != keys.len() {
            return Err(KeyError(text()));
        }
        Ok(Self { keys })
    }

    /// The combination of one key.
    pub fn single(key: Key) -> Self {
        Self { keys: vec![key] }
    }

    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// True when the key fires only while the person types it on purpose: one modifier or
    /// more with one other key.
    pub fn is_shortcut(&self) -> bool {
        self.keys.iter().any(|key| key.is_modifier())
            && self.keys.iter().any(|key| !key.is_modifier())
    }

    /// True when every key of the combination is held.
    pub fn held_in(&self, held: &[Key]) -> bool {
        self.keys.iter().all(|key| held.contains(key))
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<String> = self.keys.iter().map(|key| key.name()).collect();
        formatter.write_str(&names.join("+"))
    }
}

impl FromStr for KeyCombo {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, KeyError> {
        let keys = text
            .split('+')
            .map(Key::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(keys)
    }
}

impl Serialize for KeyCombo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyCombo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The bindings that work while another application is in front, while a call needs them
/// (ADR 0777): each action with its keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallKeys {
    pub bindings: Vec<(KeyAction, KeyCombo)>,
}

impl CallKeys {
    pub fn resolved(mut self) -> Self {
        let bindings = self.bindings.clone();
        self.bindings.retain(|(action, combo)| {
            KeyAction::ALL.iter().find(|candidate| {
                bindings
                    .iter()
                    .any(|(bound, keys)| bound == *candidate && keys == combo)
            }) == Some(action)
        });
        self
    }

    /// Every key that the readings need.
    pub fn keys(&self) -> Vec<Key> {
        let mut keys: Vec<Key> = self
            .bindings
            .iter()
            .flat_map(|(_, combo)| combo.keys().iter().copied())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

/// An event of the keys of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// A requested recording boundary has reached a keyboard with no held keys.
    Ready(u64),
    /// The keys of an action that lasts while they are held, push to talk or push to mute,
    /// are held now, or let go.
    Held(KeyAction, bool),
    /// The keys of an action that happens once were pressed.
    Pressed(KeyAction),
}

/// Turns readings of the held keys into events: the start and the end of a hold of the keys
/// of a held action, and each press of the keys of another action. A press counts when its
/// combination becomes held; holding it gives no second press. When the held keys hold two
/// combinations and one holds the other, such as Control+Shift+M and Shift+M, only the larger
/// one counts.
#[derive(Debug, Default)]
pub struct Edges {
    /// The actions whose combinations the last reading held.
    holding: Vec<KeyAction>,
}

impl Edges {
    pub fn read(&mut self, keys: &CallKeys, held: &[Key]) -> Vec<KeyEvent> {
        let keys = keys.clone().resolved();
        let holding: Vec<&(KeyAction, KeyCombo)> = keys
            .bindings
            .iter()
            .filter(|(_, combo)| combo.held_in(held))
            .collect();
        let larger = |combo: &KeyCombo| {
            holding.iter().any(|(_, other)| {
                other.keys().len() > combo.keys().len() && combo.held_in(other.keys())
            })
        };
        // The holds that end come before the presses and the holds that begin.
        let mut events: Vec<KeyEvent> = self
            .holding
            .iter()
            .filter(|action| action.holds() && !holding.iter().any(|(now, _)| now == *action))
            .map(|action| KeyEvent::Held(*action, false))
            .collect();
        for (action, combo) in &holding {
            if self.holding.contains(action) {
                continue;
            }
            if action.holds() {
                events.push(KeyEvent::Held(*action, true));
            } else if !larger(combo) {
                events.push(KeyEvent::Pressed(*action));
            }
        }
        self.holding = holding.iter().map(|(action, _)| *action).collect();
        events
    }

    /// The readings end: the keys of a held action that were held are let go.
    pub fn end(&mut self) -> Vec<KeyEvent> {
        std::mem::take(&mut self.holding)
            .into_iter()
            .filter(|action| action.holds())
            .map(|action| KeyEvent::Held(action, false))
            .collect()
    }
}

/// Reads which of some keys are held on the keyboard of the system, whichever application
/// is in front.
pub trait KeyReader: Send {
    fn held(&mut self, keys: &[Key]) -> Result<Vec<Key>, ReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The system does not let this program read the keyboard: on macOS the permission
    /// Input Monitoring.
    #[error("the system does not allow Junto to read the keyboard")]
    NotAllowed,
    #[error("the keyboard could not be read: {0}")]
    Failed(String),
}

/// How the keys of a call reach Junto while another application is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A reader of the system reads them: Windows and X11.
    Everywhere,
    /// macOS reads them with the permission Input Monitoring, which the person gives.
    WithPermission(Permission),
    /// A Wayland desktop with the global shortcuts portal: the desktop gives the presses.
    Portal,
    /// Only while a window of Junto is in front: a Wayland desktop without the portal.
    WindowOnly,
}

/// The state of a permission of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    /// Not given yet, or refused: the system asks once; afterwards the person changes it in
    /// the settings of the system.
    NotGranted,
}

/// How the keys of a call reach Junto on this system now.
pub fn reach() -> Reach {
    #[cfg(target_os = "macos")]
    {
        Reach::WithPermission(macos::input_monitoring())
    }
    #[cfg(windows)]
    {
        Reach::Everywhere
    }
    #[cfg(target_os = "linux")]
    {
        if x11::available() {
            Reach::Everywhere
        } else if portal::available() {
            Reach::Portal
        } else {
            Reach::WindowOnly
        }
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        Reach::WindowOnly
    }
}

/// The state of the permission Input Monitoring of macOS, which the reader of the keyboard
/// needs. `CGPreflightListenEventAccess`. Elsewhere there is no such permission.
pub fn input_monitoring() -> Permission {
    #[cfg(target_os = "macos")]
    {
        macos::input_monitoring()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Permission::Granted
    }
}

/// Asks macOS for the permission Input Monitoring: the system shows its question the first
/// time and lists Junto in its settings. `CGRequestListenEventAccess`. Returns the state
/// after the question. Elsewhere it does nothing.
pub fn request_input_monitoring() -> Permission {
    #[cfg(target_os = "macos")]
    {
        macos::request_input_monitoring()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Permission::Granted
    }
}

/// The address of the page of the settings of macOS where the person gives the permission
/// Input Monitoring.
pub const INPUT_MONITORING_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";

/// The reader of the keyboard of this system, when it has one.
pub fn system_reader() -> Option<Box<dyn KeyReader>> {
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(macos::Reader))
    }
    #[cfg(windows)]
    {
        Some(Box::new(windows::Reader))
    }
    #[cfg(target_os = "linux")]
    {
        x11::Reader::connect()
            .ok()
            .map(|reader| Box::new(reader) as Box<dyn KeyReader>)
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        None
    }
}

/// The interval of the readings while a call needs the keys: short enough that the first
/// syllable after the press of the key of push to talk is sent.
pub const INTERVAL: std::time::Duration = std::time::Duration::from_millis(15);

/// Reads the keys of a call at [`INTERVAL`] while a call needs them, on a thread of its own,
/// and gives each event to a callback on that thread. Without a call nothing is read. The
/// thread ends with the poller.
pub struct Poller {
    keys: std::sync::mpsc::Sender<(Option<CallKeys>, Option<u64>)>,
    failure: std::sync::Arc<std::sync::Mutex<Option<ReadError>>>,
}

impl Poller {
    pub fn start(
        mut reader: Box<dyn KeyReader>,
        events: impl Fn(KeyEvent) + Send + 'static,
    ) -> Self {
        use std::sync::mpsc::RecvTimeoutError;
        let (keys, taken) = std::sync::mpsc::channel::<(Option<CallKeys>, Option<u64>)>();
        let failure: std::sync::Arc<std::sync::Mutex<Option<ReadError>>> = Default::default();
        let told = failure.clone();
        let tell = move |error: Option<ReadError>| {
            *told
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
        };
        let started = std::thread::Builder::new()
            .name("junto-call-keys".to_owned())
            .spawn(move || {
                let mut wanted: Option<CallKeys> = None;
                let mut edges = Edges::default();
                let mut resume = None;
                loop {
                    // Without a call the thread only waits for the keys of the next call.
                    let wait = if wanted.is_some() || resume.is_some() {
                        INTERVAL
                    } else {
                        std::time::Duration::from_secs(1)
                    };
                    match taken.recv_timeout(wait) {
                        Ok((mut next, mut boundary)) => {
                            while let Ok((newer, newer_boundary)) = taken.try_recv() {
                                next = newer;
                                boundary = newer_boundary.or(boundary);
                            }
                            if boundary.is_some() {
                                resume = boundary;
                            }
                            if next.is_none() || boundary.is_some() {
                                edges.end().into_iter().for_each(&events);
                                tell(None);
                            }
                            wanted = next;
                            continue;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    if wanted.is_none() && resume.is_none() {
                        continue;
                    }
                    let read = if resume.is_some() {
                        Key::all().collect()
                    } else {
                        wanted.as_ref().map_or_else(Vec::new, CallKeys::keys)
                    };
                    match reader.held(&read) {
                        Ok(held) => {
                            tell(None);
                            if let Some(boundary) = resume {
                                if held.is_empty() {
                                    resume = None;
                                    events(KeyEvent::Ready(boundary));
                                }
                            } else if let Some(keys) = &wanted {
                                edges.read(keys, &held).into_iter().for_each(&events);
                            }
                        }
                        Err(error) => {
                            tell(Some(error));
                            edges.end().into_iter().for_each(&events);
                            if let Some(boundary) = resume.take() {
                                events(KeyEvent::Ready(boundary));
                            }
                        }
                    }
                }
            });
        if let Err(error) = started {
            *failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(ReadError::Failed(error.to_string()));
        }
        Self { keys, failure }
    }

    /// The keys that the call needs now; absent without a call, which stops the readings.
    pub fn set(&self, keys: Option<CallKeys>) {
        let _ = self.keys.send((keys.map(CallKeys::resolved), None));
    }

    /// Why the last reading failed, while it fails.
    pub fn failure(&self) -> Option<ReadError> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Where the keys of a call come from on this installation, and how that goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysState {
    /// The keyboard of the system is read while a call needs it.
    Reading,
    /// The last reading failed: on macOS without the permission Input Monitoring.
    Failed(ReadError),
    /// The desktop gives the presses of the shortcuts that Junto bound through its portal.
    Portal,
    /// The portal of the desktop ended, or is missing, with the reason.
    PortalFailed(String),
    /// Only a window of Junto that is in front gives the keys.
    WindowOnly,
}

enum Source {
    Poller(Poller),
    #[cfg(target_os = "linux")]
    Portal {
        wanted: tokio::sync::watch::Sender<portal::Requested>,
        failure: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    },
    None,
}

/// The keys of a call of this installation, also while another application is in front
/// (ADR 0776): read from the keyboard of the system, or given by the global shortcuts portal
/// of a Wayland desktop. Each event reaches the callback on a thread of the source.
pub struct GlobalKeys {
    source: Source,
}

impl GlobalKeys {
    pub fn pause_for_recording(&self) {
        #[cfg(target_os = "linux")]
        if let Source::Portal { wanted, .. } = &self.source {
            wanted.send_modify(|request| request.keys = None);
        }
    }

    pub fn resume_after_release(&self, keys: Option<CallKeys>, boundary: u64) -> bool {
        #[cfg(target_os = "linux")]
        if let Source::Portal { wanted, .. } = &self.source {
            wanted.send_replace(portal::Requested {
                keys,
                boundary: Some(boundary),
            });
            return true;
        }
        if let Source::Poller(poller) = &self.source {
            let _ = poller
                .keys
                .send((keys.map(CallKeys::resolved), Some(boundary)));
            true
        } else {
            self.set(keys);
            false
        }
    }

    /// The source of this system. The portal runs on `runtime`.
    pub fn of_system(
        runtime: &tokio::runtime::Handle,
        events: impl Fn(KeyEvent) + Send + Sync + 'static,
    ) -> Self {
        #[cfg(target_os = "linux")]
        if !x11::available() && portal::available() {
            let (wanted, watched) = tokio::sync::watch::channel(portal::Requested::default());
            let failure: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
            let told = failure.clone();
            runtime.spawn(async move {
                let ended = match zbus::Connection::session().await {
                    Ok(connection) => portal::run(connection, watched, events).await,
                    Err(error) => Err(portal::PortalError::Missing(error.to_string())),
                };
                if let Err(error) = ended {
                    tracing::warn!(%error, "the keys of a call reach Junto only in its windows");
                    *told
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(error.to_string());
                }
            });
            return Self {
                source: Source::Portal { wanted, failure },
            };
        }
        let _ = runtime;
        match system_reader() {
            Some(reader) => Self::with_reader(reader, events),
            None => Self::none(),
        }
    }

    /// Reads the keys with `reader`: a test gives [`HeldKeys`].
    pub fn with_reader(
        reader: Box<dyn KeyReader>,
        events: impl Fn(KeyEvent) + Send + 'static,
    ) -> Self {
        Self {
            source: Source::Poller(Poller::start(reader, events)),
        }
    }

    /// No source: only the windows give the keys.
    pub fn none() -> Self {
        Self {
            source: Source::None,
        }
    }

    /// The keys that the call needs now; absent without a call.
    pub fn set(&self, keys: Option<CallKeys>) {
        let keys = keys.map(CallKeys::resolved);
        match &self.source {
            Source::Poller(poller) => poller.set(keys),
            #[cfg(target_os = "linux")]
            Source::Portal { wanted, .. } => {
                wanted.send_modify(|request| request.keys = keys);
            }
            Source::None => {}
        }
    }

    pub fn state(&self) -> KeysState {
        match &self.source {
            Source::Poller(poller) => match poller.failure() {
                Some(error) => KeysState::Failed(error),
                None => KeysState::Reading,
            },
            #[cfg(target_os = "linux")]
            Source::Portal { failure, .. } => {
                match failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                {
                    Some(reason) => KeysState::PortalFailed(reason),
                    None => KeysState::Portal,
                }
            }
            Source::None => KeysState::WindowOnly,
        }
    }
}

/// What a reader of the tests finds at its next readings.
#[cfg(any(test, feature = "test-support"))]
type Reading = Option<Result<Vec<Key>, ReadError>>;

/// A reader whose held keys a test sets, and which counts its readings.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Default)]
pub struct HeldKeys {
    held: std::sync::Arc<std::sync::Mutex<Reading>>,
    readings: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(any(test, feature = "test-support"))]
impl HeldKeys {
    /// The keys that the next readings find held, or why they fail.
    pub fn hold(&self, held: Result<Vec<Key>, ReadError>) {
        *self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(held);
    }

    pub fn readings(&self) -> usize {
        self.readings.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl KeyReader for HeldKeys {
    fn held(&mut self, keys: &[Key]) -> Result<Vec<Key>, ReadError> {
        self.readings
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or(Ok(Vec::new()))?;
        Ok(held.into_iter().filter(|key| keys.contains(key)).collect())
    }
}

#[cfg(test)]
mod tests;
