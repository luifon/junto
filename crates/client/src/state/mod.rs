//! The state of the application: the client core, what the person selected, and the work
//! that keeps the window current (ADR 0730).
//!
//! One entity holds all of it. It wakes on every change of the client core and of the
//! event stream of each server, and tells the window to draw again. The window reads plain
//! data through [`AppState::view`] and calls the methods here for what a person does.

pub mod account;
pub mod admin;
pub mod book;
mod call_keys;
mod history;
pub use call_keys::KeybindChange;
mod call_sounds;
pub mod contacts;
pub mod feed;
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;
mod followed;
pub mod linking;
pub mod logo;
mod messages;
pub mod moderation;
pub mod notices;
mod notifying;
pub mod people;
mod permissions;
pub mod person_drop;
mod progress;
pub mod receipts;
pub mod recorded;
mod recorded_work;
pub mod recording;
pub mod search;
mod searching;
mod threads;
pub mod transcript_work;
mod tray;
pub mod update_install;
pub mod view;

pub use messages::{AttachRefusal, Measured, measure};
pub use notifying::MentionSwitch;
pub use recorded_work::RecordedFile;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_channel::oneshot;
use gpui::{Context, Task};
use junto_client_core::api::ApiError;
use junto_client_core::book::ConnectionMode;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::live::Notice;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::call::{EntryOutcome, InvitationOutcome, UpdateCallSettings};
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{
    AccountId, AttachmentId, CallInvitationId, ChannelId, EntryRequestId, MessageId, RecordingId,
    RecoveryId, ServerId, WorkspaceId,
};
use junto_client_core::settings::{InterfaceSettings, ThemeChoice};
use junto_client_core::transfers::{Download, RecordingDownload, Upload};
use junto_client_core::voice::{InputMode, ShareResolution, TalkKey, VoiceSettings};
use junto_client_core::{Client, ServerHandle};
use junto_client_core::{EntryKey, Owner};
use junto_media::audio::{DeviceId, Processing, level};
use junto_media::calls::{ActiveCall, CallTarget, Calls, MediaOptions, Seat};
use junto_media::engine::{Levels, MicrophoneState, ShareId, VoiceConfig};
use junto_media::permission::MicrophoneAccess;
use junto_media::share::{CaptureRequest, PictureSource, Resolution, ShareQuality, SoundChoice};

use crate::text;
use crate::theme;
use view::{
    INPUT_SEGMENTS, Known, LEVEL_SEGMENTS, MediaSnapshot, MicrophoneAction, Selection,
    ServerSnapshot, ShellView, Shown, ShownLevels,
};

/// The times of the work that the application does by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How often a request that waits for an administrator, or a device link, is checked.
    pub decision_interval: Duration,
    /// How often a server that could not be reached is contacted again.
    pub retry_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            decision_interval: Duration::from_secs(5),
            retry_interval: Duration::from_secs(20),
        }
    }
}

/// The writes of files that a window started, in the order in which they were asked for.
/// The queue belongs to the application and not to a window: a write that was accepted is
/// done after its window has closed, and the application waits for the queue before it
/// ends (see [`finish_writes`]). The writes run on background threads, so they go on
/// while the interface thread waits for them at the end.
#[derive(Default)]
struct Writes {
    last: Option<Task<()>>,
}

impl gpui::Global for Writes {}

/// How long the application waits at its end for the writes that were accepted. A write
/// takes this long only when another program holds the lock of the data directory.
const WRITE_WAIT: Duration = Duration::from_secs(5);

/// Waits for the writes that still wait or run. The application calls this when it quits,
/// on the interface thread.
pub fn finish_writes(cx: &mut gpui::App) {
    let Some(last) = cx.default_global::<Writes>().last.take() else {
        return;
    };
    if cx
        .foreground_executor()
        .block_with_timeout(WRITE_WAIT, last)
        .is_err()
    {
        tracing::error!("a write of a file had not ended when the application ended");
    }
}

/// A setting of the interface, for the failure of its write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Setting {
    Scale,
    ServerBook,
    Theme,
}

/// A part of the processing of the microphone (ADR 0701).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingPart {
    EchoCancellation,
    NoiseSuppression,
    AutomaticGain,
}

/// How long the application waits at its end for the server to be told that the call was
/// left.
const LEAVE_WAIT: Duration = Duration::from_secs(2);

/// The sound settings as the media engine takes them.
fn voice_config(voice: &VoiceSettings) -> VoiceConfig {
    VoiceConfig {
        input_device: voice.input_device.clone().map(DeviceId),
        output_device: voice.output_device.clone().map(DeviceId),
        processing: Processing {
            echo_cancellation: voice.echo_cancellation,
            noise_suppression: voice.noise_suppression,
            automatic_gain: voice.automatic_gain,
        },
        push_to_talk: voice.input_mode == InputMode::PushToTalk,
        threshold: voice.voice_threshold,
    }
}

/// The levels as the meters show them. A meter has few segments, so most changes of a
/// level change nothing that is drawn.
fn shown_levels(levels: &Levels) -> ShownLevels {
    ShownLevels {
        input: level::segments(levels.input, INPUT_SEGMENTS),
        own: (
            level::segments(levels.own.level, LEVEL_SEGMENTS),
            levels.own.speaking,
        ),
        people: levels
            .people
            .iter()
            .map(|(account_id, person)| {
                (
                    *account_id,
                    (
                        level::segments(person.level, LEVEL_SEGMENTS),
                        person.speaking,
                    ),
                )
            })
            .collect(),
    }
}

/// The calls state that the own share ended by itself, and have not put it away.
fn share_ended(calls: &junto_media::calls::CallsView) -> bool {
    calls
        .call
        .as_ref()
        .is_some_and(|call| call.share_ended.is_some())
}

/// The quality of a share as the media engine takes it.
fn share_quality(voice: &VoiceSettings) -> ShareQuality {
    ShareQuality {
        resolution: match voice.share_resolution {
            ShareResolution::P720 => Resolution::P720,
            ShareResolution::P1080 => Resolution::P1080,
            ShareResolution::Source => Resolution::Source,
        },
        frame_rate: voice.share_frame_rate,
    }
}

/// Leaves the call of this device, ends a join that waits for its server, and waits a short
/// time for the server to be told. The application calls this when it quits, on the
/// interface thread.
pub fn leave_call_at_quit(calls: &Calls, cx: &mut gpui::App) {
    // Also without a call in the view: a join that the person ended can still wait for
    // its server, and gives up the place that it is given.
    let leaving = calls.leave();
    let _ = cx
        .foreground_executor()
        .block_with_timeout(LEAVE_WAIT, leaving);
}

/// How long a request to enter waits on the server (ADR 0514), when the answer of the
/// server does not tell it.
const ENTRY_REQUEST_LASTS: Duration = Duration::from_secs(60);
/// How long an invitation into a call waits on the server (ADR 0514), when the answer of
/// the server does not tell it.
const INVITATION_LASTS: Duration = Duration::from_secs(60);
/// How long after the end of a request to enter or of an invitation its outcome can still
/// arrive.
const ANSWER_MARGIN: Duration = Duration::from_secs(5);

/// One request about a call: its number, and the call that the device was in.
struct Asked {
    number: u64,
    identity: Option<String>,
}

/// What the person asked for and that has not ended yet, or has failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// Account deletions still awaiting their server answer or local cleanup.
    pub deleting: HashSet<Owner>,
    /// The recoveries whose acknowledgement is on its way, each with the owner of the
    /// server that it was sent for (ADR 0745): two servers can state the same identifier.
    pub acknowledging: HashSet<(Owner, RecoveryId)>,
    /// Why the last acknowledgement of a recovery failed, per owner of a server.
    pub acknowledge_failed: HashMap<Owner, String>,
    /// Why a server could not be removed.
    pub remove_failed: HashMap<ServerId, String>,
    /// The servers whose removal is on its way. A second removal of one of them is not
    /// started: it would be decided against the server book of a later moment.
    pub removing: HashSet<ServerId>,
    removal_entries: HashMap<ServerId, EntryKey>,
    /// The entries whose waiting request is being given up, by server. Each cancellation
    /// ends for its own entry, also when a removal of a later entry of the server has
    /// replaced its removal.
    cancelling: HashMap<ServerId, EntryKey>,
    /// The name of the profile preset is being written.
    pub saving_name: bool,
    /// Why the last request that started or stopped a recording failed.
    pub recording_failed: Option<String>,
    /// Why the server did not do what the person asked for in a call: a rule of the call,
    /// a missing permission, or no answer.
    pub call_refused: Option<String>,
    /// What the application tells the person about the call without a request of the
    /// person: the microphone that is open again after meeting mode, an invitation that
    /// was declined.
    pub call_told: Option<String>,
    /// Why the server did not do what the person asked for with a message or a file.
    /// The window shows it while the person stays where the request was made
    /// ([`AppState::message_refusal`]).
    pub message_refused: Option<Refusal>,
}

/// A refusal of a request about a message or a file, with the selection that the person
/// made it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub sentence: String,
    /// The count of the selections of the person when the request was made.
    navigation: u64,
}

impl Pending {
    fn begin_removal(&mut self, entry: EntryKey) -> bool {
        if self.removal_entries.get(&entry.server_id) == Some(&entry) {
            return false;
        }
        self.removal_entries.insert(entry.server_id, entry);
        self.removing.insert(entry.server_id);
        self.remove_failed.remove(&entry.server_id);
        true
    }

    fn end_removal(&mut self, entry: EntryKey) -> bool {
        if self.removal_entries.get(&entry.server_id) != Some(&entry) {
            return false;
        }
        self.removal_entries.remove(&entry.server_id);
        self.removing.remove(&entry.server_id);
        true
    }

    fn begin_cancellation(&mut self, entry: EntryKey) -> bool {
        if self.cancels(entry) {
            return false;
        }
        self.cancelling.insert(entry.server_id, entry);
        true
    }

    fn end_cancellation(&mut self, entry: EntryKey) {
        if self.cancels(entry) {
            self.cancelling.remove(&entry.server_id);
        }
    }

    /// True while the waiting request of this entry is being given up.
    pub fn cancels(&self, entry: EntryKey) -> bool {
        self.cancelling.get(&entry.server_id) == Some(&entry)
    }

    /// True while a waiting request of any entry of this server is being given up.
    pub fn cancels_on(&self, server_id: ServerId) -> bool {
        self.cancelling.contains_key(&server_id)
    }

    /// True while the acknowledgement of this recovery is on its way for the owner of a
    /// server that the window shows.
    pub fn acknowledges(&self, owner: Option<Owner>, recovery_id: RecoveryId) -> bool {
        owner.is_some_and(|owner| self.acknowledging.contains(&(owner, recovery_id)))
    }

