//! The sound settings of one installation: the chosen devices, the processing of the
//! microphone, the input mode, the volume that the person set for other people, and what
//! the person set for screen shares, and the sounds of call events (ADR 0502, ADR 0507,
//! ADR 0701, ADR 0707, ADR 0710). They belong to no server, except the volumes, which name
//! an account of a server.

use std::collections::{BTreeMap, BTreeSet};

use junto_protocol::ids::{AccountId, ServerId};
use serde::{Deserialize, Serialize};

use crate::paths::{DataDir, VOICE_FILE};
use crate::storage::{StorageError, Versioned};

pub const VOICE_FORMAT_VERSION: u32 = 6;

/// The frame rates of a share that a person chooses from.
pub const SHARE_FRAME_RATES: [u32; 3] = [15, 30, 60];
pub const SHARE_FRAME_RATE_DEFAULT: u32 = 30;

/// The volume of a person that is not stored: 100 %.
pub const VOLUME_DEFAULT_PERCENT: u16 = 100;
pub const VOLUME_MAX_PERCENT: u16 = 200;
/// The most volumes that the file holds. A person sets few by hand.
pub const VOLUMES_STORED: usize = 2048;
/// The longest identifier of a device that is stored.
const DEVICE_ID_BYTES: usize = 512;

const VOICE: Versioned = Versioned {
    name: VOICE_FILE,
    kind: "sound settings",
    version: VOICE_FORMAT_VERSION,
    migrations: &[
        settings_of_shares,
        settings_of_camera,
        settings_of_call_sounds,
        settings_of_sound_kinds,
        settings_of_individual_sounds,
    ],
    limit: 512 * 1024,
};

/// Version 1 had no settings of screen shares. Version 2 holds them, with these values for
/// a file that had none.
fn settings_of_shares(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    let settings = value
        .as_object_mut()
        .ok_or("the sound settings are not an object")?;
    let defaults = VoiceSettings::default();
    for (name, default) in [
        (
            "share_resolution",
            serde_json::json!(defaults.share_resolution),
        ),
        (
            "share_frame_rate",
            serde_json::json!(defaults.share_frame_rate),
        ),
        ("share_volume", serde_json::json!(defaults.share_volume)),
        (
            "hide_people_without_video",
            serde_json::json!(defaults.hide_people_without_video),
        ),
    ] {
        settings.entry(name).or_insert(default);
    }
    Ok(value)
}

fn settings_of_camera(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    value
        .as_object_mut()
        .ok_or("the sound settings are not an object")?
        .entry("camera_device")
        .or_insert(serde_json::Value::Null);
    Ok(value)
}

/// Version 3 had no setting of the sounds of call events. They are on in version 4.
fn settings_of_call_sounds(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    value
        .as_object_mut()
        .ok_or("the sound settings are not an object")?
        .entry("call_sounds")
        .or_insert(serde_json::Value::Bool(true));
    Ok(value)
}

/// Version 4 had one switch of the sounds of call events. Version 5 has a switch for all
/// sounds and one per kind: a file whose switch was off keeps the kinds of call events off,
/// and the kinds that did not exist before, the messages and the ring, on.
fn settings_of_sound_kinds(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    let settings = value
        .as_object_mut()
        .ok_or("the sound settings are not an object")?;
    let call_sounds = settings
        .remove("call_sounds")
        .and_then(|on| on.as_bool())
        .unwrap_or(true);
    let off: Vec<SoundKind> = if call_sounds {
        Vec::new()
    } else {
        SoundKind::ALL
            .into_iter()
            .filter(|kind| kind.of_a_call())
            .collect()
    };
    settings.insert(
        "sounds".to_owned(),
        serde_json::json!({ "all": true, "off": off }),
    );
    Ok(value)
}

fn settings_of_individual_sounds(
    mut value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let off = value
        .get_mut("sounds")
        .and_then(|sounds| sounds.get_mut("off"))
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("the sound switches are absent")?;
    let mut expanded = Vec::new();
    for kind in off.iter() {
        let names: &[&str] = match kind.as_str() {
            Some("people") => &["person_joined", "person_left"],
            Some("own_microphone_and_sound") => {
                &["microphone_off", "microphone_on", "sound_off", "sound_on"]
            }
            Some("own_call") => &["call_joined", "call_left", "moved", "connection_lost"],
            Some("push_to_talk") => &["talk_starts", "talk_ends"],
            Some("shares") => &[
                "share_started",
                "share_ended",
                "viewer_joined",
                "viewer_left",
            ],
            Some("requests") => &["invitation", "request"],
            Some("recording") => &["recording_started", "recording_stopped"],
            Some("hands") => &["hand_raised"],
            _ => {
                expanded.push(kind.clone());
                continue;
            }
        };
        expanded.extend(names.iter().map(|name| serde_json::Value::from(*name)));
    }
    *off = expanded;
    Ok(value)
}

