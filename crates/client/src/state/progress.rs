//! What the window takes of the progress of a transfer. A transfer tells its progress at
//! each part that arrives or leaves, and each step that the window takes derives its view
//! again on the thread of the window. The window takes a change of the stage, of the
//! interruption and each further hundredth of the file (ADR 0744).

use junto_client_core::transfers::{
    Download, DownloadProgress, DownloadStage, RecordingDownload, Upload, UploadProgress,
    UploadStage,
};
use tokio::sync::watch;

/// A file without a stated length counts parts of this many bytes.
const UNSTATED_PART: u64 = 256 * 1024;

/// One step of a progress as the window shows it: the stage, the interruption, and the
/// hundredth of the file that is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Step {
    stage: u8,
    interrupted: bool,
    part: u64,
}

/// A progress that the window shows in steps.
pub(super) trait Stepped: Clone {
    fn step(&self) -> Step;
    /// The transfer has ended: no later progress follows.
    fn ended(&self) -> bool;
}

fn part(done: u64, bytes: u64) -> u64 {
    done.saturating_mul(100)
        .checked_div(bytes)
        .unwrap_or(done / UNSTATED_PART)
}

impl Stepped for DownloadProgress {
    fn step(&self) -> Step {
        let stage = match self.stage {
            DownloadStage::Receiving => 0,
            DownloadStage::Checking => 1,
            DownloadStage::Saved(_) => 2,
            DownloadStage::Failed(_) => 3,
            DownloadStage::Cancelled => 4,
        };
        Step {
            stage,
            interrupted: self.interrupted,
            part: part(self.received, self.bytes),
        }
    }

    fn ended(&self) -> bool {
        matches!(
            self.stage,
            DownloadStage::Saved(_) | DownloadStage::Failed(_) | DownloadStage::Cancelled
        )
    }
}

impl Stepped for UploadProgress {
    fn step(&self) -> Step {
        let stage = match self.stage {
            UploadStage::Reading => 0,
            UploadStage::Sending => 1,
            UploadStage::Checking => 2,
            UploadStage::Complete(_) => 3,
            UploadStage::Failed(_) => 4,
            UploadStage::Cancelled => 5,
        };
        Step {
            stage,
            interrupted: self.interrupted,
            part: part(self.sent, self.bytes),
        }
    }

    fn ended(&self) -> bool {
        matches!(
            self.stage,
            UploadStage::Complete(_) | UploadStage::Failed(_) | UploadStage::Cancelled
        )
    }
}

/// The steps of one transfer, read from its progress.
pub(super) struct Steps<T> {
    watch: watch::Receiver<T>,
    told: Option<Step>,
    ended: bool,
}

impl Steps<DownloadProgress> {
    /// The only reader of the progress of a download of a file of a message in the window.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn of_download(download: &Download) -> Self {
        Self::new(download.watch())
    }

    /// The only reader of the progress of a download of a file of a recording in the window.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn of_recording(download: &RecordingDownload) -> Self {
        Self::new(download.watch())
    }
}

impl Steps<UploadProgress> {
    /// The only reader of the progress of an upload in the window.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn of_upload(upload: &Upload) -> Self {
        Self::new(upload.watch())
    }
}

impl<T: Stepped> Steps<T> {
    fn new(watch: watch::Receiver<T>) -> Self {
        Self {
            watch,
            told: None,
            ended: false,
        }
    }

    /// The next progress whose step differs from the one told before; the first call gives
    /// the progress of this moment, since a transfer can end before its reader runs.
    /// Absent after the progress that ended the transfer, and when the transfer is gone.
    pub(super) async fn next(&mut self) -> Option<T> {
        if self.ended {
            return None;
        }
        loop {
            let progress = self.watch.borrow_and_update().clone();
            let step = progress.step();
            if self.told != Some(step) {
                self.told = Some(step);
                self.ended = progress.ended();
                return Some(progress);
            }
            if self.watch.changed().await.is_err() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_download_is_told_at_each_hundredth_and_at_its_end() {
        let bytes = 1_000_000;
        let (sender, receiver) = watch::channel(DownloadProgress {
            stage: DownloadStage::Receiving,
            received: 0,
            bytes,
            interrupted: false,
        });
        let mut steps = Steps::new(receiver);
        let reader = tokio::spawn(async move {
            let mut told = Vec::new();
            while let Some(progress) = steps.next().await {
                told.push(progress);
            }
            told
        });
        // The reader reads the start, and then a part of 1,000 bytes arrives a thousand
        // times, and the reader sees each.
        tokio::task::yield_now().await;
        for received in (1_000..=bytes).step_by(1_000) {
            sender.send_modify(|progress| progress.received = received);
            tokio::task::yield_now().await;
        }
        sender.send_modify(|progress| progress.interrupted = true);
        tokio::task::yield_now().await;
        sender.send_modify(|progress| {
            progress.interrupted = false;
            progress.stage = DownloadStage::Saved("saved".into());
        });
        let told = reader.await.unwrap_or_default();
        // The start, each hundredth, the interruption, its end with the saved file.
        let received: Vec<u64> = told.iter().map(|progress| progress.received).collect();
        let hundredths: Vec<u64> = (0..=100).map(|part| part * 10_000).collect();
        assert_eq!(received[..101], hundredths[..]);
        assert_eq!(told.len(), 103);
        assert!(told[101].interrupted);
        assert_eq!(told[102].stage, DownloadStage::Saved("saved".into()));
        drop(sender);
    }

    #[tokio::test]
    async fn a_transfer_that_ended_before_its_reader_ran_is_told_once() {
        let (sender, receiver) = watch::channel(UploadProgress {
            stage: UploadStage::Cancelled,
            sent: 10,
            bytes: 100,
            interrupted: false,
        });
        let mut steps = Steps::new(receiver);
        assert_eq!(
            steps.next().await.map(|progress| progress.stage),
            Some(UploadStage::Cancelled)
        );
        assert_eq!(steps.next().await, None);
        drop(sender);
    }
}
