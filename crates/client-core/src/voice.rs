//! The sound settings of one installation: the chosen devices, the processing of the
//! microphone, the input mode, and the volume that the person set for other people
//! (ADR 0507, ADR 0701, ADR 0710). They belong to no server, except the volumes, which name
//! an account of a server.

use std::collections::BTreeMap;

use junto_protocol::ids::{AccountId, ServerId};
use serde::{Deserialize, Serialize};

use crate::paths::{DataDir, VOICE_FILE};
use crate::storage::{StorageError, Versioned};

pub const VOICE_FORMAT_VERSION: u32 = 1;

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
    migrations: &[],
    limit: 512 * 1024,
};

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
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            echo_cancellation: true,
            noise_suppression: true,
            automatic_gain: true,
            input_mode: InputMode::default(),
            talk_key: TalkKey::default(),
            volumes: BTreeMap::new(),
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
        self
    }
}

#[derive(Serialize, Deserialize)]
struct VoiceFile {
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
            input_device: file.input_device,
            output_device: file.output_device,
            echo_cancellation: file.echo_cancellation,
            noise_suppression: file.noise_suppression,
            automatic_gain: file.automatic_gain,
            input_mode: file.input_mode,
            talk_key: file.talk_key,
            volumes: file.volumes,
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
                input_device: settings.input_device.clone(),
                output_device: settings.output_device.clone(),
                echo_cancellation: settings.echo_cancellation,
                noise_suppression: settings.noise_suppression,
                automatic_gain: settings.automatic_gain,
                input_mode: settings.input_mode,
                talk_key: settings.talk_key,
                volumes: settings.volumes.clone(),
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
        assert!(text.contains("\"format_version\": 1"), "{text}");
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
        let newer = r#"{"format_version":2,"later":true}"#;
        fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.load(),
            Err(StorageError::NewerFormat { found: 2, .. })
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