/// Each notification or call sound has an independent switch (ADR 0707).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundKind {
    Messages,
    Mentions,
    Ring,
    CallJoined,
    CallLeft,
    Moved,
    ConnectionLost,
    PersonJoined,
    PersonLeft,
    MicrophoneOff,
    MicrophoneOn,
    SoundOff,
    SoundOn,
    TalkStarts,
    TalkEnds,
    ShareStarted,
    ShareEnded,
    ViewerJoined,
    ViewerLeft,
    Invitation,
    Request,
    RecordingStarted,
    RecordingStopped,
    HandRaised,
}

impl SoundKind {
    pub const ALL: [Self; 24] = [
        Self::Messages,
        Self::Mentions,
        Self::Ring,
        Self::CallJoined,
        Self::CallLeft,
        Self::Moved,
        Self::ConnectionLost,
        Self::PersonJoined,
        Self::PersonLeft,
        Self::MicrophoneOff,
        Self::MicrophoneOn,
        Self::SoundOff,
        Self::SoundOn,
        Self::TalkStarts,
        Self::TalkEnds,
        Self::ShareStarted,
        Self::ShareEnded,
        Self::ViewerJoined,
        Self::ViewerLeft,
        Self::Invitation,
        Self::Request,
        Self::RecordingStarted,
        Self::RecordingStopped,
        Self::HandRaised,
    ];

    fn of_a_call(self) -> bool {
        !matches!(self, Self::Messages | Self::Mentions | Self::Ring)
    }
}

/// The switches of the sounds: one for all sounds, and the kinds that the person turned
/// off. A kind that is not named is on, so a kind that a later version adds starts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sounds {
    pub all: bool,
    #[serde(default)]
    pub off: BTreeSet<SoundKind>,
}

impl Default for Sounds {
    fn default() -> Self {
        Self {
            all: true,
            off: BTreeSet::new(),
        }
    }
}

impl Sounds {
    /// The sounds of a kind play: all sounds are on, and so is the kind.
    pub fn plays(&self, kind: SoundKind) -> bool {
        self.all && !self.off.contains(&kind)
    }

    /// The switch of a kind, whatever the switch of all sounds is.
    pub fn kind_on(&self, kind: SoundKind) -> bool {
        !self.off.contains(&kind)
    }

    pub fn set(&mut self, kind: SoundKind, on: bool) {
        if on {
            self.off.remove(&kind);
        } else {
            self.off.insert(kind);
        }
    }
}

/// The largest picture that a share of this device sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareResolution {
    /// At most 1280 × 720.
    P720,
    /// At most 1920 × 1080.
    #[default]
    P1080,
    /// The size of the screen or the window.
    Source,
}

/// When the microphone is sent (ADR 0507).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    /// The open microphone is sent all the time.
    #[default]
    VoiceActivation,
    /// The microphone is sent while the key is held.
    PushToTalk,
}

/// The key of push-to-talk: a modifier key, which types nothing into a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalkKey {
    /// The Option key on macOS.
    #[default]
    Alt,
    Control,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceSettings {
    pub camera_device: Option<String>,
    /// The identifier of the chosen input device. Absent: the default of the system.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub automatic_gain: bool,
    pub input_mode: InputMode,
    pub talk_key: TalkKey,
    /// The volume in percent that the person set for an account of a server, from 0 to
    /// 200. An account without an entry has 100.
    pub volumes: BTreeMap<ServerId, BTreeMap<AccountId, u16>>,
    pub share_resolution: ShareResolution,
    /// The pictures per second of a share, one of [`SHARE_FRAME_RATES`].
    pub share_frame_rate: u32,
    /// The volume in percent of the sound of the share that the person watches, from 0 to
    /// 200.
    pub share_volume: u16,
    /// The strip of a call shows only the people who send a camera picture.
    pub hide_people_without_video: bool,
    /// The switches of the sounds of the application (ADR 0707).
    pub sounds: Sounds,
    /// Voice activation sends the microphone only while its level reaches this many dBFS,
    /// from [`THRESHOLD_RANGE`]. Absent: all of the open microphone is sent.
    pub voice_threshold: Option<i16>,
}

