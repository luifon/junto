//! Recordings of calls: the server's record of each recording and its segments, and the
//! controller that makes the recorder agree with it (ADR 0601, ADR 0604).

pub mod control;
pub mod store;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::RecordingId;
use tokio::sync::Notify;

use crate::config::RecorderSettings;
use crate::error::{ApiError, ApiResult};
use crate::recorder::Recorder;
use crate::storage::{self, Disk};

/// What a server with a recorder has: the recorder, the disk that it writes to, and where it
/// writes.
#[derive(Clone)]
pub struct Backend {
    pub recorder: Arc<dyn Recorder>,
    pub disk: Arc<dyn Disk>,
    pub settings: RecorderSettings,
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
    pub fn new(backend: Option<Backend>) -> Self {
        Self {
            backend,
            wake: Notify::new(),
            tending: Mutex::new(HashMap::new()),
            layouts: Mutex::new(HashMap::new()),
            low: AtomicBool::new(false),
        }
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
        let recordings = Recordings::new(None);
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
        let recordings = Recordings::new(None);
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
