//! The state of the application: the client core, what the person selected, and the work
//! that keeps the window current (ADR 0730).
//!
//! One entity holds all of it. It wakes on every change of the client core and of the
//! event stream of each server, and tells the window to draw again. The window reads plain
//! data through [`AppState::view`] and calls the methods here for what a person does.

pub mod feed;
mod messages;
pub mod notices;
mod notifying;
pub mod recorded;
mod recorded_work;
pub mod recording;
pub mod search;
mod searching;
mod threads;
pub mod view;

pub use messages::AttachRefusal;
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
use junto_client_core::settings::InterfaceSettings;
use junto_client_core::transfers::{Download, RecordingDownload, Upload};
use junto_client_core::voice::{InputMode, ShareResolution, TalkKey, VoiceSettings};
use junto_client_core::{Client, ServerHandle};
use junto_media::audio::{DeviceId, Processing, level};
use junto_media::calls::{CallTarget, Calls, MediaOptions};
use junto_media::engine::{Levels, MicrophoneState, ShareId, VoiceConfig};
use junto_media::permission::MicrophoneAccess;
use junto_media::share::{CaptureRequest, PictureSource, Resolution, ShareQuality, SoundChoice};

use crate::text;
use crate::theme;
use view::{
    INPUT_SEGMENTS, Known, LEVEL_SEGMENTS, MediaSnapshot, Selection, ServerSnapshot, ShellView,
    Shown, ShownLevels,
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

/// How often the name of one account is asked for when no answer arrives.
const NAME_TRIES: u8 = 3;

/// Where the read of the name of one account stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameRead {
    /// A request is on its way. The number counts the requests so far.
    Asking(u8),
    /// No request is on its way, and this number of requests got no answer.
    Unanswered(u8),
    /// The name was read, or the server refused: it is not asked for again.
    Settled,
}

/// What the person asked for and that has not ended yet, or has failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// The recoveries whose acknowledgement is on its way.
    pub acknowledging: HashSet<RecoveryId>,
    /// Why the last acknowledgement of a recovery failed, per server.
    pub acknowledge_failed: HashMap<ServerId, String>,
    /// Why a server could not be removed.
    pub remove_failed: HashMap<ServerId, String>,
    /// The servers whose removal is on its way. A second removal of one of them is not
    /// started: it would be decided against the server book of a later moment.
    pub removing: HashSet<ServerId>,
    /// The servers whose waiting request is being given up.
    pub cancelling: HashSet<ServerId>,
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
    pub message_refused: Option<String>,
}