/// The levels that the sensitivity of voice activation can have, in dBFS: the range of the
/// input meter.
pub const THRESHOLD_RANGE: (i16, i16) = (-60, -12);
/// The level that the sensitivity has when the person turns it on.
pub const THRESHOLD_DEFAULT: i16 = -45;

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            camera_device: None,
            input_device: None,
            output_device: None,
            echo_cancellation: true,
            noise_suppression: true,
            automatic_gain: true,
            input_mode: InputMode::default(),
            talk_key: TalkKey::default(),
            volumes: BTreeMap::new(),
            share_resolution: ShareResolution::default(),
            share_frame_rate: SHARE_FRAME_RATE_DEFAULT,
            share_volume: VOLUME_DEFAULT_PERCENT,
            hide_people_without_video: false,
            sounds: Sounds::default(),
            voice_threshold: None,
        }
    }
}

impl VoiceSettings {
    pub fn volume(&self, server_id: ServerId, account_id: AccountId) -> u16 {
        self.volumes
            .get(&server_id)
            .and_then(|accounts| accounts.get(&account_id))
            .copied()
            .unwrap_or(VOLUME_DEFAULT_PERCENT)
    }

    /// Sets the volume of one account. A volume of 100 % is the absence of an entry.
    pub fn set_volume(&mut self, server_id: ServerId, account_id: AccountId, percent: u16) {
        let percent = percent.min(VOLUME_MAX_PERCENT);
        if percent == VOLUME_DEFAULT_PERCENT {
            if let Some(accounts) = self.volumes.get_mut(&server_id) {
                accounts.remove(&account_id);
                if accounts.is_empty() {
                    self.volumes.remove(&server_id);
                }
            }
        } else {
            self.volumes
                .entry(server_id)
                .or_default()
                .insert(account_id, percent);
        }
    }

    /// The settings as a file holds them: values inside their ranges, no identifier of a
    /// device that is longer than a system gives, and at most [`VOLUMES_STORED`] volumes.
    fn bounded(mut self) -> Self {
        let usable = |device: Option<String>| {
            device.filter(|id| {
                !id.is_empty() && id.len() <= DEVICE_ID_BYTES && !id.chars().any(char::is_control)
            })
        };
        self.camera_device = usable(self.camera_device);
        self.input_device = usable(self.input_device);
        self.output_device = usable(self.output_device);
        let mut kept = 0;
        for accounts in self.volumes.values_mut() {
            accounts.retain(|_, percent| {
                *percent = (*percent).min(VOLUME_MAX_PERCENT);
                kept += 1;
                *percent != VOLUME_DEFAULT_PERCENT && kept <= VOLUMES_STORED
            });
        }
        self.volumes.retain(|_, accounts| !accounts.is_empty());
        if !SHARE_FRAME_RATES.contains(&self.share_frame_rate) {
            self.share_frame_rate = SHARE_FRAME_RATE_DEFAULT;
        }
        self.share_volume = self.share_volume.min(VOLUME_MAX_PERCENT);
        self.voice_threshold = self
            .voice_threshold
            .map(|threshold| threshold.clamp(THRESHOLD_RANGE.0, THRESHOLD_RANGE.1));
        self
    }
}

#[derive(Serialize, Deserialize)]
struct VoiceFile {
    #[serde(default)]
    camera_device: Option<String>,
    format_version: u32,
    #[serde(default)]
    input_device: Option<String>,
    #[serde(default)]
    output_device: Option<String>,
    echo_cancellation: bool,
    noise_suppression: bool,
    automatic_gain: bool,
    input_mode: InputMode,
    talk_key: TalkKey,
    #[serde(default)]
    volumes: BTreeMap<ServerId, BTreeMap<AccountId, u16>>,
    share_resolution: ShareResolution,
    share_frame_rate: u32,
    share_volume: u16,
    hide_people_without_video: bool,
    sounds: Sounds,
    #[serde(default)]
    voice_threshold: Option<i16>,
}

#[derive(Debug, Clone)]
pub struct VoiceStore {
    data_dir: DataDir,
}