    /// Why the last acknowledgement of a recovery failed for the owner of a server.
    pub fn acknowledge_failure(&self, owner: Option<Owner>) -> Option<&str> {
        owner
            .and_then(|owner| self.acknowledge_failed.get(&owner))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod pending_tests {
    use junto_client_core::EntryKey;
    use junto_client_core::protocol::ids::AccountId;

    use super::*;

    fn owner() -> Owner {
        Owner {
            entry: EntryKey {
                server_id: ServerId::generate(),
                generation: 1,
            },
            account_id: AccountId::generate(),
        }
    }

    #[test]
    fn recovery_acknowledgements_are_independent_across_servers() {
        let (first, second) = (owner(), owner());
        let recovery = RecoveryId::generate();
        let mut pending = Pending::default();
        // The first server sends the acknowledgement of a recovery. The second server
        // states the same identifier: its notice can be acknowledged, and is not shown as
        // being sent.
        assert!(pending.acknowledging.insert((first, recovery)));
        assert!(pending.acknowledges(Some(first), recovery));
        assert!(!pending.acknowledges(Some(second), recovery));
        assert!(pending.acknowledging.insert((second, recovery)));
        // The next account of the first entry, and the entry added again, are other owners.
        let mut account = first;
        account.account_id = AccountId::generate();
        let mut added_again = first;
        added_again.entry.generation += 1;
        assert!(!pending.acknowledges(Some(account), recovery));
        assert!(!pending.acknowledges(Some(added_again), recovery));
        assert!(!pending.acknowledges(None, recovery));
        // A failure is told for the owner that sent the acknowledgement only.
        pending
            .acknowledge_failed
            .insert(first, "failed".to_owned());
        assert_eq!(pending.acknowledge_failure(Some(first)), Some("failed"));
        assert_eq!(pending.acknowledge_failure(Some(second)), None);
        assert_eq!(pending.acknowledge_failure(Some(account)), None);
    }

    #[test]
    fn a_cancellation_ends_only_for_its_own_entry() {
        let first = owner().entry;
        let second = EntryKey {
            generation: first.generation + 1,
            ..first
        };
        let mut pending = Pending::default();
        assert!(pending.begin_cancellation(first));
        assert!(!pending.begin_cancellation(first));
        assert!(pending.begin_cancellation(second));
        pending.end_cancellation(first);
        assert!(pending.cancels(second) && !pending.cancels(first));
        pending.end_cancellation(second);
        assert!(!pending.cancels(second));
    }
}

/// The levels of the call of this device changed, and nothing else. A window puts them
/// into the view that it holds ([`view::ShellView::show_levels`]) and does not derive the
/// view again (ADR 0730).
pub struct LevelsChanged;

impl gpui::EventEmitter<LevelsChanged> for AppState {}

/// A press outside the window, on a system notification, asks the main window to show a page
/// of the administration.
pub struct OpenAdmin(pub admin::AdminOpen);

impl gpui::EventEmitter<OpenAdmin> for AppState {}

/// A key that works while another application is in front asks the main window, now in
/// front, for an action that needs a window: show the call, or share (ADR 0777).
pub struct WindowKeyAction(pub junto_client_core::keybinds::KeyAction);

impl gpui::EventEmitter<WindowKeyAction> for AppState {}

/// What the call window does before it closes.
type CallWindowClosing = Box<dyn FnOnce(&mut gpui::App)>;

pub struct AppState {
    talking_windows: HashSet<gpui::EntityId>,
    /// The source of the system holds the key of push to talk (ADR 0776).
    talking_globally: bool,
    /// The keys of a call in every application, and what they were told last.
    global_keys: junto_client_core::global_keys::GlobalKeys,
    call_keys_told: Option<junto_client_core::global_keys::CallKeys>,
    /// The key bindings of the person (ADR 0777), and why their last change was not written.
    keybinds: junto_client_core::keybinds::Keybinds,
    keybinds_error: Option<String>,
    keybind_recordings: HashSet<gpui::EntityId>,
    keybind_boundary: u64,
    keybind_global_ready: bool,
    /// The windows (by their entity) and the keyboard of the system (`None`) that hold the
    /// keys of push to mute, and whether their hold closed the microphone.
    mute_holds: HashSet<Option<gpui::EntityId>>,
    muted_by_hold: bool,
    /// The places that the window showed, for Back and Forward (ADR 0777).
    history: history::History,
    call_window: Option<gpui::WindowHandle<gpui_base::Root>>,
    /// What the call window does before it closes: it gives the text of its composer to
    /// the main window (ADR 0772).
    call_window_closing: Option<CallWindowClosing>,
    call_window_settings: junto_client_core::call_window::CallWindowSettings,
    call_window_writes: std::sync::Arc<std::sync::Mutex<call_window::Writes>>,
    direct: direct::DirectCalls,
    ringer: junto_media::audio::ring::RingPlayer,
    /// The short tones of the events of the call of this device (ADR 0707).
    sounds: junto_media::audio::sounds::SoundPlayer,
    /// What the last tones of the call were decided from.
    heard: call_sounds::Heard,
    client: Client,
    calls: Calls,
    /// Why the sound settings are not in their file.
    voice_failed: Option<String>,
    camera_devices: Vec<junto_media::camera::CameraDevice>,
    /// What is told about the recording of the call of this device.
    recording: recording::RecordingTold,
    /// The media snapshot that the view is derived from in place of the calls.
    #[cfg(feature = "test-support")]
    media_fixture: Option<MediaSnapshot>,
    timing: Timing,
    settings: InterfaceSettings,
    /// The interface scale of this start, from a start argument. It is not stored.
    scale_of_this_start: Option<u32>,
    /// The result of the last change of the profile name, until the window has taken it.
    name_result: Option<(String, Result<(), String>)>,
    /// Why a setting could not be written. The failure of one setting stays until that
    /// setting is in the file.
    settings_failed: HashMap<Setting, String>,
    selection: Selection,
    known: Known,
    /// The time zone of the system as it was read last.
    system_zone: chrono_tz::Tz,
    /// How many readings of the zone of the system were started. Only the result of the
    /// newest one is applied, because they run in parallel.
    zone_readings: u64,
    /// What a test reads in place of the zone of the system.
    zone_reader: Option<std::sync::Arc<dyn Fn() -> chrono_tz::Tz + Send + Sync>>,
    pending: Pending,
    /// What the administration holds: the open page and its requests (ADR 0761).
    admin: admin::AdminState,
    /// What the account layer holds: the open page, its requests and the devices that it
    /// lists (ADR 0781).
    account: account::AccountState,
    /// The chooser of a new direct conversation (ADR 0307).
    contacts: contacts::ContactSearch,
    /// The role and the flow of the link on all servers (ADR 0782).
    linking: linking::LinkingState,
    /// What the person did with the update notice, and a move of a server that failed
    /// (ADR 0780).
    book: book::BookState,
    update_install: update_install::InstallState,
    /// What the window showed after the last change. Side effects follow a change of it.
    shown: Shown,
    /// The picture of the profile preset, and where a change of it stands.
    preset_picture: people::PresetPicture,
    /// The people whose profiles the window reads (ADR 0783).
    people_followed: people::Followed,
    /// The images of the logos of the servers, and where the read of each stands
    /// (ADR 0101).
    logos: logo::Logos,
    /// The workspace whose members the window needs, and the task that reads them.
    members: Option<((ServerId, WorkspaceId), Task<()>)>,
    /// One task per saved server that wakes on each change of its event stream.
    watchers: HashMap<ServerId, Task<()>>,
    /// One task per server that waits for a decision of another person.
    waiting: HashMap<ServerId, Task<()>>,
    /// One task per saved server that reads each change of its state by itself.
    notice_readers: HashMap<ServerId, Task<()>>,
    /// The number of requests to enter that the person made. An answer of the server
    /// belongs to the request with its number.
    entry_asks: u64,
    /// The answers to requests to enter that arrived while the request of the person had
    /// no identifier yet.
    early_entry_answers: Vec<(EntryRequestId, EntryOutcome)>,
    /// The number of requests about a call that the person made.
    call_asks: u64,
    /// The invitations into a call that the person made on this device and that can
    /// still wait.
    made_invitations: HashSet<(ServerId, CallInvitationId)>,
    /// The number of invitations of the person whose answer of the server has not arrived.
    invitations_on_the_way: usize,
    /// How the invitations ended that the server told while an invitation of the person was
    /// on its way. The invited person can answer before the answer of the server to the
    /// invitation arrives.
    early_invitation_ends: HashMap<(ServerId, CallInvitationId), InvitationOutcome>,
    /// The places whose messages the window shows, with the task that opens each in the
    /// client core.
    open_places: HashMap<(ServerId, PlaceId), Task<()>>,
    /// The channel whose pin list the window shows, with the task that opens it in the
    /// client core.
    open_pins: Option<((ServerId, ChannelId), Task<()>)>,
    /// The number of the last fork that the person asked for. An answer to an earlier
    /// request does not change what the dialog shows.
    fork_asks: u64,
    /// The number of the last direct conversation that the person asked for.
    conversation_asks: u64,
    /// How often the person selected a server, a workspace, a channel, a direct
    /// conversation or the call. An answer that arrives after such a selection does not
    /// move the person to another place.
    navigations: u64,
    /// The messages of replies that were asked for from the server.
    /// The quoted messages that were asked for, with the number of the read.
    asked_quotes: HashMap<(ServerId, MessageId), u64>,
    quote_asks: u64,
    /// The account that uploaded the files that wait for a message, by server.
    upload_owners: HashMap<ServerId, AccountId>,
    /// The uploads of the files that the person attached to the next message of a place.
    uploads: HashMap<(ServerId, PlaceId), Vec<Upload>>,
    /// The uploads of the messages that wait to be sent, by the number of the message.
    sent_uploads: HashMap<(ServerId, u64), Vec<Upload>>,
    downloads: HashMap<(ServerId, AttachmentId), Download>,
    /// The owner that each server had at the last look. The window lists that are keyed
    /// by a server hold what one owner read or saved: they are emptied for a server when
    /// its owner is another one, or none (ADR 0745).
    owners: HashMap<ServerId, junto_client_core::Owner>,
    /// Where files are saved in place of the download directory of the person.
    download_directory: Option<std::path::PathBuf>,
    /// The number of requests about messages that the person made.
    message_asks: u64,
    /// The requests for a change of a message that have no answer yet.
    message_asks_flying: usize,
    /// The position of the list of each open place, as the client core was told it.
    anchors: HashMap<(ServerId, PlaceId), Option<u64>>,
    /// The recordings that the window shows, with the task that opens each in the
    /// client core.
    open_recordings: HashMap<(ServerId, RecordingId), Task<()>>,
    /// The downloads of the files of recordings.
    recording_downloads: HashMap<(ServerId, RecordingId, RecordedFile), RecordingDownload>,
    /// The number of the download of each file that the window follows, and the number
    /// of the last download.
    recording_download_numbers: HashMap<(ServerId, RecordingId, RecordedFile), u64>,
    download_asks: u64,
    /// The number of the last correction or name that the person sent for a transcript.
    transcript_asks: u64,
    /// The downloads whose file is opened when it is saved.
    opening: HashSet<(ServerId, RecordingId, RecordedFile)>,
    /// A saved file is opened with the application of the system for it, and shown in the
    /// file manager of the system.
    uses_system_applications: bool,
    /// The files that a start without the applications of the system would have opened.
    kept_opened: Vec<std::path::PathBuf>,
    /// The files that a start without the applications of the system would have shown.
    kept_shown: Vec<std::path::PathBuf>,
    /// The number of the last request of a search. An answer to an earlier request is
    /// not used.
    search_asks: u64,
    /// How many answers of a search reached the window, also the ones that it did not use.
    #[cfg(feature = "test-support")]
    search_answers: u64,
    /// The continuation of the search of the window, for its next page.
    search_next: Option<String>,
    /// The jump to a message that waits until the client core holds the list of its
    /// place.
    jump: Option<notices::Jump>,
    /// The jump to a search result by its position, until the list of its place shows
    /// whether the message is there.
    result_jump: Option<notices::Jump>,
    /// The transcript, the line and the opening of the side panel for which the lines
    /// around that line were asked for.
    lines_asked_around: Option<(ServerId, RecordingId, u32, u64)>,
    /// The number of the last read of moderation notices with a proof of the key.
    moderation_asks: u64,
    /// The permissions of the operating system (ADR 0708).
    permissions: permissions::PermissionsState,
    /// The notifications of the system.
    notifier: std::rc::Rc<dyn crate::notify::SystemNotifier>,
    /// The notifier of a start without the notifications of the system.
    kept: crate::notify::KeptNotifier,
    /// The tray icon (ADR 0759).
    tray: std::rc::Rc<dyn crate::tray::SystemTray>,
    /// The tray of a start without the tray icon of the system.
    kept_tray: crate::tray::KeptTray,
    /// The state that the tray was told last.
    tray_shown: Option<crate::tray::TrayState>,
    /// The unread mentions that the tray icon shows with its mark (ADR 0759).
    tray_mentions: u64,
    /// The main window is the active window of the system.
    window_active: bool,
    /// The call window is the active window of the system.
    call_window_active: bool,
    /// The panel of the call window shows the notices of the server of the call.
    call_window_notices: bool,
    /// The main area showed the view of the call of this device at the last change.
    call_shown: bool,
    /// The shown server and place and the call of this device at the last change.
    shown_with_call: (Option<ServerId>, Option<PlaceId>, Option<CallTarget>),
    /// What was seen of the notices and the invitations of each server.
    seen: HashMap<ServerId, notices::Seen>,
    /// The number of the last read of the message of a search result. The answer of an
    /// earlier read leads nowhere.
    result_asks: u64,
    /// How many reads of the message of a search result have ended.
    result_answers: u64,
    /// How often the state of each server was replaced.
    reloads: HashMap<ServerId, u64>,
    /// The read marks of notices that wait until the marks before them were sent.
    marks_waiting: HashMap<ServerId, Vec<uuid::Uuid>>,
    /// The read marks of notices that are on their way.
    marks_sending: HashMap<ServerId, Vec<uuid::Uuid>>,
    /// The servers that the view is derived from in place of the client core.
    #[cfg(feature = "test-support")]
    fixture: Option<Vec<ServerSnapshot>>,
    /// The time that the view is derived for in place of the time of the system.
    #[cfg(feature = "test-support")]
    clock_fixture: Option<time::OffsetDateTime>,
    /// The panics of this data directory since the start, until the person closes the
    /// notice (ADR 0757).
    panic_notice: Option<PanicNotice>,
    _changes: Task<()>,
    _retry: Task<()>,
    _media: [Task<()>; 3],
    _panics: Task<()>,
}

/// How long the task of the panic reports waits after a panic in its own update.
const PAUSE_AFTER_A_PANIC: std::time::Duration = std::time::Duration::from_secs(1);

/// What one update of a task that follows a source for the life of the state gave.
enum Updated<R> {
    Done(R),
    /// The state is gone: the task ends.
    Gone,
    /// The update panicked. The hook recorded the panic; the task waits
    /// [`PAUSE_AFTER_A_PANIC`] and updates again, so that the state reads its source again.
    Panicked,
}

/// Runs one update of a task that follows a source for the life of the state: the calls, the
/// levels, the changes of the client core, the clock, the retries and the panic reports. A
/// panic in the update, such as one in an observer that the update notifies, ends this
/// update only; without the stop the panic would end the task, and the state would no longer
/// follow its source (ADR 0757).
/// `source` names the task for the tests of ADR 0757, which read which tasks paused.
fn update_once<R>(
    source: &'static str,
    this: &gpui::WeakEntity<AppState>,
    cx: &mut gpui::AsyncApp,
    update: impl FnOnce(&mut AppState, &mut Context<AppState>) -> R,
) -> Updated<R> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.update(cx, update))) {
        Ok(Ok(value)) => Updated::Done(value),
        Ok(Err(_)) => Updated::Gone,
        Err(_) => {
            #[cfg(feature = "test-support")]
            crate::panics::test_support::task_paused(source);
            #[cfg(not(feature = "test-support"))]
            let _ = source;
            Updated::Panicked
        }
    }
}