pub struct AppState {
    client: Client,
    calls: Calls,
    /// Why the sound settings are not in their file.
    voice_failed: Option<String>,
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
    pending: Pending,
    /// What the window showed after the last change. Side effects follow a change of it.
    shown: Shown,
    /// The accounts whose name was asked for from their profile route.
    asked_names: HashMap<(ServerId, AccountId), NameRead>,
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
    /// Where files are saved in place of the download directory of the person.
    download_directory: Option<std::path::PathBuf>,
    /// The number of requests about messages that the person made.
    message_asks: u64,
    /// The position of the list of each open place, as the client core was told it.
    anchors: HashMap<(ServerId, PlaceId), Option<u64>>,
    /// The read position of each place that the client core was asked to set.
    marked: HashMap<(ServerId, PlaceId), u64>,
    /// The recordings that the window shows, with the task that opens each in the
    /// client core.
    open_recordings: HashMap<(ServerId, RecordingId), Task<()>>,
    /// The downloads of the files of recordings.
    recording_downloads: HashMap<(ServerId, RecordingId, RecordedFile), RecordingDownload>,
    /// The number of the download of each file that the window follows, and the number
    /// of the last download.
    recording_download_numbers: HashMap<(ServerId, RecordingId, RecordedFile), u64>,
    download_asks: u64,
    /// The downloads whose file is opened when it is saved.
    opening: HashSet<(ServerId, RecordingId, RecordedFile)>,
    /// A saved file is opened with the application of the system for it.
    opens_files: bool,
    /// The files that a start without the applications of the system would have opened.
    kept_opened: Vec<std::path::PathBuf>,
    /// The number of the last request of a search. An answer to an earlier request is
    /// not used.
    search_asks: u64,
    /// The continuation of the search of the window, for its next page.
    search_next: Option<String>,
    /// The jump to a message that waits until the client core holds the list of its
    /// place.
    jump: Option<notices::Jump>,
    /// The notifications of the system.
    notifier: std::rc::Rc<dyn crate::notify::SystemNotifier>,
    /// The notifier of a start without the notifications of the system.
    kept: crate::notify::KeptNotifier,
    /// The window is the active window of the system.
    window_active: bool,
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
    /// The number of the last read or change of the level of a workspace.
    default_asks: HashMap<(ServerId, WorkspaceId), u64>,
    /// The servers that the view is derived from in place of the client core.
    #[cfg(feature = "test-support")]
    fixture: Option<Vec<ServerSnapshot>>,
    /// The time that the view is derived for in place of the time of the system.
    #[cfg(feature = "test-support")]
    clock_fixture: Option<time::OffsetDateTime>,
    _changes: Task<()>,
    _retry: Task<()>,
    _media: [Task<()>; 3],
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
        let voice = match client.voice().load() {
            Ok(voice) => voice,
            Err(error) => {
                tracing::warn!(%error, "the sound settings were not read");
                VoiceSettings::default()
            }
        };
        let calls = Calls::new(client.clone(), media);
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
                if this.update(cx, |this, cx| this.calls_changed(cx)).is_err() {
                    break;
                }
            }
        });
        let mut levels = calls.levels();
        let follow_levels = cx.spawn(async move |this, cx| {
            while levels.changed().await.is_ok() {
                let shown = shown_levels(&levels.borrow_and_update());
                let told = this.update(cx, |this, cx| {
                    if this.known.media.levels != shown {
                        this.known.media.levels = shown;
                        cx.notify();
                    }
                });
                if told.is_err() {
                    break;
                }
            }
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let told = this.update(cx, |this, cx| {
                    if this.known.media.calls.call.is_some() {
                        this.known.media.now = Some(time::OffsetDateTime::now_utc());
                        cx.notify();
                    }
                });
                if told.is_err() {
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
            while changes.changed().await {
                if this.update(cx, |this, cx| this.changed(cx)).is_err() {
                    break;
                }
            }
        });
        let retry_interval = timing.retry_interval;
        let retry = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(retry_interval).await;
                if this
                    .update(cx, |this, cx| this.retry_unreached(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let known = Known {
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
        let mut state = Self {
            client,
            calls,
            voice_failed: None,
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
            pending: Pending::default(),
            shown: Shown::default(),
            asked_names: HashMap::new(),
            members: None,
            watchers: HashMap::new(),
            waiting: HashMap::new(),
            notice_readers: HashMap::new(),
            entry_asks: 0,
            early_entry_answers: Vec::new(),
            call_asks: 0,
            made_invitations: HashSet::new(),
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
            anchors: HashMap::new(),
            marked: HashMap::new(),
            open_recordings: HashMap::new(),
            recording_downloads: HashMap::new(),
            recording_download_numbers: HashMap::new(),
            download_asks: 0,
            opening: HashSet::new(),
            opens_files: false,
            kept_opened: Vec::new(),
            search_asks: 0,
            search_next: None,
            jump: None,
            notifier: std::rc::Rc::new(kept.clone()),
            kept,
            window_active: false,
            seen: HashMap::new(),
            result_asks: 0,
            result_answers: 0,
            reloads: HashMap::new(),
            marks_waiting: HashMap::new(),
            marks_sending: HashMap::new(),
            default_asks: HashMap::new(),
            #[cfg(feature = "test-support")]
            fixture: None,
            #[cfg(feature = "test-support")]
            clock_fixture: None,
            _changes: follow,
            _retry: retry,
            _media: [follow_calls, follow_levels, clock],
        };
        state.changed(cx);
        state
    }

    pub fn calls(&self) -> &Calls {
        &self.calls
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
        let before = self.call_identity();
        let now = view.call.as_ref().map(|call| call.identity.clone());
        let (ended, entered) = (
            before.is_some() && before != now,
            now.is_some() && before != now,
        );
        self.known.media.now = view.call.as_ref().map(|_| time::OffsetDateTime::now_utc());
        self.recording.in_call(now.as_deref());
        self.show_recording();
        if ended {
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
        self.known.media.calls = view;
        if entered {
            // The person is in a call: a request to enter another one is withdrawn, so
            // that its acceptance does not replace this call.
            self.withdraw_entry(cx);
            self.refresh_devices(cx);
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

    /// Starts the share of this device in its call, with the stored resolution and frame
    /// rate. The result is in the view.
    pub fn start_share(&mut self, picture: PictureSource, sound: SoundChoice) {
        self.calls.start_share(CaptureRequest {
            picture,
            sound,
            quality: share_quality(&self.known.media.voice),
        });
    }

    /// Ends the share of this device, and puts away the statement that a share ended.
    pub fn stop_share(&mut self) {
        self.calls.stop_share();
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

    /// The server and the voice channel of the call of this device.
    fn call_channel(&self) -> Option<(ServerHandle, ChannelId)> {
        let target = self.known.media.calls.call.as_ref()?.target;
        let channel_id = target.place.channel_id()?;
        Some((self.client.server(target.server_id)?, channel_id))
    }

    /// Starts a recording of the call of this device. The server answers at once and
    /// tells by an event whether the recorder took it (ADR 0604).
    pub fn start_recording(&mut self, cx: &mut Context<Self>) {
        self.request_recording(true, cx);
    }

    /// Asks for the end of the recording of the call of this device.
    pub fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.request_recording(false, cx);
    }

    /// Gives the view what is told about the recording now.
    fn show_recording(&mut self) {
        self.known.recording_problem = self.recording.problem().cloned();
        self.known.recording_busy = self.recording.busy();
        self.pending.recording_failed = self.recording.request_failed().map(str::to_owned);
    }

    fn request_recording(&mut self, start: bool, cx: &mut Context<Self>) {
        let Some((server, channel_id)) = self.call_channel() else {
            return;
        };
        let Some(request) = self.recording.request(start) else {
            return;
        };
        self.show_recording();
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let result = if start {
                api.start_recording(channel_id, None).await.map(|_| ())
            } else {
                api.stop_recording(channel_id).await
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

    /// Opens the microphone of the person. The person does this after the presenter of
    /// meeting mode removed the block: the application never opens it by itself.
    pub fn open_microphone(&mut self) {
        self.calls.set_muted(false);
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
        cx.spawn(async move |this, cx| {
            let invitation = match api.invite_to_call(channel_id, account_id).await {
                Ok(invitation) => invitation,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| this.refused(&asked, &error, cx));
                    return;
                }
            };
            let made = (server_id, invitation.id);
            let kept = this.update(cx, |this, _| {
                this.made_invitations.insert(made);
            });
            if kept.is_err() {
                return;
            }
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
    /// can have missed that: every message can have changed meanwhile, and the state
    /// holds every change of the settings that the server took.
    fn reloaded(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        *self.reloads.entry(server_id).or_default() += 1;
        self.forget_sent_settings(server_id);
        self.forget_quotes(server_id, None, cx);
        self.read_default_after_reload(server_id, cx);
        self.search_after_reload(server_id, cx);
        self.changed(cx);
    }

    /// One change of the state of a server, told by itself. The state of a server holds
    /// a recording only while it runs; how it ended is told here (ADR 0719).
    fn noticed(&mut self, server_id: ServerId, notice: &Notice, cx: &mut Context<Self>) {
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
            // An invitation that the person made was declined, or nobody answered it.
            Event::CallInvitationResolved {
                invitation_id,
                outcome,
            } => {
                let made = self.made_invitations.remove(&(server_id, *invitation_id));
                let told = match outcome {
                    InvitationOutcome::Declined => Some(text::INVITATION_DECLINED),
                    InvitationOutcome::Expired => Some(text::INVITATION_EXPIRED),
                    InvitationOutcome::Accepted | InvitationOutcome::Cancelled => None,
                };
                if let (true, Some(told)) = (made, told) {
                    self.pending.call_told = Some(told.to_owned());
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Reads the sound devices and the microphone permission again, on a background
    /// thread: the list of a system can take a moment.
    pub fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        let calls = self.calls.clone();
        let reading = cx
            .background_executor()
            .spawn(async move { (calls.devices(), calls.microphone_access()) });
        cx.spawn(async move |this, cx| {
            let (devices, access) = reading.await;
            let _ = this.update(cx, |this, cx| {
                match devices {
                    Ok(devices) => this.known.media.devices = devices,
                    Err(error) => tracing::warn!(%error, "the sound devices were not read"),
                }
                this.known.media.access = Some(access);
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

    /// Closes the microphone of the person, or opens it. The setting holds outside a call
    /// too, for the next one.
    pub fn toggle_microphone(&mut self) {
        self.calls.set_muted(!self.calls.view().muted);
    }

    pub fn toggle_sound(&mut self) {
        self.calls.set_deafened(!self.calls.view().deafened);
    }

    /// The push-to-talk key is held or released.
    pub fn set_talking(&mut self, talking: bool) {
        self.calls.set_talking(talking);
    }

    pub fn retry_microphone(&mut self, cx: &mut Context<Self>) {
        self.calls.retry_microphone();
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
            self.selection.channels.insert(workspace_id, channel_id);
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
    pub fn settings_error(&self) -> Option<&str> {
        [Setting::Scale, Setting::ServerBook]
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
            return view::derive(&self.snapshots(), &self.selection, &known);
        }
        view::derive(&self.snapshots(), &self.selection, &self.known)
    }

    /// Draws this state of the calls in place of the calls of the client. The screen tool
    /// uses it to draw a call without a media server.
    #[cfg(feature = "test-support")]
    pub fn show_media_fixture(&mut self, media: MediaSnapshot, cx: &mut Context<Self>) {
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

    /// Runs after every change of the client core and of an event stream: starts and ends
    /// the work that depends on what exists and on what is shown, and draws again.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let servers = self.client.servers();
        let ids: HashSet<ServerId> = servers.iter().map(ServerHandle::id).collect();
        self.watchers.retain(|id, _| ids.contains(id));
        self.waiting.retain(|id, _| ids.contains(id));
        self.notice_readers.retain(|id, _| ids.contains(id));
        self.made_invitations.retain(|(id, _)| ids.contains(id));
        self.pending
            .acknowledge_failed
            .retain(|id, _| ids.contains(id));
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
                            Err(_) => this.reloaded(id, cx),
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
        let snapshots = self.snapshots();
        self.settle_sent_settings(&snapshots);
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
        self.shown = shown;
        self.ask_missing_names(&view, cx);
        self.follow_places(shown, cx);
        self.follow_recordings(shown, cx);
        self.follow_jump(shown);
        self.tell_alerts(&snapshots, shown, cx);
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

    /// A person in a voice channel, or of a direct conversation, who is not in the member
    /// list of the workspace has no name there. The name is read once from the profile
    /// route.
    fn ask_missing_names(&mut self, view: &ShellView, cx: &mut Context<Self>) {
        let (Some(selected), Some(workspace)) = (&view.selected, view.workspace()) else {
            return;
        };
        if !matches!(workspace.members, view::Members::Loaded(_)) {
            return;
        }
        let server_id = selected.id;
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let missing: Vec<AccountId> = workspace
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .filter_map(|row| row.voice.as_ref())
            .flat_map(|voice| voice.occupants.iter())
            .filter(|occupant| occupant.name.is_none())
            .map(|occupant| occupant.account_id)
            // The other person of a direct conversation can be a member of another
            // workspace, or an administrator of the server (ADR 0307).
            .chain(
                workspace
                    .directs
                    .iter()
                    .filter(|row| row.name.is_none())
                    .map(|row| row.account_id),
            )
            .filter(|account_id| {
                let read = self
                    .asked_names
                    .entry((server_id, *account_id))
                    .or_insert(NameRead::Unanswered(0));
                match *read {
                    NameRead::Unanswered(tries) if tries < NAME_TRIES => {
                        *read = NameRead::Asking(tries + 1);
                        true
                    }
                    NameRead::Unanswered(_) | NameRead::Asking(_) | NameRead::Settled => false,
                }
            })
            .collect();
        for account_id in missing {
            let api = server.api();
            cx.spawn(async move |this, cx| {
                let read = api.profile(account_id).await;
                let _ = this.update(cx, |this, cx| {
                    let key = (server_id, account_id);
                    let tries = match this.asked_names.get(&key) {
                        Some(NameRead::Asking(tries)) => *tries,
                        _ => NAME_TRIES,
                    };
                    let next = match read {
                        Ok(profile) => {
                            this.known.names.insert(key, profile.name);
                            cx.notify();
                            NameRead::Settled
                        }
                        // No answer: the name is asked for again with a later change.
                        Err(error) if error.is_unavailable() => NameRead::Unanswered(tries),
                        // A refusal is final: the account may not see the profile.
                        Err(_) => NameRead::Settled,
                    };
                    this.asked_names.insert(key, next);
                });
            })
            .detach();
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
        self.end_jump();
        self.navigations += 1;
        self.selection.channels.insert(workspace_id, channel_id);
        // The channel takes the place of a direct conversation that the server showed.
        if let Some(server_id) = self.shown.server {
            self.selection.conversations.remove(&server_id);
        }
        self.changed(cx);
    }

    /// Asks the server what this device is now, or logs in.
    pub fn connect(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.connect(), cx);
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
        if !self.pending.removing.insert(server_id) {
            return false;
        }
        let client = self.client.clone();
        Self::queue_write(
            cx,
            move || client.remove_server(server_id),
            move |this, removed, cx| this.removed(server_id, removed, cx),
        );
        cx.notify();
        true
    }

    fn removed(
        &mut self,
        server_id: ServerId,
        result: Result<(), junto_client_core::manager::RemoveError>,
        cx: &mut Context<Self>,
    ) {
        self.pending.removing.remove(&server_id);
        match result {
            Ok(()) => {
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
        if self.pending.removing.contains(&server_id) || !self.pending.cancelling.insert(server_id)
        {
            return;
        }
        self.pending.removing.insert(server_id);
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
                this.pending.cancelling.remove(&server_id);
                this.pending.removing.remove(&server_id);
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
        if !self.pending.acknowledging.insert(recovery_id) {
            return;
        }
        self.pending.acknowledge_failed.remove(&server_id);
        let sending = server.acknowledge_recovery(recovery_id);
        cx.spawn(async move |this, cx| {
            let result = sending.await;
            let _ = this.update(cx, |this, cx| {
                this.pending.acknowledging.remove(&recovery_id);
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
                        .insert(server_id, text::flow_error(&FlowError::from(error)));
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
