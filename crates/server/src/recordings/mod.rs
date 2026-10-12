//! Recordings of calls: the server's record of each recording and its segments, the
//! controller that makes the recorder agree with it (ADR 0601, ADR 0604), the processing of
//! the files of a finished recording (ADR 0606), and its transcript (ADR 0607).

pub mod check;
pub mod contract;
pub mod control;
pub mod files;
pub mod process;
pub mod store;
pub mod tools;
pub mod transcribe;
pub mod transcripts;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, RecordingId};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::config::{RecorderSettings, RecordingLimits};
use crate::error::{ApiError, ApiResult};
use crate::messages::transfers::{Permit, Transfers};
use crate::recorder::Recorder;
use crate::recordings::tools::Tools;
use crate::storage::{self, Disk};

/// What a server with a recorder has: the recorder, the disk that it writes to, where it
/// writes, and the programs that read its files.
#[derive(Clone)]
pub struct Backend {
    pub recorder: Arc<dyn Recorder>,
    pub disk: Arc<dyn Disk>,
    pub settings: RecorderSettings,
    pub tools: Tools,
}

impl Backend {
    pub fn new(
        recorder: Arc<dyn Recorder>,
        disk: Arc<dyn Disk>,
        settings: RecorderSettings,
        limits: &RecordingLimits,
    ) -> Self {
        let tools = Tools::new(
            settings.ffmpeg.clone(),
            settings.ffprobe.clone(),
            limits.tool_time_limit,
        );
        Self {
            recorder,
            disk,
            settings,
            tools,
        }
    }
}

/// The main share that the composite of a recording was told last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToldLayout {
    pub job_id: String,
    pub main_share: Option<String>,
    /// The participant of the job was in the room when the server looked last.
    pub present: bool,
}

pub struct Recordings {
    backend: Option<Backend>,
    wake: Notify,
    /// The recordings that a pass runs over, each with whether a further pass was asked for
    /// in the meantime.
    tending: Mutex<HashMap<RecordingId, bool>>,
    layouts: Mutex<HashMap<RecordingId, ToldLayout>>,
    low: AtomicBool,
    /// Names this server process in the lease of a transcription job (ADR 0607).
    instance: Uuid,
    files_wait: Notify,
    jobs_wait: Notify,
    downloads: Arc<Transfers>,
    /// The name below `rescued/` that the sweep of saved audio examined last, when more
    /// names follow it (ADR 0606).
    rescued_after: Mutex<Option<String>>,
}

/// One pass of the controller over one recording. A recording has at most one at a time.
pub(crate) struct Pass<'a> {
    recordings: &'a Recordings,
    id: RecordingId,
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        let again = self.recordings.tending().remove(&self.id);
        if again == Some(true) {
            self.recordings.poke();
        }
    }
}

impl Recordings {
    pub fn new(backend: Option<Backend>, limits: &RecordingLimits) -> Self {
        let places = |number: u32| usize::try_from(number).unwrap_or(usize::MAX);
        let downloads = Transfers::new(
            places(limits.most_downloads),
            places(limits.downloads_per_account),
        );
        Self {
            backend,
            wake: Notify::new(),
            tending: Mutex::new(HashMap::new()),
            layouts: Mutex::new(HashMap::new()),
            low: AtomicBool::new(false),
            instance: Uuid::new_v4(),
            files_wait: Notify::new(),
            jobs_wait: Notify::new(),
            downloads: Arc::new(downloads),
            rescued_after: Mutex::new(None),
        }
    }

    pub fn instance(&self) -> Uuid {
        self.instance
    }

    /// Makes the processing look for recordings and for directories to delete now.
    pub fn process_now(&self) {
        self.files_wait.notify_one();
    }

    pub(crate) async fn asked_to_process(&self) {
        self.files_wait.notified().await;
    }

    /// Makes the server look for a transcription job now.
    pub fn transcribe_now(&self) {
        self.jobs_wait.notify_one();
    }

    pub(crate) async fn asked_to_transcribe(&self) {
        self.jobs_wait.notified().await;
    }

    /// A place among the downloads that the server sends at one time, for a download of
    /// `account`. `None` when every place is taken, or every place of the account
    /// (ADR 0608).
    pub fn download(&self, account: AccountId) -> Option<Permit> {
        self.downloads.acquire(account)
    }

