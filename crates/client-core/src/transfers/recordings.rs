//! The files of recordings (ADR 0608): the file of one segment, which continues after a cut
//! connection, and the transcript as a text file.
//!
//! A file of a recording has no hash. Its length is compared with the bytes that its
//! segment states. The text of a transcript is made by the server for each request and has
//! no stated length, so it is read from its start at every try and bounded by the client.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use junto_protocol::ids::{RecordingId, RecordingSegmentId};
use junto_protocol::recording::{RecordingSegment, SegmentSource};
use tokio::sync::watch;

use super::{
    DownloadProgress, DownloadShared, Length, Stored, StoredKind, TransferFailure, Transfers,
    Wanted, local_name, not_started, part_file,
};

/// The largest text of a transcript that a download saves. The server takes a transcript
/// file of at most 64 MiB from its worker (ADR 0607), and the text form holds less than
/// that file.
pub const TRANSCRIPT_TEXT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The name under which the file of a segment is saved: `stem` and the ending of what the
/// segment records, `.mp4` for the video of the composite and `.ogg` for an audio track
/// (ADR 0608). The routes of a recording know no name of a channel or of a person, so the
/// caller gives the stem. The name is reduced as the name of an attachment is.
pub fn segment_file_name(stem: &str, segment: &RecordingSegment) -> String {
    let ending = match segment.source {
        SegmentSource::Composite => "mp4",
        SegmentSource::Microphone | SegmentSource::ScreenShareAudio => "ogg",
    };
    named(stem, ending)
}

/// The name under which the text of a transcript is saved: `stem` and `.txt`.
pub fn transcript_file_name(stem: &str) -> String {
    named(stem, "txt")
}

/// One file name from a stem and an ending. A path separator in the stem is a character
/// of the name and starts no directory. A stem without a usable character is `recording`.
fn named(stem: &str, ending: &str) -> String {
    let stem: String = stem
        .chars()
        .map(|character| match character {
            '/' | '\\' => '_',
            other => other,
        })
        .collect();
    let empty = stem
        .chars()
        .all(|character| character.is_control() || matches!(character, '.' | ' '));
    let stem = if empty {
        "recording".to_owned()
    } else {
        local_name(&stem)
    };
    local_name(&format!("{stem}.{ending}"))
}

/// One download of a file of a recording. The value can be copied.
#[derive(Clone)]
pub struct RecordingDownload {
    pub recording_id: RecordingId,
    /// The segment whose file is saved. Absent for the text of the transcript.
    pub segment_id: Option<RecordingSegmentId>,
    shared: Arc<DownloadShared>,
}

impl RecordingDownload {
    pub fn progress(&self) -> DownloadProgress {
        self.shared.progress.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<DownloadProgress> {
        self.shared.progress.subscribe()
    }

    /// Ends the download and deletes the part of the file that arrived.
    pub fn cancel(&self) {
        self.shared.cancelled.send_replace(true);
    }

    /// Waits for the end of the download and returns the file.
    pub fn finished(
        &self,
    ) -> impl Future<Output = Result<PathBuf, TransferFailure>> + Send + 'static + use<> {
        super::finished(self.watch())
    }
}

impl Transfers {
    /// Starts the download of the file of one segment into `directory`. The file gets the
    /// name of [`segment_file_name`], or that name with a number when a file has it.
    /// Nothing is replaced. A download that was cut continues in its unfinished file,
    /// also after a restart of the client, when it is started with the same stem. `None`
    /// for a segment without a file.
    pub fn download_segment(
        &self,
        recording_id: RecordingId,
        segment: &RecordingSegment,
        stem: &str,
        directory: PathBuf,
    ) -> Option<RecordingDownload> {
        let bytes = segment.file.as_ref()?.bytes;
        let segment_id = segment.id;
        let Some((owner, api)) = (self.owned)() else {
            return Some(RecordingDownload {
                recording_id,
                segment_id: Some(segment_id),
                shared: not_started(bytes),
            });
        };
        let name = segment_file_name(stem, segment);
        let stored = Stored {
            owner,
            kind: StoredKind::RecordingSegment,
            id: segment_id.as_uuid(),
            bytes: Some(bytes),
            sha256: None,
        };
        let wanted = Wanted {
            part: part_file(&directory, &stored),
            name,
            length: Length::Exactly(bytes),
            sha256: None,
        };
        let open = move |from| {
            let api = api.clone();
            async move { api.segment_file(recording_id, segment_id, from).await }
        };
        Some(RecordingDownload {
            recording_id,
            segment_id: Some(segment_id),
            shared: self.start_download(wanted, directory, open),
        })
    }