impl VoiceStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    /// The stored settings, or the defaults when there is no file.
    pub fn load(&self) -> Result<VoiceSettings, StorageError> {
        let Some(file) = VOICE.load::<VoiceFile>(self.data_dir.files())? else {
            return Ok(VoiceSettings::default());
        };
        Ok(VoiceSettings {
            camera_device: file.camera_device,
            input_device: file.input_device,
            output_device: file.output_device,
            echo_cancellation: file.echo_cancellation,
            noise_suppression: file.noise_suppression,
            automatic_gain: file.automatic_gain,
            input_mode: file.input_mode,
            talk_key: file.talk_key,
            volumes: file.volumes,
            share_resolution: file.share_resolution,
            share_frame_rate: file.share_frame_rate,
            share_volume: file.share_volume,
            hide_people_without_video: file.hide_people_without_video,
            sounds: file.sounds,
            voice_threshold: file.voice_threshold,
        }
        .bounded())
    }

    /// Changes the stored settings in one step under the lock of the data directory, as
    /// the settings of the interface are changed (ADR 0710): the file is read again,
    /// changed and written. A file that is not a file of sound settings is replaced with
    /// the defaults and the change; a file of a newer format, or one that is larger than
    /// this client reads, stays, and the change fails. Returns what was written.
    pub fn update(
        &self,
        change: impl FnOnce(&mut VoiceSettings),
    ) -> Result<VoiceSettings, StorageError> {
        let _lock = self.data_dir.lock()?;
        let mut settings = match self.load() {
            Ok(settings) => settings,
            Err(StorageError::Corrupted { .. }) => VoiceSettings::default(),
            Err(other) => return Err(other),
        };
        change(&mut settings);
        let settings = settings.bounded();
        VOICE.store(
            self.data_dir.files(),
            &VoiceFile {
                format_version: VOICE_FORMAT_VERSION,
                camera_device: settings.camera_device.clone(),
                input_device: settings.input_device.clone(),
                output_device: settings.output_device.clone(),
                echo_cancellation: settings.echo_cancellation,
                noise_suppression: settings.noise_suppression,
                automatic_gain: settings.automatic_gain,
                input_mode: settings.input_mode,
                talk_key: settings.talk_key,
                volumes: settings.volumes.clone(),
                share_resolution: settings.share_resolution,
                share_frame_rate: settings.share_frame_rate,
                share_volume: settings.share_volume,
                hide_people_without_video: settings.hide_people_without_video,
                sounds: settings.sounds.clone(),
                voice_threshold: settings.voice_threshold,
            },
        )?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn store() -> (tempfile::TempDir, DataDir, VoiceStore) {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let store = VoiceStore::new(data_dir.clone());
        (directory, data_dir, store)
    }

    #[test]
    fn the_settings_start_with_their_defaults_and_keep_what_is_changed() {
        let (_directory, data_dir, store) = store();
        let defaults = store.load().unwrap();
        assert_eq!(defaults, VoiceSettings::default());
        assert!(
            defaults.echo_cancellation && defaults.noise_suppression && defaults.automatic_gain
        );
        assert_eq!(defaults.input_mode, InputMode::VoiceActivation);
        assert!(!data_dir.root().join(VOICE_FILE).exists());

        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let written = store
            .update(|settings| {
                settings.input_device = Some("desk".to_owned());
                settings.noise_suppression = false;
                settings.input_mode = InputMode::PushToTalk;
                settings.talk_key = TalkKey::Control;
                settings.set_volume(server_id, account_id, 140);
            })
            .unwrap();
        assert_eq!(store.load().unwrap(), written);
        assert_eq!(written.input_device.as_deref(), Some("desk"));
        assert_eq!(written.output_device, None);
        assert_eq!(written.volume(server_id, account_id), 140);
        assert_eq!(written.volume(server_id, AccountId::generate()), 100);
        let text = fs::read_to_string(data_dir.root().join(VOICE_FILE)).unwrap();
        assert!(text.contains("\"format_version\": 6"), "{text}");
        assert!(text.contains("\"input_mode\": \"push_to_talk\""), "{text}");
    }

    #[test]
    fn a_volume_stays_in_its_range_and_a_volume_of_100_percent_is_not_stored() {
        let (_directory, _data_dir, store) = store();
        let (server_id, loud, usual) = (
            ServerId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let written = store
            .update(|settings| {
                settings.set_volume(server_id, loud, 900);
                settings.set_volume(server_id, usual, 60);
            })
            .unwrap();
        assert_eq!(written.volume(server_id, loud), VOLUME_MAX_PERCENT);
        let written = store
            .update(|settings| {
                settings.set_volume(server_id, usual, 100);
                settings.set_volume(server_id, loud, 100);
            })
            .unwrap();
        assert!(written.volumes.is_empty());
    }

    #[test]
    fn a_file_holds_a_bounded_number_of_volumes_and_no_device_text_of_any_length() {
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let written = store
            .update(|settings| {
                for _ in 0..VOLUMES_STORED + 10 {
                    settings.set_volume(server_id, AccountId::generate(), 50);
                }
                settings.input_device = Some("x".repeat(DEVICE_ID_BYTES + 1));
                settings.output_device = Some("line\nbreak".to_owned());
            })
            .unwrap();
        assert_eq!(written.volumes[&server_id].len(), VOLUMES_STORED);
        assert_eq!((written.input_device, written.output_device), (None, None));
        let size = fs::metadata(data_dir.root().join(VOICE_FILE))
            .unwrap()
            .len();
        assert!(size < VOICE.limit, "{size}");
        assert_eq!(
            store.load().unwrap().volumes[&server_id].len(),
            VOLUMES_STORED
        );
    }

    #[test]
    fn a_file_of_a_newer_format_stays_and_an_unreadable_file_is_replaced() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(VOICE_FILE);
        let newer = r#"{"format_version":7,"later":true}"#;
        fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.load(),
            Err(StorageError::NewerFormat { found: 7, .. })
        ));
        assert!(matches!(
            store.update(|settings| settings.automatic_gain = false),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), newer);

        fs::write(&file, "not json").unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        let written = store
            .update(|settings| settings.automatic_gain = false)
            .unwrap();
        assert!(!written.automatic_gain && written.echo_cancellation);
        assert_eq!(store.load().unwrap(), written);
    }

    #[test]
    fn a_file_of_version_1_gets_the_settings_of_shares_and_keeps_what_it_held() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(VOICE_FILE);
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let first = format!(
            r#"{{"format_version":1,"input_device":"desk","echo_cancellation":false,
                "noise_suppression":true,"automatic_gain":true,"input_mode":"push_to_talk",
                "talk_key":"control","volumes":{{"{server_id}":{{"{account_id}":60}}}}}}"#
        );
        fs::write(&file, first).unwrap();
        let read = store.load().unwrap();
        assert_eq!(read.input_device.as_deref(), Some("desk"));
        assert!(!read.echo_cancellation);
        assert_eq!(read.input_mode, InputMode::PushToTalk);
        assert_eq!(read.volume(server_id, account_id), 60);
        assert_eq!(
            (
                read.share_resolution,
                read.share_frame_rate,
                read.share_volume
            ),
            (ShareResolution::P1080, 30, 100)
        );
        assert!(!read.hide_people_without_video);
        assert_eq!(read.sounds, Sounds::default());

        // The next change writes the file in the present version.
        let written = store
            .update(|settings| {
                settings.share_resolution = ShareResolution::Source;
                settings.share_frame_rate = 60;
                settings.share_volume = 40;
                settings.hide_people_without_video = true;
            })
            .unwrap();
        assert_eq!(store.load().unwrap(), written);
        assert_eq!(written.volume(server_id, account_id), 60);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"format_version\": 6"), "{text}");
        assert!(text.contains("\"share_resolution\": \"source\""), "{text}");
    }

    #[test]
    fn a_file_of_version_3_gets_the_sounds_of_call_events_on_and_they_can_be_turned_off() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(VOICE_FILE);
        let third = r#"{"format_version":3,"camera_device":null,"input_device":null,
            "output_device":"phones","echo_cancellation":true,"noise_suppression":true,
            "automatic_gain":false,"input_mode":"voice_activation","talk_key":"alt",
            "volumes":{},"share_resolution":"p1080","share_frame_rate":30,
            "share_volume":100,"hide_people_without_video":true}"#;
        fs::write(&file, third).unwrap();
        let read = store.load().unwrap();
        assert_eq!(read.sounds, Sounds::default());
        assert!(read.hide_people_without_video && !read.automatic_gain);
        assert_eq!(read.output_device.as_deref(), Some("phones"));

        let written = store
            .update(|settings| settings.sounds.set(SoundKind::PersonJoined, false))
            .unwrap();
        assert!(
            !written.sounds.plays(SoundKind::PersonJoined)
                && written.sounds.plays(SoundKind::Messages)
        );
        assert_eq!(store.load().unwrap(), written);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"person_joined\""), "{text}");
    }

    #[test]
    fn a_file_of_version_4_with_the_call_sounds_off_keeps_them_off_and_the_new_kinds_on() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(VOICE_FILE);
        let fourth = |on: bool| {
            format!(
                r#"{{"format_version":4,"camera_device":null,"input_device":null,
                "output_device":null,"echo_cancellation":true,"noise_suppression":true,
                "automatic_gain":true,"input_mode":"voice_activation","talk_key":"alt",
                "volumes":{{}},"share_resolution":"p1080","share_frame_rate":30,
                "share_volume":100,"hide_people_without_video":false,"call_sounds":{on}}}"#
            )
        };
        fs::write(&file, fourth(false)).unwrap();
        let read = store.load().unwrap();
        assert!(read.sounds.all);
        let on: Vec<SoundKind> = SoundKind::ALL
            .into_iter()
            .filter(|kind| read.sounds.plays(*kind))
            .collect();
        assert_eq!(
            on,
            [SoundKind::Messages, SoundKind::Mentions, SoundKind::Ring]
        );

        fs::write(&file, fourth(true)).unwrap();
        assert_eq!(store.load().unwrap().sounds, Sounds::default());

        // The switch of all sounds turns every kind off and keeps the switch of each kind.
        let written = store
            .update(|settings| {
                settings.sounds.set(SoundKind::HandRaised, false);
                settings.sounds.all = false;
            })
            .unwrap();
        assert!(
            SoundKind::ALL
                .into_iter()
                .all(|kind| !written.sounds.plays(kind))
        );
        assert!(
            !written.sounds.kind_on(SoundKind::HandRaised)
                && written.sounds.kind_on(SoundKind::Ring)
        );
        assert_eq!(store.load().unwrap(), written);
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains("call_sounds"), "{text}");
    }

    #[test]
    fn version_five_expands_disabled_groups_without_enabling_any_of_their_sounds() {
        let (_directory, data_dir, store) = store();
        store.update(|_| {}).unwrap();
        let file = data_dir.root().join(VOICE_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        value["format_version"] = 5.into();
        value["sounds"]["off"] = serde_json::json!([
            "people",
            "own_microphone_and_sound",
            "own_call",
            "push_to_talk",
            "shares",
            "requests",
            "recording",
            "hands"
        ]);
        fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
        let read = store.load().unwrap();
        for kind in SoundKind::ALL {
            assert_eq!(read.sounds.plays(kind), !kind.of_a_call(), "{kind:?}");
        }
        let changed = store
            .update(|settings| settings.sounds.set(SoundKind::PersonJoined, true))
            .unwrap();
        assert!(changed.sounds.plays(SoundKind::PersonJoined));
        assert!(!changed.sounds.plays(SoundKind::PersonLeft));
        assert_eq!(store.load().unwrap(), changed);
    }

    #[test]
    fn a_share_setting_outside_its_values_is_read_as_a_value_inside() {
        let (_directory, _data_dir, store) = store();
        let written = store
            .update(|settings| {
                settings.share_frame_rate = 24;
                settings.share_volume = 900;
            })
            .unwrap();
        assert_eq!(written.share_frame_rate, SHARE_FRAME_RATE_DEFAULT);
        assert_eq!(written.share_volume, VOLUME_MAX_PERCENT);
    }

    #[test]
    fn a_change_of_one_setting_keeps_what_another_program_wrote_of_the_others() {
        let (_directory, data_dir, first) = store();
        let second = VoiceStore::new(data_dir);
        first
            .update(|settings| settings.echo_cancellation = false)
            .unwrap();
        let written = second
            .update(|settings| settings.talk_key = TalkKey::Control)
            .unwrap();
        assert!(!written.echo_cancellation);
        assert_eq!(written.talk_key, TalkKey::Control);
    }
}

#[cfg(test)]
mod camera_tests {
    use super::*;
    #[test]
    fn camera_choice_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path().join("client")).unwrap();
        let store = VoiceStore::new(data.clone());
        assert_eq!(store.load().unwrap().camera_device, None);
        store
            .update(|settings| settings.camera_device = Some("chosen-camera".into()))
            .unwrap();
        assert_eq!(
            VoiceStore::new(data)
                .load()
                .unwrap()
                .camera_device
                .as_deref(),
            Some("chosen-camera")
        );
        store
            .update(|settings| settings.camera_device = Some("x".repeat(513)))
            .unwrap();
        assert_eq!(store.load().unwrap().camera_device, None);
    }
}