/// The notice of the panics that the panic hook recorded for the data directory of the
/// window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanicNotice {
    /// The panics since the notice was last closed.
    pub count: usize,
    /// The latest of them.
    pub latest: crate::panics::Report,
}

impl Drop for AppState {
    fn drop(&mut self) {
        // The call does not outlive the application. The leave runs to its end on the
        // runtime of the client.
        drop(self.calls.leave());
    }
}

impl AppState {
    /// Starts the event streams, contacts every saved server and follows their changes.
    pub fn new(
        client: Client,
        timing: Timing,
        media: MediaOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = match client.settings().load() {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(%error, "the interface settings were not read");
                InterfaceSettings::default()
            }
        };
        theme::apply(settings.theme, cx);
        let keybinds = client.settings().load_keybinds().unwrap_or_else(|error| {
            tracing::warn!(%error, "the key bindings were not read");
            junto_client_core::keybinds::Keybinds::default()
        });
        let (voice, voice_failed) = match client.voice().load() {
            Ok(voice) => (voice, None),
            Err(error) => {
                tracing::warn!(%error, "the sound settings were not read");
                let failure = text::flow_error(&FlowError::Storage(error.to_string()));
                (VoiceSettings::default(), Some(failure))
            }
        };
        let ringer = junto_media::audio::ring::RingPlayer::new(media.host.clone());
        let sounds = junto_media::audio::sounds::SoundPlayer::new(media.host.clone());
        let calls = Calls::new(client.clone(), media);
        if voice_failed.is_some() {
            // The settings of the person were not read, and the defaults send an open
            // microphone all the time. The microphone is closed until the person opens it.
            calls.mute();
        }
        calls.set_voice(voice_config(&voice));
        calls.set_share_volume(f32::from(voice.share_volume) / 100.0);
        for (server_id, accounts) in &voice.volumes {
            for (account_id, percent) in accounts {
                calls.set_volume(*server_id, *account_id, f32::from(*percent) / 100.0);
            }
        }
        let mut calls_watch = calls.watch();
        let follow_calls = cx.spawn(async move |this, cx| {
            while calls_watch.changed().await.is_ok() {
                match update_once("calls", &this, cx, |this, cx| this.calls_changed(cx)) {
                    Updated::Done(()) => {}
                    Updated::Gone => break,
                    Updated::Panicked => {
                        calls_watch.mark_changed();
                        cx.background_executor().timer(PAUSE_AFTER_A_PANIC).await;
                    }
                }
            }
        });
        let mut levels = calls.levels();
        let follow_levels = cx.spawn(async move |this, cx| {
            while levels.changed().await.is_ok() {
                let shown = shown_levels(&levels.borrow_and_update());
                let told = update_once("levels", &this, cx, |this, cx| {
                    if this.known.media.levels != shown {
                        this.known.media.levels = shown;
                        cx.emit(LevelsChanged);
                    }
                });
                match told {
                    Updated::Done(()) => {}
                    Updated::Gone => break,
                    Updated::Panicked => {
                        levels.mark_changed();
                        cx.background_executor().timer(PAUSE_AFTER_A_PANIC).await;
                    }
                }
            }
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let told = update_once("clock", &this, cx, |this, cx| {
                    let was_ringing = this.direct.ringing();
                    this.reconcile_direct(cx);
                    if was_ringing || this.direct.ringing() {
                        cx.notify();
                    }
                    if this.known.media.calls.call.is_some() {
                        this.known.media.now = Some(time::OffsetDateTime::now_utc());
                        cx.notify();
                    }
                });
                if let Updated::Gone = told {
                    break;
                }
            }
        });
        if let Err(error) = client.start_event_streams() {
            tracing::error!(%error, "the event streams did not start");
        }
        let connecting = client.connect_all();
        cx.spawn(async move |_, _| {
            connecting.await;
        })
        .detach();

        let mut changes = client.changes();
        let follow = cx.spawn(async move |this, cx| {
            let mut again = false;
            while again || changes.changed().await {
                again = false;
                match update_once("changes", &this, cx, |this, cx| this.changed(cx)) {
                    Updated::Done(()) => {}
                    Updated::Gone => break,
                    Updated::Panicked => {
                        again = true;
                        cx.background_executor().timer(PAUSE_AFTER_A_PANIC).await;
                    }
                }
            }
        });
        let mut reports = crate::panics::watch(client.data_dir().root());
        let panics = cx.spawn(async move |this, cx| {
            use futures_util::StreamExt as _;
            while let Some(report) = reports.next().await {
                // The reports that wait are taken into the same update.
                let mut batch = vec![report];
                while let Ok(report) = reports.try_recv() {
                    batch.push(report);
                }
                // A panic in this update, such as one in a draw or an observer that the notice
                // starts, is recorded by the hook and ends this update only. The task then waits
                // before it takes the next reports, so that an update that panics at each run
                // returns to the event loop between two runs (ADR 0757).
                match update_once("reports", &this, cx, |this, cx| {
                    this.panics_recorded(batch, cx)
                }) {
                    Updated::Done(()) => {}
                    Updated::Gone => break,
                    Updated::Panicked => {
                        cx.background_executor().timer(PAUSE_AFTER_A_PANIC).await;
                    }
                }
            }
        });
        let retry_interval = timing.retry_interval;
        let retry = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(retry_interval).await;
                if let Updated::Gone =
                    update_once("retry", &this, cx, |this, cx| this.retry_unreached(cx))
                {
                    break;
                }
            }
        });
        // The one read of the zone of the system on the thread of the window, at the start.
        // Later reads run on a background thread (ADR 0730).
        #[allow(clippy::disallowed_methods)]
        let system_zone = crate::zones::system_zone();
        let known = Known {
            zone: feed::Zone::Named(system_zone),
            preset_name: client
                .profile()
                .load()
                .map(|preset| preset.name)
                .unwrap_or_default(),
            media: MediaSnapshot {
                calls: calls.view(),
                voice,
                can_share: calls.can_share(),
                ..MediaSnapshot::default()
            },
            ..Known::default()
        };
        let kept = crate::notify::KeptNotifier::default();
        let kept_tray = crate::tray::KeptTray::default();
        let call_window_settings =
            junto_client_core::call_window::CallWindowStore::new(client.data_dir().clone())
                .load()
                .unwrap_or_default();
        let mut state = Self {
            talking_windows: HashSet::new(),
            talking_globally: false,
            global_keys: junto_client_core::global_keys::GlobalKeys::none(),
            call_keys_told: None,
            keybinds,
            keybinds_error: None,
            keybind_recordings: HashSet::new(),
            keybind_boundary: 0,
            keybind_global_ready: true,
            mute_holds: HashSet::new(),
            muted_by_hold: false,
            history: history::History::default(),
            call_window: None,
            call_window_closing: None,
            call_window_settings,
            call_window_writes: Default::default(),
            direct: direct::DirectCalls::default(),
            ringer,
            sounds,
            heard: call_sounds::Heard::default(),
            client,
            calls,
            voice_failed,
            camera_devices: Vec::new(),
            recording: recording::RecordingTold::default(),
            #[cfg(feature = "test-support")]
            media_fixture: None,
            timing,
            settings,
            scale_of_this_start: None,
            name_result: None,
            settings_failed: HashMap::new(),
            selection: Selection::default(),
            known,
            system_zone,
            zone_readings: 0,
            zone_reader: None,
            pending: Pending::default(),
            admin: admin::AdminState::default(),
            account: account::AccountState::default(),
            contacts: contacts::ContactSearch::default(),
            linking: linking::LinkingState::default(),
            book: book::BookState::default(),
            update_install: update_install::InstallState::new(),
            shown: Shown::default(),
            preset_picture: people::PresetPicture::default(),
            people_followed: people::Followed::default(),
            logos: logo::Logos::default(),
            members: None,
            watchers: HashMap::new(),
            waiting: HashMap::new(),
            notice_readers: HashMap::new(),
            entry_asks: 0,
            early_entry_answers: Vec::new(),
            call_asks: 0,
            made_invitations: HashSet::new(),
            invitations_on_the_way: 0,
            early_invitation_ends: HashMap::new(),
            open_places: HashMap::new(),
            open_pins: None,
            fork_asks: 0,
            conversation_asks: 0,
            navigations: 0,
            asked_quotes: HashMap::new(),
            quote_asks: 0,
            upload_owners: HashMap::new(),
            uploads: HashMap::new(),
            sent_uploads: HashMap::new(),
            downloads: HashMap::new(),
            download_directory: None,
            message_asks: 0,
            message_asks_flying: 0,
            owners: HashMap::new(),
            anchors: HashMap::new(),
            open_recordings: HashMap::new(),
            recording_downloads: HashMap::new(),
            recording_download_numbers: HashMap::new(),
            download_asks: 0,
            transcript_asks: 0,
            opening: HashSet::new(),
            uses_system_applications: false,
            kept_opened: Vec::new(),
            kept_shown: Vec::new(),
            search_asks: 0,
            #[cfg(feature = "test-support")]
            search_answers: 0,
            search_next: None,
            jump: None,
            result_jump: None,
            lines_asked_around: None,
            moderation_asks: 0,
            permissions: permissions::PermissionsState::new(),
            notifier: std::rc::Rc::new(kept.clone()),
            kept,
            tray: std::rc::Rc::new(kept_tray.clone()),
            kept_tray,
            tray_shown: None,
            tray_mentions: 0,
            window_active: false,
            call_window_active: false,
            call_window_notices: false,
            call_shown: false,
            shown_with_call: (None, None, None),
            seen: HashMap::new(),
            result_asks: 0,
            result_answers: 0,
            reloads: HashMap::new(),
            marks_waiting: HashMap::new(),
            marks_sending: HashMap::new(),
            #[cfg(feature = "test-support")]
            fixture: None,
            #[cfg(feature = "test-support")]
            clock_fixture: None,
            panic_notice: None,
            _changes: follow,
            _retry: retry,
            _media: [follow_calls, follow_levels, clock],
            _panics: panics,
        };
        if state.voice_failed.is_some() {
            // The microphone was closed for settings that were not read: the person
            // reads why, wherever the window is.
            state.pending.call_told = Some(text::SOUND_SETTINGS_NOT_READ.to_owned());
        }
        state.load_preset_picture(cx);
        state.changed(cx);
        state
    }

    fn panics_recorded(&mut self, reports: Vec<crate::panics::Report>, cx: &mut Context<Self>) {
        for report in reports {
            // The 1st, 10th, 100th, ... panic of each location, so that a panic that repeats
            // does not fill the log (ADR 0757).
            if crate::panics::is_a_power_of_ten(report.occurrence) {
                tracing::error!(
                    report = ?report.file,
                    location = %report.location,
                    occurrence = report.occurrence,
                    "a panic was recorded: {}",
                    report.message
                );
            }
            let count = self.panic_notice.as_ref().map_or(0, |notice| notice.count) + 1;
            self.panic_notice = Some(PanicNotice {
                count,
                latest: report,
            });
        }
        cx.notify();
    }

    /// The notice of the panics since the start or since the person closed it.
    pub fn panic_notice(&self) -> Option<&PanicNotice> {
        self.panic_notice.as_ref()
    }

    pub fn close_panic_notice(&mut self, cx: &mut Context<Self>) {
        if self.panic_notice.take().is_some() {
            cx.notify();
        }
    }

    pub fn is_call_detached(&self) -> bool {
        self.call_window.is_some()
    }

    pub(crate) fn call_action(&self) -> Option<CallAction> {
        self.known.media.calls.call.as_ref().map(CallAction::of)
    }

    pub fn calls(&self) -> &Calls {
        &self.calls
    }

    /// The levels of the call of this device as the meters show them.
    pub fn shown_levels(&self) -> &ShownLevels {
        #[cfg(feature = "test-support")]
        if let Some(media) = &self.media_fixture {
            return &media.levels;
        }
        &self.known.media.levels
    }

    /// The sound settings as the window uses them.
    pub fn voice(&self) -> &VoiceSettings {
        #[cfg(feature = "test-support")]
        if let Some(media) = &self.media_fixture {
            return &media.voice;
        }
        &self.known.media.voice
    }

    /// Why the sound settings that the window shows are not in their file.
    pub fn voice_error(&self) -> Option<&str> {
        self.voice_failed.as_deref()
    }

    /// Runs after each change of the calls: the view follows, and a call that begins reads
    /// the devices and the permission that its settings show.
    fn calls_changed(&mut self, cx: &mut Context<Self>) {
        let view = self.calls.view();
        // A call is known by the identity that the server gave the device for it. The
        // view can pass from one call to the next without a moment in no call.
        let now = view.call.as_ref().map(|call| call.identity.clone());
        let (ended, entered) =
            call_window::call_transition(self.known.media.calls.call.as_ref(), view.call.as_ref());
        self.known.media.now = view.call.as_ref().map(|_| time::OffsetDateTime::now_utc());
        self.recording.in_call(now.as_deref());
        self.show_recording();
        if ended {
            self.return_call(cx);
            // What was told about a call ends with it.
            self.pending.call_refused = None;
            self.pending.call_told = None;
        } else if let (Some(held), Some(call)) = (&self.known.media.calls.call, &view.call)
            && held.microphone_block.is_some()
            && call.microphone_block.is_none()
            && !view.muted
            // With push-to-talk the microphone sends only while the person holds the key.
            && self.known.media.voice.input_mode != InputMode::PushToTalk
            // A microphone that the device cannot use sends nothing.
            && call.access != MicrophoneAccess::Refused
            && !matches!(
                call.microphone,
                MicrophoneState::NoPermission | MicrophoneState::Failed(_)
            )
        {
            // A block of the microphone ended, by the end of meeting mode or by a
            // moderator, and the person's own setting is "open": the people in the call
            // hear the person again without an action of the person, who is told so
            // (ADR 0505). While another person runs meeting mode, the calls close the
            // own setting in the step before (ADR 0739), so nothing is told then.
            self.pending.call_told = Some(text::MICROPHONE_OPEN_AGAIN.to_owned());
        }
        if let Some(unused) = view.unused {
            // An action of the person named another call or seat, and opened nothing
            // (ADR 0788). The person is told, and the calls put the statement away.
            self.pending.call_told = Some(text::unused_action(unused).to_owned());
            self.calls.dismiss_unused();
        }
        if let Some(why) = view.closed {
            // The calls closed the microphone setting by a rule. The person reads why.
            self.pending.call_told = Some(text::microphone_closed(why).to_owned());
            self.calls.dismiss_closed();
        }
        self.known.media.calls = view;
        self.known.share_notice_dismissed &= share_ended(&self.shown_calls());
        if entered {
            // The person is in a call: a request to enter another one is withdrawn, so
            // that its acceptance does not replace this call.
            self.withdraw_entry(cx);
            self.refresh_devices(cx);
            if self
                .known
                .media
                .calls
                .call
                .as_ref()
                .is_some_and(|call| matches!(call.target.place, CallPlace::Direct { .. }))
            {
                self.show_call(cx);
            }
        }
        self.changed(cx);
    }

    /// The identity that the server gave this device for its call.
    fn call_identity(&self) -> Option<String> {
        self.known
            .media
            .calls
            .call
            .as_ref()
            .map(|call| call.identity.clone())
    }

    /// Tells the person something about the call that the person did not ask for.
    pub fn tell_call(&mut self, told: &str, cx: &mut Context<Self>) {
        self.pending.call_told = Some(told.to_owned());
        cx.notify();
    }

    /// Starts a share of this device at the seat for which the person chose it, with the
    /// stored resolution and frame rate; the shares that run go on. The result is in the
    /// view. `dialog_parent` names the window over which the portal of Linux shows its
    /// dialog.
    pub fn start_share(
        &mut self,
        seat: &Seat,
        picture: PictureSource,
        sound: SoundChoice,
        dialog_parent: Option<String>,
    ) {
        let request = CaptureRequest {
            picture,
            sound,
            quality: share_quality(&self.known.media.voice),
            dialog_parent,
        };
        self.calls.start_share_in(seat, request);
    }

    /// Ends the share of this device in `slot`, or every share without a slot, and puts
    /// away the statement that a share ended.
    pub fn stop_share(&mut self, slot: Option<u8>) {
        self.calls.stop_share(slot);
    }

    /// Puts away the notice of the own share that ended by itself. The notice goes at
    /// once: the calls put the ended share away on their own threads, and until then they
    /// still state the end.
    pub fn dismiss_share_notice(&mut self, cx: &mut Context<Self>) {
        self.known.share_notice_dismissed = true;
        self.calls.forget_share_end();
        self.changed(cx);
    }

    pub fn watch_share(&mut self, share: ShareId) {
        self.calls.watch_share(share);
    }

    /// Uses a volume for the sound of the watched share at once, while the person moves
    /// the control. [`AppState::store_share_volume`] writes it.
    pub fn set_share_volume(&mut self, percent: u16, cx: &mut Context<Self>) {
        let percent = percent.min(junto_client_core::voice::VOLUME_MAX_PERCENT);
        self.known.media.voice.share_volume = percent;
        self.calls.set_share_volume(f32::from(percent) / 100.0);
        cx.notify();
    }

    pub fn store_share_volume(&mut self, cx: &mut Context<Self>) {
        let percent = self.known.media.voice.share_volume;
        self.write_voice(cx, move |voice| voice.share_volume = percent);
    }

    /// The largest picture of the next share of this device.
    pub fn set_share_resolution(&mut self, resolution: ShareResolution, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| voice.share_resolution = resolution);
    }

    pub fn set_share_frame_rate(&mut self, frame_rate: u32, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| voice.share_frame_rate = frame_rate);
    }

    pub fn set_hide_without_video(&mut self, hide: bool, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| voice.hide_people_without_video = hide);
    }

    /// Plays the tones of call events on another host than the sound of the calls.
    #[cfg(feature = "test-support")]
    pub fn use_sounds_host(&mut self, host: std::sync::Arc<dyn junto_media::audio::AudioHost>) {
        self.sounds = junto_media::audio::sounds::SoundPlayer::new(host);
    }

    /// Turns the short tones of the events of a call on or off (ADR 0707).
    pub fn set_call_sounds(&mut self, on: bool, cx: &mut Context<Self>) {
        // The tones that wait or play end with the switch.
        if !on {
            self.sounds.cancel();
        }
        self.change_voice(cx, move |voice| voice.call_sounds = on);
    }

    /// Plays the tones of the change of the call of this device since the last change
    /// (ADR 0707).
    fn play_call_sounds(&mut self) {
        let calls = &self.known.media.calls;
        let model = calls
            .call
            .as_ref()
            .and_then(|call| self.client.server(call.target.server_id))
            .and_then(|server| server.live().view().model);
        let participants = calls
            .call
            .as_ref()
            .zip(model.as_deref())
            .map(|(call, model)| {
                model
                    .call_at(&call.target.place)
                    .map(|listed| listed.participants.as_slice())
                    .unwrap_or_default()
            });
        let next = call_sounds::heard(
            calls,
            participants,
            model.as_deref().map(|model| model.account.id),
        );
        let before = std::mem::replace(&mut self.heard, next);
        let played = call_sounds::sounds(&before, &self.heard, self.known.media.voice.call_sounds);
        let device = self
            .heard
            .output
            .as_ref()
            .or(before.output.as_ref())
            .cloned();
        let device = device.or_else(|| self.chosen_output());
        for sound in played {
            self.sounds.play(sound, device.as_ref());
        }
    }

    /// The output device that the person chose, which a call and the tones use.
    fn chosen_output(&self) -> Option<DeviceId> {
        self.known.media.voice.output_device.clone().map(DeviceId)
    }

    /// The server and the voice channel of the call of this device.
    fn call_channel(&self) -> Option<(ServerHandle, ChannelId)> {
        let target = actionable_call(&self.known.media.calls, &self.calls.view())?;
        let channel_id = target.place.channel_id()?;
        Some((self.client.server(target.server_id)?, channel_id))
    }

    /// Starts a recording of the call of this device. `language` is the language of the
    /// speech for the transcript; without one the language of each sentence is detected
    /// (ADR 0603). The server answers at once and tells by an event whether the recorder
    /// took it (ADR 0604).
    pub fn start_recording(&mut self, language: Option<String>, cx: &mut Context<Self>) {
        self.request_recording(Some(language), cx);
    }

    /// Asks for the end of the recording of the call of this device, also while the
    /// recorder starts it.
    pub fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.request_recording(None, cx);
    }

    /// Gives the view what is told about the recording now.
    fn show_recording(&mut self) {
        self.known.recording_problem = self.recording.problem().cloned();
        self.known.recording_busy = self.recording.busy();
        self.pending.recording_failed = self.recording.request_failed().map(str::to_owned);
    }

    /// `start` holds the language of a recording that starts; `None` stops it.
    fn request_recording(&mut self, start: Option<Option<String>>, cx: &mut Context<Self>) {
        let Some((server, channel_id)) = self.call_channel() else {
            return;
        };
        let Some(request) = self.recording.request(start.is_some()) else {
            return;
        };
        self.show_recording();
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let result = match start {
                Some(language) => api
                    .start_recording(channel_id, language.as_deref())
                    .await
                    .map(|_| ()),
                None => api.stop_recording(channel_id).await,
            };
            let result = result.map_err(|error| text::flow_error(&FlowError::from(&error)));
            let _ = this.update(cx, |this, cx| {
                this.recording.answered(request, result);
                this.show_recording();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn dismiss_recording_error(&mut self, cx: &mut Context<Self>) {
        self.recording.dismiss_request_failure();
        self.show_recording();
        cx.notify();
    }

    /// Marks the start of a request about a call. What was told about the request before
    /// is put away.
    fn begin_ask(&mut self) -> Asked {
        self.call_asks += 1;
        self.pending.call_refused = None;
        Asked {
            number: self.call_asks,
            identity: self.call_identity(),
        }
    }

    /// Tells a refusal, when its request is still the last one and the device is still in
    /// the call in which the request was made. A late refusal of an earlier request does
    /// not replace what the person did afterwards.
    fn refused(&mut self, asked: &Asked, error: &ApiError, cx: &mut Context<Self>) {
        if self.call_asks == asked.number && self.call_identity() == asked.identity {
            self.pending.call_refused = Some(text::flow_error(&FlowError::from(error)));
            cx.notify();
        }
    }

    /// Asks a server for a change of a call. A refusal is told in
    /// [`Pending::call_refused`]; the result of a change arrives with the state.
    fn ask<F>(
        &mut self,
        server_id: ServerId,
        cx: &mut Context<Self>,
        ask: impl FnOnce(junto_client_core::api::ServerApi) -> F,
    ) where
        F: Future<Output = Result<(), ApiError>> + 'static,
    {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let asking = ask(server.api());
        let asked = self.begin_ask();
        cx.spawn(async move |this, cx| {
            if let Err(error) = asking.await {
                let _ = this.update(cx, |this, cx| this.refused(&asked, &error, cx));
            }
        })
        .detach();
        cx.notify();
    }

    /// As [`AppState::ask`], for the voice channel of the call of this device.
    fn ask_own_call<F>(
        &mut self,
        cx: &mut Context<Self>,
        ask: impl FnOnce(junto_client_core::api::ServerApi, ChannelId) -> F,
    ) where
        F: Future<Output = Result<(), ApiError>> + 'static,
    {
        let Some((server, channel_id)) = self.call_channel() else {
            return;
        };
        self.ask(server.id(), cx, move |api| ask(api, channel_id));
    }

    pub fn dismiss_call_refusal(&mut self, cx: &mut Context<Self>) {
        self.pending.call_refused = None;
        cx.notify();
    }

    pub fn dismiss_call_told(&mut self, cx: &mut Context<Self>) {
        self.pending.call_told = None;
        cx.notify();
    }

    /// Raises the hand of the person in the call, or lowers it (ADR 0512).
    pub fn set_hand(&mut self, raised: bool, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, _| async move {
            if raised {
                api.raise_hand().await
            } else {
                api.lower_hand().await
            }
        });
    }

    /// Opens the microphone of the person at the seat that the control was drawn for. The
    /// person does this after the presenter of meeting mode removed the block: the
    /// application never opens it by itself.
    pub fn open_microphone(&mut self, seat: &Seat) {
        self.calls.unmute_in(Some(seat));
    }

    /// Turns meeting mode on in the call of this device, or off (ADR 0505).
    pub fn set_meeting(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, channel_id| async move {
            if on {
                api.start_meeting(channel_id).await
            } else {
                api.end_meeting(channel_id).await
            }
        });
    }

    /// Locks the voice channel of the call of this device, or unlocks it (ADR 0504).
    pub fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, channel_id| async move {
            if locked {
                api.lock_call(channel_id).await
            } else {
                api.unlock_call(channel_id).await
            }
        });
    }

    /// Changes the user limit or the setting that hides who is inside (ADR 0514).
    pub fn update_room(&mut self, change: UpdateCallSettings, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, channel_id| async move {
            api.update_call_settings(channel_id, &change)
                .await
                .map(|_| ())
        });
    }

    /// Blocks the microphone of a person in a voice channel, or removes the block: a
    /// moderator, and the presenter of meeting mode who lets a person speak (ADR 0503,
    /// ADR 0512).
    pub fn set_microphone_block(
        &mut self,
        room: (ServerId, ChannelId),
        account_id: AccountId,
        block: bool,
        cx: &mut Context<Self>,
    ) {
        let (server_id, channel_id) = room;
        self.ask(server_id, cx, move |api| async move {
            if block {
                api.block_microphone(channel_id, account_id).await
            } else {
                api.unblock_microphone(channel_id, account_id).await
            }
        });
    }

    /// The presenter of meeting mode lets a person of the call of this device speak.
    pub fn allow_to_speak(&mut self, account_id: AccountId, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, channel_id| async move {
            api.unblock_microphone(channel_id, account_id).await
        });
    }

    /// The presenter of meeting mode lowers the raised hand of a person of the call of this
    /// device, and the block of the microphone of that person stays (ADR 0516).
    pub fn lower_hand_in_call(&mut self, account_id: AccountId, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, channel_id| async move {
            api.lower_participant_hand(channel_id, account_id).await
        });
    }

    /// Lowers the raised hand of a person in a voice channel: a moderator with "mute
    /// members", and the presenter of meeting mode in that call (ADR 0516).
    pub fn lower_hand_of(
        &mut self,
        room: (ServerId, ChannelId),
        account_id: AccountId,
        cx: &mut Context<Self>,
    ) {
        let (server_id, channel_id) = room;
        self.ask(server_id, cx, move |api| async move {
            api.lower_participant_hand(channel_id, account_id).await
        });
    }

    pub fn set_sound_block(
        &mut self,
        room: (ServerId, ChannelId),
        account_id: AccountId,
        block: bool,
        cx: &mut Context<Self>,
    ) {
        let (server_id, channel_id) = room;
        self.ask(server_id, cx, move |api| async move {
            if block {
                api.block_sound(channel_id, account_id).await
            } else {
                api.unblock_sound(channel_id, account_id).await
            }
        });
    }

    /// Removes a person from the call of a voice channel (ADR 0503).
    pub fn disconnect(
        &mut self,
        room: (ServerId, ChannelId),
        account_id: AccountId,
        cx: &mut Context<Self>,
    ) {
        let (server_id, channel_id) = room;
        self.ask(server_id, cx, move |api| async move {
            api.disconnect_participant(channel_id, account_id).await
        });
    }

    /// Moves a person from one voice channel into another (ADR 0511).
    pub fn move_person(
        &mut self,
        room: (ServerId, ChannelId),
        account_id: AccountId,
        to: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let (server_id, channel_id) = room;
        self.ask(server_id, cx, move |api| async move {
            api.move_participant(channel_id, account_id, to)
                .await
                .map(|_| ())
        });
    }

    /// Invites a member into the call of this device (ADR 0514). The invitation is
    /// remembered while it can wait: the state of the server holds an invitation only
    /// while it waits, and how it ended is told by an event that names the invitation and
    /// nothing else.
    pub fn invite(&mut self, account_id: AccountId, cx: &mut Context<Self>) {
        let Some((server, channel_id)) = self.call_channel() else {
            return;
        };
        let (server_id, api) = (server.id(), server.api());
        let asked = self.begin_ask();
        self.invitations_on_the_way += 1;
        cx.spawn(async move |this, cx| {
            let answer = api.invite_to_call(channel_id, account_id).await;
            let waits = this.update(cx, |this, cx| {
                this.invitations_on_the_way -= 1;
                let ended = answer.as_ref().ok().and_then(|invitation| {
                    this.early_invitation_ends
                        .remove(&(server_id, invitation.id))
                });
                if this.invitations_on_the_way == 0 {
                    this.early_invitation_ends.clear();
                }
                match (&answer, ended) {
                    (Err(error), _) => {
                        this.refused(&asked, error, cx);
                        false
                    }
                    (Ok(_), Some(outcome)) => {
                        this.invitation_ended(outcome, cx);
                        false
                    }
                    (Ok(invitation), None) => {
                        this.made_invitations.insert((server_id, invitation.id));
                        true
                    }
                }
            });
            let (Ok(invitation), Ok(true)) = (answer, waits) else {
                return;
            };
            let made = (server_id, invitation.id);
            let lasts = Duration::try_from(invitation.expires_at - invitation.created_at)
                .unwrap_or(INVITATION_LASTS)
                + ANSWER_MARGIN;
            cx.background_executor().timer(lasts).await;
            let _ = this.update(cx, |this, _| {
                this.made_invitations.remove(&made);
            });
        })
        .detach();
        cx.notify();
    }

    /// Tells the person how an invitation that the person made ended.
    fn invitation_ended(&mut self, outcome: InvitationOutcome, cx: &mut Context<Self>) {
        let told = match outcome {
            InvitationOutcome::Declined => text::INVITATION_DECLINED,
            InvitationOutcome::Expired => text::INVITATION_EXPIRED,
            InvitationOutcome::Accepted | InvitationOutcome::Cancelled => return,
        };
        self.pending.call_told = Some(told.to_owned());
        cx.notify();
    }

    pub fn withdraw_invitation(&mut self, invitation_id: CallInvitationId, cx: &mut Context<Self>) {
        self.ask_own_call(cx, move |api, _| async move {
            api.cancel_call_invitation(invitation_id).await
        });
    }

    /// Answers the request of a person outside to enter the call of this device.
    pub fn answer_entry(
        &mut self,
        request_id: EntryRequestId,
        accept: bool,
        cx: &mut Context<Self>,
    ) {
        self.ask_own_call(cx, move |api, _| async move {
            api.answer_entry_request(request_id, accept).await
        });
    }

    /// Accepts an invitation and enters its call. A refusal is told as a join that failed.
    pub fn accept_invitation(&mut self, invited: &view::Invited, cx: &mut Context<Self>) {
        let joining = self.calls.join_invited(invited.target, invited.id);
        cx.spawn(async move |_, _| {
            let _ = joining.await;
        })
        .detach();
    }

    pub fn decline_invitation(&mut self, invited: &view::Invited, cx: &mut Context<Self>) {
        let invitation_id = invited.id;
        self.direct.resolve(
            invited.target.server_id,
            invitation_id,
            InvitationOutcome::Declined,
        );
        self.reconcile_direct(cx);
        self.ask(invited.target.server_id, cx, move |api| async move {
            api.refuse_call_invitation(invitation_id).await
        });
    }

    /// Asks the people inside a locked or full voice channel to let the person in
    /// (ADR 0514). The answer arrives as an event; an acceptance enters the call. A
    /// person has one request at a time: an earlier one is withdrawn.
    pub fn ask_to_enter(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        self.withdraw_entry(cx);
        self.entry_asks += 1;
        self.early_entry_answers.clear();
        let ask = self.entry_asks;
        self.known.entry = Some(view::OwnEntry {
            server_id,
            channel_id,
            request_id: None,
            state: view::EntryState::Asking,
        });
        self.pending.call_refused = None;
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let asked = api.request_entry(channel_id).await;
            // The request that this answer belongs to is still the one of the person.
            let held = this.update(cx, |this, cx| {
                let current = this.known.entry.as_mut().filter(|_| this.entry_asks == ask);
                let held = match (current, &asked) {
                    (Some(entry), Ok(request)) => {
                        entry.request_id = Some(request.id);
                        entry.state = view::EntryState::Waiting;
                        // A person inside can answer before this device knows the
                        // identifier of its request.
                        let early = std::mem::take(&mut this.early_entry_answers)
                            .into_iter()
                            .find(|(answered, _)| *answered == request.id);
                        if let Some((_, outcome)) = early {
                            this.entry_answered(outcome, cx);
                        }
                        true
                    }
                    (Some(_), Err(error)) => {
                        this.known.entry = None;
                        this.pending.call_refused = Some(text::flow_error(&FlowError::from(error)));
                        false
                    }
                    (None, _) => false,
                };
                this.changed(cx);
                held
            });
            let Ok(request) = asked else {
                return;
            };
            if !held.unwrap_or(false) {
                // The person withdrew or asked anew before the server answered: this
                // request is not the one of the person, and is withdrawn.
                let _ = api.cancel_entry_request(request.id).await;
                return;
            }
            // A request ends on the server after a fixed time, and the event that tells it
            // can be missed while the event stream is interrupted. The time between the
            // two times of the server is used, so the clock of this machine does not count.
            let lasts = Duration::try_from(request.expires_at - request.created_at)
                .unwrap_or(ENTRY_REQUEST_LASTS)
                + ANSWER_MARGIN;
            cx.background_executor().timer(lasts).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this.known.entry.as_mut().filter(|entry| {
                    entry.request_id == Some(request.id) && entry.state == view::EntryState::Waiting
                }) {
                    entry.state = view::EntryState::Expired;
                    this.changed(cx);
                }
            });
        })
        .detach();
        self.changed(cx);
    }

    /// Uses the answer to the request of the person to enter a voice channel.
    fn entry_answered(&mut self, outcome: EntryOutcome, cx: &mut Context<Self>) {
        let Some(entry) = self.known.entry.as_mut() else {
            return;
        };
        let (server_id, channel_id) = (entry.server_id, entry.channel_id);
        match outcome {
            EntryOutcome::Accepted => {
                self.known.entry = None;
                let workspace_id = self
                    .client
                    .server(server_id)
                    .and_then(|server| server.live().view().model)
                    .and_then(|model| {
                        model
                            .channel(channel_id)
                            .map(|channel| channel.channel.workspace_id)
                    });
                if let Some(workspace_id) = workspace_id {
                    self.join_call(server_id, workspace_id, channel_id, cx);
                }
            }
            EntryOutcome::Denied => entry.state = view::EntryState::Denied,
            EntryOutcome::Expired => entry.state = view::EntryState::Expired,
            EntryOutcome::Cancelled => self.known.entry = None,
        }
        self.changed(cx);
    }

    /// Withdraws the request to enter that the window shows at a voice channel: the one
    /// of this device, or one that another device of the account made (ADR 0739).
    pub fn withdraw_entry_at(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let elsewhere = server.live().view().model.and_then(|model| {
            view::shown_entry(self.known.entry.as_ref(), &model, server_id, channel_id).elsewhere
        });
        let own =
            self.known.entry.as_ref().is_some_and(|entry| {
                entry.server_id == server_id && entry.channel_id == channel_id
            });
        // The request of this device, or what the window told of its end, goes first.
        if own {
            self.withdraw_entry(cx);
        }
        let Some(request_id) = elsewhere else {
            return;
        };
        let api = server.api();
        cx.spawn(async move |_, _| {
            // A request that was answered meanwhile is gone on the server.
            let _ = api.cancel_entry_request(request_id).await;
        })
        .detach();
    }

    /// Withdraws the request to enter, and puts away what was told about it.
    pub fn withdraw_entry(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.known.entry.take() else {
            return;
        };
        // An answer of the server to a request that is on its way finds no request of
        // the person, and withdraws what the server made.
        self.entry_asks += 1;
        self.early_entry_answers.clear();
        if let (Some(request_id), view::EntryState::Waiting, Some(server)) = (
            entry.request_id,
            entry.state,
            self.client.server(entry.server_id),
        ) {
            let api = server.api();
            cx.spawn(async move |_, _| {
                // A request that was answered meanwhile is gone on the server.
                let _ = api.cancel_entry_request(request_id).await;
            })
            .detach();
        }
        self.changed(cx);
    }

    /// The state of a server was replaced, or the reader of its changes fell behind and
    /// can have missed that: every message can have changed meanwhile.
    fn reloaded(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        *self.reloads.entry(server_id).or_default() += 1;
        self.forget_quotes(server_id, None, cx);
        self.search_after_reload(server_id, cx);
        self.changed(cx);
    }

    /// One change of the state of a server, told by itself. The state of a server holds
    /// a recording only while it runs; how it ended is told here (ADR 0719).
    #[cfg(feature = "test-support")]
    pub fn notice_fixture(&mut self, server_id: ServerId, notice: &Notice, cx: &mut Context<Self>) {
        self.noticed(server_id, notice, cx);
    }

    fn noticed(&mut self, server_id: ServerId, notice: &Notice, cx: &mut Context<Self>) {
        self.people_noticed(server_id, notice, cx);
        self.followed_noticed(server_id, notice, cx);
        self.logos_noticed(server_id, notice, cx);
        self.account_noticed(server_id, Some(notice), cx);
        self.recordings_noticed(server_id, Some(notice), cx);
        let event = match notice {
            Notice::Event { event, .. } => event,
            // The state was replaced: every message can have changed meanwhile, and the
            // state holds every change of the settings that the server took.
            Notice::Reloaded { .. } => {
                self.reloaded(server_id, cx);
                return;
            }
            Notice::Ended { .. } => return,
        };
        if let Event::MessageChanged { message_id, .. } = event.as_ref() {
            self.forget_quotes(server_id, Some(*message_id), cx);
        }
        if let Event::PermissionsChanged { workspace_id } = event.as_ref() {
            self.search_after_permission_change(server_id, cx);
            self.audit_after_permission_change(server_id, *workspace_id, cx);
        }
        let own_call = self
            .known
            .media
            .calls
            .call
            .as_ref()
            .filter(|call| call.target.server_id == server_id)
            .and_then(|_| self.client.server(server_id))
            .and_then(|server| server.live().view().model)
            .and_then(|model| model.own_call.as_ref().map(|own| own.call_id));
        match event.as_ref() {
            Event::CallMoveResolved { outcome, .. } => {
                use junto_client_core::protocol::call::MoveOutcome;
                let told = match outcome {
                    MoveOutcome::Completed => None,
                    MoveOutcome::TimedOut => {
                        Some("The move timed out. The person remains in the original call.")
                    }
                    MoveOutcome::Refused => Some(
                        "The device could not join the destination. The person remains in the original call.",
                    ),
                    MoveOutcome::Abandoned => {
                        Some("The move was cancelled because the call or its permissions changed.")
                    }
                };
                if let Some(told) = told {
                    self.pending.call_told = Some(told.to_owned());
                    cx.notify();
                }
            }
            Event::CallEnded { call_id, .. } => {
                self.direct.ended(server_id, *call_id);
                self.reconcile_direct(cx);
                cx.notify();
            }
            Event::CallInvited { invitation } => {
                if let Some(own) = self.client.server(server_id).and_then(|s| s.account()) {
                    self.direct
                        .see(server_id, own, invitation, std::time::Instant::now());
                }
                self.reconcile_direct(cx);
                cx.notify();
            }
            Event::RecordingStarted { .. } | Event::RecordingUpdated { .. } => {
                self.recording.event(server_id, own_call, event);
                self.show_recording();
                cx.notify();
            }
            // The answer to the request of the person to enter a voice channel. An
            // acceptance lets the person in once, within a minute: the device enters.
            Event::CallEntryAnswered {
                request_id,
                outcome,
            } => {
                match self
                    .known
                    .entry
                    .as_ref()
                    .filter(|entry| entry.server_id == server_id)
                    .map(|entry| entry.request_id)
                {
                    Some(Some(held)) if held == *request_id => self.entry_answered(*outcome, cx),
                    // The server has not told the identifier of the request of the person
                    // yet: the answer is kept until it has.
                    Some(None) => self.early_entry_answers.push((*request_id, *outcome)),
                    _ => {}
                }
            }
            // An invitation that the person made was declined, or nobody answered it. A
            // direct call whose ring ended so ends in the reconciliation of direct calls,
            // which also runs when the call itself arrives later.
            Event::CallInvitationResolved {
                invitation_id,
                outcome,
            } => {
                self.direct.resolve(server_id, *invitation_id, *outcome);
                self.reconcile_direct(cx);
                cx.notify();
                let ended = (server_id, *invitation_id);
                if self.made_invitations.remove(&ended) {
                    self.invitation_ended(*outcome, cx);
                } else if self.invitations_on_the_way > 0 {
                    // The invitation can be the one whose answer is on its way.
                    self.early_invitation_ends.insert(ended, *outcome);
                }
            }
            _ => {}
        }
    }

    /// Reads the sound devices and the microphone permission again, on a background
    /// thread: the list of a system can take a moment.
    pub fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        let read = self
            .permissions
            .order(crate::permissions::Permission::Microphone)
            .read();
        let calls = self.calls.clone();
        let reading = cx.background_executor().spawn(async move {
            (
                calls.devices(),
                calls.microphone_access(),
                calls.camera_devices(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (devices, access, cameras) = reading.await;
            let _ = this.update(cx, |this, cx| {
                match devices {
                    Ok(devices) => this.known.media.devices = devices,
                    Err(error) => tracing::warn!(%error, "the sound devices were not read"),
                }
                if this
                    .permissions
                    .order(crate::permissions::Permission::Microphone)
                    .accepts(read)
                {
                    this.known.media.access = Some(access);
                }
                this.camera_devices = cameras.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks the server of a voice channel for a place in its call. The result is in the
    /// view: the call, or the notice of a refusal.
    pub fn join_call(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let joining = self.calls.join(CallTarget {
            server_id,
            place: CallPlace::Channel {
                workspace_id,
                channel_id,
            },
        });
        cx.spawn(async move |_, _| {
            let _ = joining.await;
        })
        .detach();
    }

    pub fn leave_call(&mut self, cx: &mut Context<Self>) {
        let leaving = self.calls.leave();
        cx.spawn(async move |_, _| leaving.await).detach();
    }

    /// Does what the microphone control showed when it was drawn. The setting holds
    /// outside a call too, for the next one.
    pub fn use_microphone(&mut self, action: &MicrophoneAction) {
        match action {
            MicrophoneAction::Close => self.calls.mute(),
            MicrophoneAction::Open(seat) => self.calls.unmute_in(seat.as_ref()),
        }
    }

    /// Turns the sound off or on, as the control showed when it was drawn.
    pub fn set_sound_off(&mut self, off: bool) {
        self.calls.set_deafened(off);
    }

    /// The push-to-talk key is held or released.
    pub fn set_talking(&mut self, talking: bool) {
        self.calls.set_talking(talking);
    }

    pub fn retry_microphone(&mut self, seat: Option<&Seat>, cx: &mut Context<Self>) {
        if let Some(seat) = seat {
            self.calls.retry_microphone_in(seat);
        }
        self.refresh_devices(cx);
    }

    pub fn dismiss_call_notice(&mut self) {
        self.calls.dismiss_outcome();
    }

    /// Shows the voice channel of the call of this device.
    pub fn show_call(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.known.media.calls.call.as_ref() else {
            return;
        };
        let target = call.target;
        if let CallPlace::Direct {
            low_account_id,
            high_account_id,
        } = target.place
        {
            if let Some(own) = self
                .client
                .server(target.server_id)
                .and_then(|s| s.account())
            {
                self.open_conversation(
                    target.server_id,
                    if own == low_account_id {
                        high_account_id
                    } else {
                        low_account_id
                    },
                    cx,
                );
            }
            return;
        }
        self.end_jump();
        self.navigations += 1;
        self.selection.server = Some(target.server_id);
        if let CallPlace::Channel {
            workspace_id,
            channel_id,
        } = target.place
        {
            self.selection
                .workspaces
                .insert(target.server_id, workspace_id);
            self.selection
                .channels
                .insert((target.server_id, workspace_id), channel_id);
        }
        self.changed(cx);
    }

    /// Uses a volume for a person at once, while the person of this device moves the
    /// control. [`AppState::store_volume`] writes it.
    pub fn set_volume(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        percent: u16,
        cx: &mut Context<Self>,
    ) {
        self.known
            .media
            .voice
            .set_volume(server_id, account_id, percent);
        let percent = self.known.media.voice.volume(server_id, account_id);
        self.calls
            .set_volume(server_id, account_id, f32::from(percent) / 100.0);
        cx.notify();
    }

    /// Writes the volume of a person as it is now.
    pub fn store_volume(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        cx: &mut Context<Self>,
    ) {
        let percent = self.known.media.voice.volume(server_id, account_id);
        self.write_voice(cx, move |voice| {
            voice.set_volume(server_id, account_id, percent);
        });
    }

    /// Chooses the input device. `None` is the default of the system.
    pub fn choose_input(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| voice.input_device.clone_from(&device));
    }

    pub fn choose_output(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| voice.output_device.clone_from(&device));
    }

    pub fn set_processing(&mut self, part: ProcessingPart, on: bool, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| match part {
            ProcessingPart::EchoCancellation => voice.echo_cancellation = on,
            ProcessingPart::NoiseSuppression => voice.noise_suppression = on,
            ProcessingPart::AutomaticGain => voice.automatic_gain = on,
        });
    }

    pub fn set_input_mode(&mut self, mode: InputMode, cx: &mut Context<Self>) {
        if mode != InputMode::PushToTalk {
            self.calls.set_talking(false);
        }
        self.change_voice(cx, move |voice| voice.input_mode = mode);
    }

    /// Opens the microphone outside a call while the sound settings are shown, for the level
    /// under the sensitivity, and closes it (ADR 0733). A call opens the microphone itself.
    pub fn set_input_test(&mut self, on: bool) {
        if on && self.calls.view().call.is_none() {
            self.calls.start_input_test();
        } else {
            self.calls.stop_input_test();
        }
    }

    /// The sensitivity of voice activation: the engine uses it at once, and the file is
    /// written when `store` is set, at the release of the slider.
    pub fn set_voice_threshold(
        &mut self,
        threshold: Option<i16>,
        store: bool,
        cx: &mut Context<Self>,
    ) {
        let change = move |voice: &mut VoiceSettings| voice.voice_threshold = threshold;
        if store {
            self.change_voice(cx, change);
        } else {
            change(&mut self.known.media.voice);
            self.calls.set_voice(voice_config(&self.known.media.voice));
            cx.notify();
        }
    }

    pub fn set_talk_key(&mut self, key: TalkKey, cx: &mut Context<Self>) {
        self.calls.set_talking(false);
        self.change_voice(cx, move |voice| voice.talk_key = key);
    }

    /// Changes a sound setting: the engine uses it at once, and the file is written on a
    /// background thread.
    fn change_voice(
        &mut self,
        cx: &mut Context<Self>,
        change: impl Fn(&mut VoiceSettings) + Send + 'static,
    ) {
        change(&mut self.known.media.voice);
        self.calls.set_voice(voice_config(&self.known.media.voice));
        // The level test opens the device again with the changed device or processing.
        if self.calls.input_test_runs() {
            self.calls.start_input_test();
        }
        self.write_voice(cx, change);
    }

    fn write_voice(
        &mut self,
        cx: &mut Context<Self>,
        change: impl Fn(&mut VoiceSettings) + Send + 'static,
    ) {
        let store = self.client.voice();
        Self::queue_write(
            cx,
            move || store.update(|stored| change(stored)),
            |this, written, cx| {
                this.voice_failed = written
                    .err()
                    .map(|error| text::flow_error(&FlowError::Storage(error.to_string())));
                cx.notify();
            },
        );
        cx.notify();
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The settings as the window uses them: the stored ones, with the interface scale of
    /// this start when a start argument gave one.
    pub fn settings(&self) -> InterfaceSettings {
        InterfaceSettings {
            scale_percent: self
                .scale_of_this_start
                .unwrap_or(self.settings.scale_percent),
            ..self.settings
        }
    }

    /// Uses an interface scale for this start only. The stored setting stays.
    pub fn use_scale_for_this_start(&mut self, scale_percent: u32, cx: &mut Context<Self>) {
        self.scale_of_this_start = Some(theme::clamp_scale(scale_percent));
        cx.notify();
    }

    /// Why a setting that the window shows is not in the file.
    pub fn notifications_unavailable(&self) -> Option<&'static str> {
        self.notifier.unavailable_reason()
    }

    pub fn settings_error(&self) -> Option<&str> {
        [Setting::Scale, Setting::ServerBook, Setting::Theme]
            .iter()
            .find_map(|setting| self.settings_failed.get(setting))
            .map(String::as_str)
    }

    /// Runs `work` on a background thread after every write that was asked for before,
    /// and then gives its result to `done`. The work is done also when this state has
    /// ended by then; `done` runs only while the state lives.
    fn queue_write<R: Send + 'static>(
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let before = cx.default_global::<Writes>().last.take();
        let (result, written) = oneshot::channel();
        let write = cx.background_executor().spawn(async move {
            if let Some(before) = before {
                before.await;
            }
            let _ = result.send(work());
        });
        cx.default_global::<Writes>().last = Some(write);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = written.await {
                let _ = this.update(cx, |this, cx| done(this, result, cx));
            }
        })
        .detach();
    }

    pub fn pending(&self) -> &Pending {
        &self.pending
    }

    /// True while the waiting request of the saved entry of this server is being given up.
    pub fn cancels_request(&self, server_id: ServerId) -> bool {
        self.client
            .server(server_id)
            .is_some_and(|server| self.pending.cancels(server.entry().key()))
    }

    /// The name of the profile preset. Empty until the person has chosen one.
    pub fn preset_name(&self) -> &str {
        &self.known.preset_name
    }

    pub fn server(&self, server_id: ServerId) -> Option<ServerHandle> {
        self.client.server(server_id)
    }

    /// What the window shows now.
    pub fn view(&self) -> ShellView {
        #[cfg(feature = "test-support")]
        if let Some(media) = &self.media_fixture {
            let known = Known {
                media: media.clone(),
                ..self.known.clone()
            };
            return self.direct.filter_view(view::derive(
                &self.snapshots(),
                &self.selection,
                &known,
            ));
        }
        self.direct.filter_view(view::derive(
            &self.snapshots(),
            &self.selection,
            &self.known,
        ))
    }

    /// Draws this state of the calls in place of the calls of the client. The screen tool
    /// uses it to draw a call without a media server.
    #[cfg(feature = "test-support")]
    pub fn show_media_fixture(&mut self, media: MediaSnapshot, cx: &mut Context<Self>) {
        self.known.share_notice_dismissed &= share_ended(&media.calls);
        self.media_fixture = Some(media);
        self.changed(cx);
    }

    /// Draws this state of the recording of the call in place of what the servers told.
    /// The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_recording_fixture(
        &mut self,
        problem: Option<(ServerId, junto_client_core::protocol::recording::Recording)>,
        request_failed: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.recording.show(problem, request_failed);
        self.show_recording();
        self.changed(cx);
    }

    /// Draws this request to enter and these statements about the call in place of what
    /// the servers told. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_call_rules_fixture(
        &mut self,
        entry: Option<view::OwnEntry>,
        told: Option<String>,
        refused: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.known.entry = entry;
        self.pending.call_told = told;
        self.pending.call_refused = refused;
        self.changed(cx);
    }

    fn snapshots(&self) -> Vec<ServerSnapshot> {
        #[cfg(feature = "test-support")]
        if let Some(fixture) = &self.fixture {
            return fixture.clone();
        }
        self.client
            .servers()
            .iter()
            .map(ServerSnapshot::of)
            .collect()
    }

    /// Draws these servers in place of the servers of the client core. The screen tool
    /// uses it to draw a state that no server of a test gives, such as a ban.
    #[cfg(feature = "test-support")]
    pub fn show_fixture(&mut self, servers: Vec<ServerSnapshot>, cx: &mut Context<Self>) {
        self.fixture = Some(servers);
        self.changed(cx);
    }

    /// Draws the servers of the client as they are now, with these invitations added to the
    /// state of one server, in place of the servers of the client core. A window test uses
    /// it for invitations that a server without a media server cannot make.
    #[cfg(feature = "test-support")]
    pub fn show_invitations_fixture(
        &mut self,
        server_id: ServerId,
        invitations: Vec<junto_client_core::protocol::call::CallInvitation>,
        cx: &mut Context<Self>,
    ) {
        let mut servers: Vec<ServerSnapshot> = self
            .client
            .servers()
            .iter()
            .map(ServerSnapshot::of)
            .collect();
        for server in &mut servers {
            if server.id == server_id
                && let Some(model) = &mut server.model
            {
                std::sync::Arc::make_mut(model)
                    .invitations
                    .extend(invitations.iter().cloned());
            }
        }
        self.fixture = Some(servers);
        self.changed(cx);
    }

    /// Runs after every change of the client core and of an event stream: starts and ends
    /// the work that depends on what exists and on what is shown, and draws again.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.reconcile_direct(cx);
        self.play_call_sounds();
        let servers = self.client.servers();
        let ids: HashSet<ServerId> = servers.iter().map(ServerHandle::id).collect();
        self.watchers.retain(|id, _| ids.contains(id));
        self.waiting.retain(|id, _| ids.contains(id));
        self.notice_readers.retain(|id, _| ids.contains(id));
        self.made_invitations.retain(|(id, _)| ids.contains(id));
        self.early_invitation_ends
            .retain(|(id, _), _| ids.contains(id));
        self.pending
            .acknowledge_failed
            .retain(|owner, _| ids.contains(&owner.entry.server_id));
        self.pending.remove_failed.retain(|id, _| ids.contains(id));
        for server in &servers {
            let id = server.id();
            self.watchers.entry(id).or_insert_with(|| {
                let mut watch = server.live().watch();
                cx.spawn(async move |this, cx| {
                    while watch.changed().await.is_some() {
                        if this.update(cx, |this, cx| this.changed(cx)).is_err() {
                            break;
                        }
                    }
                })
            });
            self.notice_readers.entry(id).or_insert_with(|| {
                let mut notices = server.live().notices();
                cx.spawn(async move |this, cx| {
                    while let Some(notice) = notices.next().await {
                        // A reader that fell behind missed single changes, and can have
                        // missed that the state was replaced.
                        let told = this.update(cx, |this, cx| match &notice {
                            Ok(notice) => this.noticed(id, notice, cx),
                            Err(_) => {
                                this.account_noticed(id, None, cx);
                                this.recordings_noticed(id, None, cx);
                                this.people_missed(id, cx);
                                this.followed_missed(id, cx);
                                this.logos_missed(id, cx);
                                this.reloaded(id, cx);
                            }
                        });
                        if told.is_err() {
                            break;
                        }
                    }
                })
            });
            let waits = matches!(
                server.access().state,
                AccessState::WaitingForApproval { .. } | AccessState::WaitingForLink { .. }
            );
            if !waits {
                self.waiting.remove(&id);
            } else if !self.waiting.contains_key(&id) {
                let decision = server.wait_for_decision(self.timing.decision_interval);
                let task = cx.spawn(async move |this, cx| {
                    // A failure that ends the wait stays in the state of the flow. The
                    // next change of the server starts a new wait while it still waits.
                    let _ = decision.await;
                    let _ = this.update(cx, |this, cx| {
                        this.waiting.remove(&id);
                        cx.notify();
                    });
                });
                self.waiting.insert(id, task);
            }
        }

        #[cfg(feature = "test-support")]
        let now = self
            .clock_fixture
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        #[cfg(not(feature = "test-support"))]
        let now = time::OffsetDateTime::now_utc();
        self.known.clock = Some(now);
        self.known.today = Some(self.known.zone.local(now).date());
        self.follow_owners();
        let snapshots = self.snapshots();
        self.settle_sent_settings(&snapshots);
        self.keep_people(cx);
        self.keep_logos(cx);
        self.follow_updates();
        self.follow_arrangement();
        self.follow_call_keys(&snapshots);
        let view = self.view();
        let shown = view.shown();
        if shown.server != self.shown.server {
            self.client.select(shown.server);
        }
        let wanted = shown.server.zip(shown.workspace);
        if self.members.as_ref().map(|(key, _)| *key) != wanted {
            if let Some(((server_id, workspace_id), _)) = self.members.take()
                && let Some(server) = self.client.server(server_id)
            {
                server.live().release_members(workspace_id);
            }
            if let Some(key) = wanted {
                self.load_members(key, cx);
            }
        }
        // The side panel belongs to the channel that is shown: a thread or a pin list of
        // a channel that left the main area is closed, and does not open again by itself.
        if self.selection.thread.is_some() && shown.thread.is_none() {
            self.selection.thread = None;
        }
        if self.selection.pins.is_some() && shown.pins.is_none() {
            self.selection.pins = None;
        }
        // The transcript of the side panel belongs to the shown voice channel. It stays
        // under a thread and under the pinned messages.
        if self
            .selection
            .transcript
            .is_some_and(|(server_id, channel_id, _)| {
                shown.server != Some(server_id) || shown.channel != Some(channel_id)
            })
        {
            self.selection.transcript = None;
            self.known.transcript_line = None;
        }
        // A refusal of a direct conversation belongs to the place where it is stated
        // (ADR 0741).
        if self
            .known
            .conversation_refused
            .as_ref()
            .is_some_and(|refused| {
                shown.server != Some(refused.server_id) || shown.place() != refused.place
            })
        {
            self.known.conversation_refused = None;
        }
        // The notices that were open beside a place close when the main area changes to
        // the view of the call of this device because another place is shown or the call
        // began, so that its panel shows the chat; in the view of a call the row of the
        // notices opens them again. A call that returns from the call window to the place
        // that the main area shows keeps them (ADR 0742).
        let call_shown = view.lists().is_some_and(|lists| lists.call.is_some());
        let shown_with_call = (
            shown.server,
            shown.place(),
            self.media_calls().call.as_ref().map(|call| call.target),
        );
        if call_shown && !self.call_shown && shown_with_call != self.shown_with_call {
            self.selection.notices = None;
        }
        self.call_shown = call_shown;
        self.shown_with_call = shown_with_call;
        // The notices belong to a server: they close when another server is shown. They
        // stay under a thread and under the pinned messages.
        if self.selection.notices.is_some() && self.selection.notices != shown.server {
            self.selection.notices = None;
        }
        if self.selection.notify.is_some() && view.notify.is_none() {
            self.selection.notify = None;
        }
        self.end_search_of_other_server(shown.server);
        self.drop_lost_search_filter(&snapshots, cx);
        let before = self.shown;
        self.follow_history(&before, &shown);
        self.shown = shown;
        self.follow_people(&view, &snapshots, cx);
        self.follow_followed(&view, cx);
        self.follow_moderation(&snapshots, cx);
        let readers = self.archived_readers(&view, &snapshots);
        self.follow_admin(readers, cx);
        self.follow_account(cx);
        let call_window = self.call_window_place(&snapshots);
        self.follow_places(shown, call_window, cx);
        self.follow_recordings(shown, cx);
        self.follow_jump(shown);
        self.follow_result_jump(shown);
        self.tell_alerts(&snapshots, shown, call_window, cx);
        let mentions = tray::tray_mention_count(&view);
        self.set_tray_mentions(mentions, cx);
        self.follow_tray();
        self.ask_missing_quotes(shown, cx);
        self.end_uploads_of_other_accounts();
        self.drop_sent_uploads();
        cx.notify();
    }

    fn load_members(&mut self, key: (ServerId, WorkspaceId), cx: &mut Context<Self>) {
        let Some(server) = self.client.server(key.0) else {
            return;
        };
        self.known.members_failed.retain(|failed| *failed != key);
        let loading = server.live().load_members(key.1);
        let task = cx.spawn(async move |this, cx| {
            let result = loading.await;
            let _ = this.update(cx, |this, cx| {
                if result.is_err() && !this.known.members_failed.contains(&key) {
                    this.known.members_failed.push(key);
                }
                cx.notify();
            });
        });
        self.members = Some((key, task));
    }

    /// Reads the members of the shown workspace again after a read that failed.
    pub fn reload_members(&mut self, cx: &mut Context<Self>) {
        if let Some((key, _)) = self.members.take() {
            self.load_members(key, cx);
            cx.notify();
        }
    }

    /// Contacts again every server that this device is a member of and that has no
    /// session: the server could not be reached, or its login failed.
    fn retry_unreached(&mut self, cx: &mut Context<Self>) {
        for server in self.client.servers() {
            let view = server.access();
            if matches!(view.state, AccessState::LoggedOut { .. }) && view.in_progress.is_none() {
                Self::run(server.connect(), cx);
            }
        }
    }

    /// Runs an action of a flow. Its result is in the state of the flow.
    fn run(action: impl Future<Output = Result<(), FlowError>> + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |_, _| {
            let _ = action.await;
        })
        .detach();
    }

    pub fn select_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.end_jump();
        self.navigations += 1;
        self.selection.server = Some(server_id);
        self.changed(cx);
    }

    pub fn select_workspace(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        self.end_jump();
        self.navigations += 1;
        self.selection.server = Some(server_id);
        self.selection.workspaces.insert(server_id, workspace_id);
        self.changed(cx);
    }

    pub fn select_channel(
        &mut self,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let Some(server_id) = self.shown.server else {
            return;
        };
        self.end_jump();
        self.navigations += 1;
        self.selection
            .channels
            .insert((server_id, workspace_id), channel_id);
        // The channel takes the place of a direct conversation that the server showed.
        self.selection.conversations.remove(&server_id);
        self.changed(cx);
    }

    /// Asks the server what this device is now, or logs in.
    pub fn connect(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.connect(), cx);
        }
    }

    /// Logs in after a logout of the person: from then on the application logs in by
    /// itself again (ADR 0712).
    pub fn log_in(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.dispatch_when_free(AccessAction::LogIn), cx);
        }
    }

    /// Logs out of the account of the server on this device. The account and the key stay,
    /// and the application does not log in again until the person asks (ADR 0712).
    pub fn log_out(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.dispatch_when_free(AccessAction::LogOutByPerson), cx);
        }
    }

    /// Deletes the key of this device for the server. The server stays in the book.
    pub fn forget_account(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.dispatch_when_free(AccessAction::ForgetAccount), cx);
        }
    }

    /// Removes the server from the book with the key of this device. The files are changed
    /// on a background thread; a failure is told in [`Pending::remove_failed`]. Returns
    /// false, and starts nothing, while a removal of this server runs.
    pub fn remove_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) -> bool {
        let Some(server) = self.client.server(server_id) else {
            return false;
        };
        let entry = server.entry().key();
        if !self.pending.begin_removal(entry) {
            return false;
        }
        let removing = self.client.remove_server_when_free(server_id);
        cx.spawn(async move |this, cx| {
            let removed = cx.background_executor().spawn(removing).await;
            let _ = this.update(cx, |this, cx| this.removed_entry(entry, removed, cx));
        })
        .detach();
        cx.notify();
        true
    }

    fn removed_entry(
        &mut self,
        entry: EntryKey,
        result: Result<(), junto_client_core::manager::RemoveError>,
        cx: &mut Context<Self>,
    ) {
        if !self.pending.end_removal(entry) {
            return;
        }
        if self
            .client
            .server(entry.server_id)
            .is_some_and(|server| server.entry().key() != entry)
        {
            self.changed(cx);
            return;
        }
        self.removed(entry.server_id, result, cx);
    }

    fn removed(
        &mut self,
        server_id: ServerId,
        result: Result<(), junto_client_core::manager::RemoveError>,
        cx: &mut Context<Self>,
    ) {
        self.pending.removing.remove(&server_id);
        match result {
            // The key has an account: the server stays, and its state shows the account.
            Ok(()) | Err(junto_client_core::manager::RemoveError::HasAccount) => {
                self.pending.remove_failed.remove(&server_id);
            }
            Err(error) => {
                let failure = match error {
                    junto_client_core::manager::RemoveError::Busy => FlowError::Busy,
                    other => FlowError::Storage(other.to_string()),
                };
                self.pending
                    .remove_failed
                    .insert(server_id, text::flow_error(&failure));
            }
        }
        self.changed(cx);
    }

    /// Gives up a request that waits: forgets it, then removes the server from the book.
    /// When the request was approved meanwhile, the device has its account and nothing is
    /// removed. Every other failure is told in [`Pending::remove_failed`].
    pub fn cancel_request(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let entry = server.entry().key();
        if self.pending.removing.contains(&server_id) || !self.pending.begin_cancellation(entry) {
            return;
        }
        self.pending.begin_removal(entry);
        self.pending.remove_failed.remove(&server_id);
        self.waiting.remove(&server_id);
        let client = self.client.clone();
        let forgetting = server.dispatch_when_free(AccessAction::ForgetAccount);
        cx.spawn(async move |this, cx| {
            let forgotten = forgetting.await;
            let removed = match &forgotten {
                Ok(()) => Some(
                    cx.background_executor()
                        .spawn(async move { client.remove_server(server_id) })
                        .await,
                ),
                Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.pending.end_cancellation(entry);
                if !this.pending.end_removal(entry) {
                    return;
                }
                if this
                    .client
                    .server(server_id)
                    .is_some_and(|server| server.entry().key() != entry)
                {
                    this.changed(cx);
                    return;
                }
                match (forgotten, removed) {
                    (_, Some(removed)) => this.removed(server_id, removed, cx),
                    // The state no longer accepts the action: the request was decided
                    // while the person cancelled it. The state of the server shows how.
                    (Err(FlowError::NotAvailable), None) => this.changed(cx),
                    (Err(error), None) => {
                        this.pending
                            .remove_failed
                            .insert(server_id, text::flow_error(&error));
                        this.changed(cx);
                    }
                    (Ok(()), None) => this.changed(cx),
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn set_keep_connected(&mut self, server_id: ServerId, keep: bool, cx: &mut Context<Self>) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let mode = if keep {
            ConnectionMode::Always
        } else {
            ConnectionMode::WhenSelected
        };
        // The writes of one server run in the order of the clicks. The client core tells
        // its change when the book is written.
        Self::queue_write(
            cx,
            move || server.set_connection_mode(mode),
            |_, written, _| {
                if let Err(error) = written {
                    tracing::warn!(%error, "the connection mode was not saved");
                }
            },
        );
    }

    /// States that the person has seen a recovery (ADR 0118).
    pub fn acknowledge_recovery(
        &mut self,
        server_id: ServerId,
        recovery_id: RecoveryId,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(owner) = server.owner() else {
            return;
        };
        if !self.pending.acknowledging.insert((owner, recovery_id)) {
            return;
        }
        self.pending.acknowledge_failed.remove(&owner);
        let sending = server.acknowledge_recovery(recovery_id);
        cx.spawn(async move |this, cx| {
            let result = sending.await;
            let _ = this.update(cx, |this, cx| {
                this.pending.acknowledging.remove(&(owner, recovery_id));
                // The server holds no notice of this device any more: the recovery left
                // the state, and there is nothing to tell.
                let gone = matches!(
                    &result,
                    Err(ApiError::Rejected(rejection))
                        if rejection.code
                            == junto_client_core::protocol::error::ErrorCode::NotFound
                );
                if let (Err(error), false) = (&result, gone) {
                    this.pending
                        .acknowledge_failed
                        .insert(owner, text::flow_error(&FlowError::from(error)));
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Sets the name of the profile preset. The name is checked at once; the file is
    /// written on a background thread. The window takes the result with
    /// [`AppState::take_name_result`] when this state tells a change.
    pub fn set_preset_name(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.pending.saving_name {
            return;
        }
        let name = match junto_client_core::profile::account_name(name) {
            Ok(name) => name,
            Err(invalid) => {
                let failure = text::flow_error(&FlowError::from(invalid));
                self.name_result = Some((name.to_owned(), Err(failure)));
                cx.notify();
                return;
            }
        };
        self.pending.saving_name = true;
        let profile = self.client.profile().clone();
        let submitted = name.clone();
        Self::queue_write(
            cx,
            move || profile.set_name(&name),
            move |this, saved, cx| {
                this.pending.saving_name = false;
                let saved = match saved {
                    Ok(preset) => {
                        this.known.preset_name = preset.name;
                        Ok(())
                    }
                    Err(junto_client_core::profile::ProfileError::Name(invalid)) => {
                        Err(text::flow_error(&FlowError::from(invalid)))
                    }
                    Err(other) => Err(text::flow_error(&FlowError::Storage(other.to_string()))),
                };
                this.name_result = Some((submitted, saved));
                cx.notify();
            },
        );
        cx.notify();
    }

    /// The name that was submitted last, and whether it was stored. Each result is given
    /// once.
    pub fn take_name_result(&mut self) -> Option<(String, Result<(), String>)> {
        self.name_result.take()
    }

    /// Sets the interface scale and stores it.
    pub fn set_scale(&mut self, scale_percent: u32, cx: &mut Context<Self>) {
        self.scale_of_this_start = None;
        self.settings.scale_percent = theme::clamp_scale(scale_percent);
        self.write_setting(Setting::Scale, cx);
    }

    /// Sets the theme, draws the windows in it and stores it.
    pub fn set_theme(&mut self, choice: ThemeChoice, cx: &mut Context<Self>) {
        self.settings.theme = choice;
        theme::apply(choice, cx);
        self.write_setting(Setting::Theme, cx);
    }

    pub fn set_server_book_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.settings.server_book_collapsed = collapsed;
        self.write_setting(Setting::ServerBook, cx);
    }

    /// Writes one changed setting on a background thread, in the order of the changes. The
    /// window uses the new value at once. Only the changed setting is written, so that a
    /// second program that changed another one keeps its value. A setting whose write
    /// failed before is written again with this one, and its failure is told in
    /// [`AppState::settings_error`] until it is in the file.
    fn write_setting(&mut self, changed: Setting, cx: &mut Context<Self>) {
        let mut writing: Vec<Setting> = self.settings_failed.keys().copied().collect();
        if !writing.contains(&changed) {
            writing.push(changed);
        }
        let (store, wanted, written) = (self.client.settings(), self.settings, writing.clone());
        Self::queue_write(
            cx,
            move || {
                store.update(|stored| {
                    for setting in &writing {
                        match setting {
                            Setting::Scale => stored.scale_percent = wanted.scale_percent,
                            Setting::ServerBook => {
                                stored.server_book_collapsed = wanted.server_book_collapsed;
                            }
                            Setting::Theme => stored.theme = wanted.theme,
                        }
                    }
                })
            },
            move |this, result, cx| {
                match result {
                    Ok(_) => {
                        for setting in &written {
                            this.settings_failed.remove(setting);
                        }
                    }
                    Err(error) => {
                        let failure = text::flow_error(&FlowError::Storage(error.to_string()));
                        for setting in written {
                            this.settings_failed.insert(setting, failure.clone());
                        }
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }
}

/// What the camera control does when the person uses it, as it was drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraAction {
    Stop,
    /// Starts the camera at this seat.
    Start(Seat),
}

#[derive(Clone)]
pub(crate) struct CallAction {
    target: CallTarget,
    identity: String,
}

impl CallAction {
    fn of(call: &junto_media::calls::ActiveCall) -> Self {
        Self {
            target: call.target,
            identity: call.identity.clone(),
        }
    }

    pub(crate) fn permits(&self, current: &junto_media::calls::CallsView) -> bool {
        current.call.as_ref().is_some_and(|call| {
            call.phase == junto_media::calls::Phase::Connected
                && call.target == self.target
                && call.identity == self.identity
        })
    }
}

fn actionable_call(
    shown: &junto_media::calls::CallsView,
    current: &junto_media::calls::CallsView,
) -> Option<CallTarget> {
    let shown = shown.call.as_ref()?;
    current
        .call
        .as_ref()
        .filter(|call| {
            call.phase == junto_media::calls::Phase::Connected
                && call.target == shown.target
                && call.identity == shown.identity
        })
        .map(|call| call.target)
}

pub mod direct;

mod camera;

mod call_window;