    /// Starts the download of the transcript of a recording as a text file into
    /// `directory`, under the name of [`transcript_file_name`] or that name with a
    /// number. The length of the text is not known before its end: the progress states
    /// the bytes that arrived, and its `bytes` is zero until the text is complete.
    pub fn download_transcript_text(
        &self,
        recording_id: RecordingId,
        stem: &str,
        directory: PathBuf,
    ) -> RecordingDownload {
        let Some((owner, api)) = (self.owned)() else {
            return RecordingDownload {
                recording_id,
                segment_id: None,
                shared: not_started(0),
            };
        };
        let name = transcript_file_name(stem);
        let stored = Stored {
            owner,
            kind: StoredKind::TranscriptText,
            id: recording_id.as_uuid(),
            bytes: None,
            sha256: None,
        };
        let wanted = Wanted {
            part: part_file(&directory, &stored),
            name,
            length: Length::AtMost(TRANSCRIPT_TEXT_MAX_BYTES),
            sha256: None,
        };
        let open = move |_| {
            let api = api.clone();
            async move { api.transcript_text_file(recording_id).await }
        };
        RecordingDownload {
            recording_id,
            segment_id: None,
            shared: self.start_download(wanted, directory, open),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    use futures_util::StreamExt as _;
    use junto_protocol::error::ErrorCode;
    use junto_protocol::ids::{AccountId, ServerId};

    use super::super::{DownloadStage, IDLE_TRIES};
    use super::*;
    use crate::address::ServerAddress;
    use crate::api::{
        ApiClient, HttpRequest, HttpResponse, OpenFuture, OpenedResponse, RetryPolicy, Transport,
        TransportError, TransportErrorKind, TransportFuture,
    };
    use crate::book::{EntryKey, Owner};
    use crate::live::{LiveServer, Shared};
    use crate::model::recording_tests::segment;
    use crate::runtime::Runtime;
    use crate::secret::SecretToken;
    use crate::version::SupportedVersions;

    /// One answer of the played server to a request for a file.
    pub(in crate::transfers) struct Answer {
        status: u16,
        body: Vec<u8>,
        /// The connection fails after the body.
        cut: bool,
        /// The first byte of the range that the answer states.
        starts: Option<u64>,
        /// After the body no further byte arrives, and the answer does not end.
        stalls: bool,
    }

    pub(in crate::transfers) fn whole(body: &[u8]) -> Answer {
        Answer {
            status: 200,
            body: body.to_vec(),
            cut: false,
            starts: None,
            stalls: false,
        }
    }

    pub(in crate::transfers) fn stalled(body: &[u8]) -> Answer {
        Answer {
            stalls: true,
            ..whole(body)
        }
    }

    pub(in crate::transfers) fn rest(body: &[u8]) -> Answer {
        Answer {
            status: 206,
            ..whole(body)
        }
    }

    pub(in crate::transfers) fn cut(body: &[u8]) -> Answer {
        Answer {
            cut: true,
            ..whole(body)
        }
    }

    pub(in crate::transfers) fn refusal(status: u16, code: &str) -> Answer {
        Answer {
            status,
            body: format!(r#"{{"error":{{"code":"{code}","message":""}}}}"#).into_bytes(),
            cut: false,
            starts: None,
            stalls: false,
        }
    }

    /// A server that answers each request for a file with the next planned answer.
    #[derive(Default)]
    pub(in crate::transfers) struct Files {
        answers: Mutex<VecDeque<Answer>>,
        /// The path and the first byte of each request.
        asked: Mutex<Vec<(String, u64)>>,
        /// How many requests arrived that ask for no file.
        pub(in crate::transfers) sent: std::sync::atomic::AtomicUsize,
    }

    impl Transport for Files {
        fn send(&self, _request: HttpRequest) -> TransportFuture {
            self.sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Box::pin(async {
                Ok(HttpResponse {
                    status: 500,
                    retry_after: None,
                    body: Vec::new().into(),
                    receipt: crate::api::Receipt::NotStated,
                })
            })
        }

        fn open(&self, request: HttpRequest, from: u64) -> OpenFuture {
            let path = request.url.path().to_owned();
            self.asked.lock().unwrap().push((path, from));
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("an answer for the request");
            Box::pin(async move {
                let mut parts = vec![Ok(bytes::Bytes::from(answer.body))];
                if answer.cut {
                    parts.push(Err(TransportError::new(
                        TransportErrorKind::Interrupted,
                        "cut",
                    )));
                }
                let parts = futures_util::stream::iter(parts);
                Ok(OpenedResponse {
                    status: answer.status,
                    retry_after: None,
                    range_start: answer.starts,
                    body: if answer.stalls {
                        Box::pin(parts.chain(futures_util::stream::pending()))
                    } else {
                        Box::pin(parts)
                    },
                })
            })
        }
    }

    /// A saved server and an account on it that no other call of this function gives.
    pub(in crate::transfers) fn owner() -> Owner {
        Owner {
            entry: EntryKey {
                server_id: ServerId::generate(),
                generation: 1,
            },
            account_id: AccountId::generate(),
        }
    }

    fn played(answers: Vec<Answer>) -> (Arc<Files>, Transfers) {
        played_as(owner(), answers)
    }

    /// The transfers of one owner against a server that gives these answers.
    pub(in crate::transfers) fn played_as(
        owner: Owner,
        answers: Vec<Answer>,
    ) -> (Arc<Files>, Transfers) {
        let files = Arc::new(Files {
            answers: Mutex::new(answers.into()),
            asked: Mutex::default(),
            sent: Default::default(),
        });
        let api = ApiClient::new(
            files.clone(),
            ServerAddress::parse("chat.example.test").unwrap(),
            ServerId::generate(),
            SupportedVersions::THIS_BUILD.negotiate(&[1]).unwrap(),
            RetryPolicy::default(),
        )
        .with_token(SecretToken::new("jst_test".to_owned()));
        let pinned = api;
        let transfers = Transfers {
            live: LiveServer {
                shared: Shared::new(),
            },
            runtime: Runtime::current_or_new().unwrap(),
            owned: Arc::new(move || Some((owner, pinned.clone()))),
        };
        (files, transfers)
    }

    /// The name of the unfinished file of a segment that is saved as an audio track.
    fn part_name(transfers: &Transfers, segment: &RecordingSegment) -> String {
        let (owner, _) = (transfers.owned)().unwrap();
        let stored = Stored {
            owner,
            kind: StoredKind::RecordingSegment,
            id: segment.id.as_uuid(),
            bytes: segment.file.as_ref().map(|file| file.bytes),
            sha256: None,
        };
        part_file(Path::new(""), &stored)
            .to_string_lossy()
            .into_owned()
    }

    pub(in crate::transfers) fn starts(files: &Files) -> Vec<u64> {
        files
            .asked
            .lock()
            .unwrap()
            .iter()
            .map(|(_, from)| *from)
            .collect()
    }

    pub(in crate::transfers) fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// An audio track whose file has the bytes of `content`.
    fn track(content: &[u8]) -> RecordingSegment {
        segment(SegmentSource::Microphone, content.len() as u64)
    }

    #[test]
    fn a_file_of_a_recording_is_named_by_the_caller_and_by_what_it_holds() {
        let video = segment(SegmentSource::Composite, 1);
        let audio = segment(SegmentSource::Microphone, 1);
        let shared = segment(SegmentSource::ScreenShareAudio, 1);
        assert_eq!(
            segment_file_name("Daily 2030-01-02", &video),
            "Daily 2030-01-02.mp4"
        );
        assert_eq!(segment_file_name("Daily, Ada", &audio), "Daily, Ada.ogg");
        assert_eq!(
            segment_file_name("Daily, shared", &shared),
            "Daily, shared.ogg"
        );
        assert_eq!(
            transcript_file_name("Daily 2030-01-02"),
            "Daily 2030-01-02.txt"
        );
        // The stem comes from names that other accounts chose: it is one file name.
        assert_eq!(segment_file_name("../../a/b", &audio), "_.._a_b.ogg");
        assert_eq!(segment_file_name("x\u{202e}3pm", &video), "x_3pm.mp4");
        assert_eq!(segment_file_name("con", &video), "_con.mp4");
        assert_eq!(segment_file_name(" . ", &video), "recording.mp4");
        assert_eq!(transcript_file_name(""), "recording.txt");
        let long = segment_file_name(&"n".repeat(300), &video);
        assert_eq!(long.chars().count(), 200);
        assert!(long.ends_with(".mp4"));
    }

    #[tokio::test]
    async fn a_cut_download_continues_after_a_rename_of_its_channel() {
        let content = b"abcdefghij";
        let segment = track(content);
        let recording_id = RecordingId::generate();
        let directory = tempfile::tempdir().unwrap();
        // A download with the stem "Daily" left its first bytes. The channel was renamed,
        // and the next download has another stem.
        let (files, transfers) = played(vec![rest(&content[4..])]);
        let part = part_name(&transfers, &segment);
        assert!(!part.contains("Daily"));
        std::fs::write(directory.path().join(&part), &content[..4]).unwrap();

        let download = transfers
            .download_segment(recording_id, &segment, "Weekly", directory.path().into())
            .unwrap();
        let saved = download.finished().await.unwrap();
        assert_eq!(saved, directory.path().join("Weekly.ogg"));
        assert_eq!(std::fs::read(&saved).unwrap(), content);
        assert_eq!(starts(&files), [4]);
        assert_eq!(names(directory.path()), ["Weekly.ogg"]);
    }

    #[tokio::test]
    async fn a_segment_continues_its_part_file_and_gets_a_name_that_no_file_has() {
        let content = b"abcdefghij";
        let segment = track(content);
        let recording_id = RecordingId::generate();
        let directory = tempfile::tempdir().unwrap();
        // A file of the person has the name, and an earlier download left its first bytes.
        std::fs::write(directory.path().join("Daily.ogg"), b"mine").unwrap();
        let (files, transfers) = played(vec![rest(&content[4..])]);
        let part = part_name(&transfers, &segment);
        std::fs::write(directory.path().join(&part), &content[..4]).unwrap();

        let download = transfers
            .download_segment(recording_id, &segment, "Daily", directory.path().into())
            .unwrap();
        assert_eq!(download.segment_id, Some(segment.id));
        let saved = download.finished().await.unwrap();
        assert_eq!(saved, directory.path().join("Daily (1).ogg"));
        assert_eq!(std::fs::read(&saved).unwrap(), content);
        assert_eq!(
            std::fs::read(directory.path().join("Daily.ogg")).unwrap(),
            b"mine"
        );
        assert_eq!(names(directory.path()), ["Daily (1).ogg", "Daily.ogg"]);
        let asked = files.asked.lock().unwrap().clone();
        let path = format!(
            "/api/v1/recordings/{recording_id}/segments/{}/file",
            segment.id
        );
        assert_eq!(asked, [(path, 4)]);
        let progress = download.progress();
        assert_eq!(progress.stage, DownloadStage::Saved(saved));
        assert_eq!((progress.received, progress.bytes), (10, 10));

        // A segment without a file has nothing to download.
        let mut failed = segment.clone();
        failed.file = None;
        let none =
            transfers.download_segment(recording_id, &failed, "Daily", directory.path().into());
        assert!(none.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_server_and_a_cut_connection_are_waited_out() {
        let content = b"abcdefghij";
        let segment = track(content);
        let directory = tempfile::tempdir().unwrap();
        let (files, transfers) = played(vec![
            refusal(503, "busy"),
            cut(&content[..3]),
            rest(&content[3..]),
        ]);
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        let saved = download.finished().await.unwrap();
        assert_eq!(std::fs::read(saved).unwrap(), content);
        // The request after the cut asks for the bytes that the file does not hold.
        assert_eq!(starts(&files), [0, 0, 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_of_another_length_than_its_segment_states_is_not_saved() {
        let content = b"abcdefghij";
        let segment = track(content);
        let stated = |transfers: &Transfers, directory: &Path| {
            transfers
                .download_segment(RecordingId::generate(), &segment, "Daily", directory.into())
                .unwrap()
        };

        // More bytes than the segment states.
        let directory = tempfile::tempdir().unwrap();
        let (_, transfers) = played(vec![whole(b"abcdefghijklm")]);
        let failure = stated(&transfers, directory.path()).finished().await;
        assert_eq!(failure, Err(TransferFailure::Corrupt));
        assert!(names(directory.path()).is_empty());

        // Fewer bytes: the server has no byte after them, and says so.
        let directory = tempfile::tempdir().unwrap();
        let (files, transfers) = played(vec![
            whole(&content[..6]),
            refusal(416, "range_not_satisfiable"),
        ]);
        let download = stated(&transfers, directory.path());
        let refused = TransferFailure::Refused {
            code: ErrorCode::RangeNotSatisfiable,
            status: 416,
        };
        assert_eq!(download.finished().await, Err(refused));
        assert_eq!(download.progress().stage, DownloadStage::Failed(refused));
        assert_eq!(starts(&files), [0, 6]);
        // No file has the name of a saved file, and the unfinished file is deleted: the
        // server has no byte that continues it.
        assert!(names(directory.path()).is_empty());
    }

    #[tokio::test]
    async fn a_part_of_the_file_that_was_not_asked_for_is_not_added() {
        let content = b"abcdefghij";
        let segment = track(content);
        let directory = tempfile::tempdir().unwrap();
        // The client asks for the bytes from the fifth one on. The answer states a range
        // that starts at the third byte, with as many bytes as are missing.
        let (files, transfers) = played(vec![Answer {
            starts: Some(2),
            ..rest(&content[2..8])
        }]);
        let part = part_name(&transfers, &segment);
        std::fs::write(directory.path().join(&part), &content[..4]).unwrap();
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        assert_eq!(download.finished().await, Err(TransferFailure::Corrupt));
        assert_eq!(starts(&files), [4]);
        assert!(names(directory.path()).is_empty());
    }

    #[test]
    fn the_start_of_a_range_is_read_from_its_header() {
        use crate::api::transport::range_start_of;
        assert_eq!(range_start_of("bytes 100-199/200"), Some(100));
        assert_eq!(range_start_of("bytes 0-9/*"), Some(0));
        assert_eq!(range_start_of("bytes */200"), None);
        assert_eq!(range_start_of("items 1-2/3"), None);
    }

    #[tokio::test]
    async fn a_refusal_ends_the_download_with_its_reason() {
        let segment = track(b"abc");
        let directory = tempfile::tempdir().unwrap();
        // An earlier download left its first byte.
        let (_, transfers) = played(vec![refusal(404, "not_found")]);
        let part = part_name(&transfers, &segment);
        std::fs::write(directory.path().join(&part), b"a").unwrap();
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        let refused = TransferFailure::Refused {
            code: ErrorCode::NotFound,
            status: 404,
        };
        assert_eq!(download.finished().await, Err(refused));
        // The server has no such file any more: the unfinished file is deleted.
        assert!(names(directory.path()).is_empty());

        // A refusal that can end, such as a missing permission, keeps the unfinished file.
        let (_, transfers) = played(vec![refusal(403, "forbidden")]);
        let part = part_name(&transfers, &segment);
        std::fs::write(directory.path().join(&part), b"a").unwrap();
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        assert!(download.finished().await.is_err());
        assert_eq!(names(directory.path()), [part]);
    }

    #[tokio::test(start_paused = true)]
    async fn answers_that_never_complete_a_file_end_its_download() {
        // A server that answers each request without a byte.
        let segment = track(b"abcdefghij");
        let directory = tempfile::tempdir().unwrap();
        let empty = (0..IDLE_TRIES).map(|_| whole(b"")).collect();
        let (files, transfers) = played(empty);
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        assert_eq!(download.finished().await, Err(TransferFailure::Interrupted));
        assert_eq!(starts(&files).len(), IDLE_TRIES as usize);

        // A server that answers each request with the start of the file, and never with
        // a part of it: each answer replaces what was held, and none passes the bytes
        // that an earlier one reached.
        let starts_again = (0..=IDLE_TRIES).map(|_| whole(b"abc")).collect();
        let (files, transfers) = played(starts_again);
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(3_600), download.finished())
            .await
            .expect("the download ends");
        assert_eq!(ended, Err(TransferFailure::Interrupted));
        assert_eq!(starts(&files).len(), IDLE_TRIES as usize + 1);

        // The text of a transcript whose answer is cut at every try.
        let cuts = (0..IDLE_TRIES).map(|_| cut(b"[00:00:01] Ada")).collect();
        let (files, transfers) = played(cuts);
        let download = transfers.download_transcript_text(
            RecordingId::generate(),
            "Daily",
            directory.path().into(),
        );
        assert_eq!(download.finished().await, Err(TransferFailure::Interrupted));
        assert_eq!(starts(&files).len(), IDLE_TRIES as usize);
        assert!(names(directory.path()).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_at_the_place_of_the_part_file_is_refused_and_its_target_stays() {
        let segment = track(b"abc");
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("mine.txt");
        std::fs::write(&target, b"mine").unwrap();
        let (files, transfers) = played(Vec::new());
        let part = part_name(&transfers, &segment);
        std::os::unix::fs::symlink(&target, directory.path().join(part)).unwrap();
        let download = transfers
            .download_segment(
                RecordingId::generate(),
                &segment,
                "Daily",
                directory.path().into(),
            )
            .unwrap();
        let failure = download.finished().await;
        assert!(
            matches!(failure, Err(TransferFailure::File(_))),
            "{failure:?}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"mine");
        assert!(starts(&files).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_text_of_a_transcript_is_saved_as_a_file() {
        let text = "[00:00:01] Ada: Good morning.\n[00:00:03] Ben: Can you hear me?\n";
        let recording_id = RecordingId::generate();
        let directory = tempfile::tempdir().unwrap();
        // The first answer is cut. The text is made for each request: it is read from its
        // start again, and what arrived before is not kept.
        let (files, transfers) = played(vec![
            cut(&text.as_bytes()[..9]),
            whole(text.as_bytes()),
            whole(text.as_bytes()),
        ]);
        let download =
            transfers.download_transcript_text(recording_id, "Daily", directory.path().into());
        assert_eq!(
            (download.recording_id, download.segment_id),
            (recording_id, None)
        );
        let saved = download.finished().await.unwrap();
        assert_eq!(saved, directory.path().join("Daily.txt"));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), text);
        let progress = download.progress();
        let length = text.len() as u64;
        assert_eq!((progress.received, progress.bytes), (length, length));

        // A second download does not replace the first file.
        let again =
            transfers.download_transcript_text(recording_id, "Daily", directory.path().into());
        let second = again.finished().await.unwrap();
        assert_eq!(second, directory.path().join("Daily (1).txt"));
        assert_eq!(names(directory.path()), ["Daily (1).txt", "Daily.txt"]);
        let path = format!("/api/v1/recordings/{recording_id}/transcript/text");
        let asked = files.asked.lock().unwrap().clone();
        assert_eq!(asked, [(path.clone(), 0), (path.clone(), 0), (path, 0)]);
    }

    #[tokio::test]
    async fn a_text_above_the_bound_is_not_saved() {
        let directory = tempfile::tempdir().unwrap();
        let (_, transfers) = played(vec![whole(b"abcdef")]);
        let (_, api) = (transfers.owned)().unwrap();
        let recording_id = RecordingId::generate();
        let wanted = super::super::Wanted {
            name: "Daily.txt".to_owned(),
            part: directory.path().join("Daily.txt.0123.part"),
            length: super::super::Length::AtMost(5),
            sha256: None,
        };
        let open = move |_| {
            let api = api.clone();
            async move { api.transcript_text_file(recording_id).await }
        };
        let shared = transfers.start_download(wanted, directory.path().into(), open);
        let failure = super::super::finished(shared.progress.subscribe()).await;
        assert_eq!(failure, Err(TransferFailure::Corrupt));
        assert!(names(directory.path()).is_empty());
    }
}