    /// Where the sweep of saved audio continues: after this name, or at the first name.
    pub(crate) fn rescued_after(&self) -> Option<String> {
        self.rescued_after
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn continue_rescued_after(&self, name: Option<String>) {
        *self
            .rescued_after
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = name;
    }

    pub fn backend(&self) -> Option<&Backend> {
        self.backend.as_ref()
    }

    /// The recorder, for a request that starts a recording.
    pub fn recording(&self) -> ApiResult<&Backend> {
        self.backend.as_ref().ok_or_else(|| {
            ApiError::new(
                ErrorCode::RecorderUnavailable,
                "this server has no recorder; no recording can be started",
            )
        })
    }

    /// The free disk space of the recordings. Absent on a server without a recorder and when
    /// the space cannot be read, which is logged.
    pub fn free_bytes(&self) -> Option<u64> {
        let backend = self.backend.as_ref()?;
        match backend.disk.free_bytes() {
            Ok(free) => Some(free),
            Err(error) => {
                tracing::warn!(%error, "the free disk space of the recordings cannot be read");
                None
            }
        }
    }

    /// Whether the free disk space is at or below the reserve. A change is logged, which is
    /// the warning for the administrators (ADR 0006).
    pub fn storage_low(&self, reserve: u64) -> Option<bool> {
        let low = storage::at_reserve(self.free_bytes()?, reserve);
        if self.low.swap(low, Ordering::SeqCst) != low {
            if low {
                tracing::warn!(
                    reserve,
                    "the free disk space reached the reserve: no recording starts, and the \
                     recordings that run are stopped"
                );
            } else {
                tracing::info!("the free disk space is above the reserve again");
            }
        }
        Some(low)
    }

    /// Makes the controller look at the recordings now.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    pub(crate) async fn poked(&self) {
        self.wake.notified().await;
    }

    fn tending(&self) -> MutexGuard<'_, HashMap<RecordingId, bool>> {
        self.tending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a pass over a recording. `None` while another pass over it runs: that pass
    /// may have read the room before the change that asked for this one, so the controller
    /// is woken again when it has ended.
    pub(crate) fn pass(&self, id: RecordingId) -> Option<Pass<'_>> {
        let mut tending = self.tending();
        if let Some(again) = tending.get_mut(&id) {
            *again = true;
            return None;
        }
        tending.insert(id, false);
        Some(Pass {
            recordings: self,
            id,
        })
    }

    fn layouts(&self) -> MutexGuard<'_, HashMap<RecordingId, ToldLayout>> {
        self.layouts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn told_layout(&self, id: RecordingId) -> Option<ToldLayout> {
        self.layouts().get(&id).cloned()
    }

    pub(crate) fn tell_layout(&self, id: RecordingId, told: Option<ToldLayout>) {
        let mut layouts = self.layouts();
        match told {
            Some(told) => layouts.insert(id, told),
            None => layouts.remove(&id),
        };
    }

    /// Forgets what was told to the composites of recordings that are no longer active.
    pub(crate) fn keep_layouts_of(&self, active: &[RecordingId]) {
        self.layouts().retain(|id, _| active.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_has_one_pass_at_a_time() {
        let recordings = Recordings::new(None, &RecordingLimits::default());
        let (first, second) = (RecordingId::generate(), RecordingId::generate());
        let pass = recordings.pass(first);
        assert!(pass.is_some());
        assert!(recordings.pass(first).is_none());
        assert!(recordings.pass(first).is_none());
        assert!(recordings.pass(second).is_some());
        drop(pass);
        assert!(recordings.pass(first).is_some());
        assert!(recordings.pass(first).is_some());
    }

    #[tokio::test]
    async fn a_pass_that_was_asked_for_during_another_wakes_the_controller_afterwards() {
        let recordings = Recordings::new(None, &RecordingLimits::default());
        let id = RecordingId::generate();
        let woken =
            || tokio::time::timeout(std::time::Duration::from_millis(10), recordings.poked());
        drop(recordings.pass(id));
        assert!(woken().await.is_err());

        let pass = recordings.pass(id);
        assert!(recordings.pass(id).is_none());
        assert!(woken().await.is_err());
        drop(pass);
        assert!(woken().await.is_ok());
        assert!(woken().await.is_err());
    }
}
