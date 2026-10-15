//! The routes of recordings and of their transcripts (ADR 0604, ADR 0607, ADR 0608). The
//! start and the stop of a recording belong to its call and are with the call routes.

use junto_protocol::ids::{RecordingId, RecordingSegmentId, WorkspaceId};
use junto_protocol::recording::{Recording, RecordingDetail, RecordingPage};
use junto_protocol::transcript::{
    CorrectLine, RenameSpeaker, Transcript, TranscriptLine, TranscriptLinePage,
    TranscriptLineQuery, TranscriptSpeaker,
};

use super::client::read_json;
use super::error::ApiError;
use super::messages::FileBody;
use super::session::ServerApi;
use super::transport::Method;

fn recording_of(recording_id: RecordingId) -> String {
    format!("/recordings/{recording_id}")
}

fn transcript_of(recording_id: RecordingId) -> String {
    format!("/recordings/{recording_id}/transcript")
}

impl ServerApi {
    // Recordings.

    /// One page of the recordings of a workspace that the caller may read, the newest
    /// first. `before` is the last recording of the page before. `limit`: from 1 to 200;
    /// the server uses 50 without it.
    pub async fn recordings(
        &self,
        workspace_id: WorkspaceId,
        before: Option<RecordingId>,
        limit: Option<u32>,
    ) -> Result<RecordingPage, ApiError> {
        let mut query = Vec::new();
        if let Some(before) = before {
            query.push(("before", before.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        let path = format!("/workspaces/{workspace_id}/recordings");
        read_json(&self.call(Method::Get, path, query, None).await?)
    }

    /// One recording with its segments, in the order in which they were asked for.
    pub async fn recording(&self, recording_id: RecordingId) -> Result<RecordingDetail, ApiError> {
        self.get(recording_of(recording_id)).await
    }

    /// Removes a recording with its files and its transcript. The caller needs "manage
    /// recordings" in the voice channel. A recording that runs or whose files are
    /// processed answers with a conflict (ADR 0608).
    pub async fn remove_recording(&self, recording_id: RecordingId) -> Result<(), ApiError> {
        self.act(Method::Delete, recording_of(recording_id)).await
    }

    /// The file of one segment from the byte `from` on: the video of the composite as
    /// MP4, or one audio track as Ogg. A segment without a file answers `not_found`.
    pub async fn segment_file(
        &self,
        recording_id: RecordingId,
        segment_id: RecordingSegmentId,
        from: u64,
    ) -> Result<FileBody, ApiError> {
        let path = format!("{}/segments/{segment_id}/file", recording_of(recording_id));
        let opened = self.open(path, from).await?;
        Ok(FileBody {
            partial: opened.status == 206,
            starts_at: opened.range_start,
            body: opened.body,
        })
    }

    // Transcripts.

    /// The transcript of a recording without its lines: its state, its speaker labels
    /// with their names, the glossary terms that were used, and the result for each audio
    /// segment.
    pub async fn transcript(&self, recording_id: RecordingId) -> Result<Transcript, ApiError> {
        self.get(transcript_of(recording_id)).await
    }

    /// Asks for the transcript of a recording again. The caller needs "manage
    /// recordings". The server accepts this for a recording with files that has no
    /// transcript or a failed one (ADR 0607).
    pub async fn request_transcript(
        &self,
        recording_id: RecordingId,
    ) -> Result<Recording, ApiError> {
        let path = transcript_of(recording_id);
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    /// One page of the lines of a transcript in the order of the timeline. `after` is the
    /// position of the last line of the page before. `limit`: from 1 to 500; the server
    /// uses 200 without it.
    pub async fn transcript_lines(
        &self,
        recording_id: RecordingId,
        after: Option<u32>,
        limit: Option<u32>,
    ) -> Result<TranscriptLinePage, ApiError> {
        let query = TranscriptLineQuery {
            after,
            limit,
            ..TranscriptLineQuery::default()
        };
        self.transcript_lines_at(recording_id, &query).await
    }

    /// A page of lines of a transcript: after a line, before a line, or with a line in
    /// its middle (ADR 0607). The page states whether lines follow and come before it.
    pub async fn transcript_lines_at(
        &self,
        recording_id: RecordingId,
        query: &TranscriptLineQuery,
    ) -> Result<TranscriptLinePage, ApiError> {
        let mut pairs = Vec::new();
        for (name, value) in [
            ("after", query.after),
            ("before", query.before),
            ("around", query.around),
            ("limit", query.limit),
        ] {
            if let Some(value) = value {
                pairs.push((name, value.to_string()));
            }
        }
        let path = format!("{}/lines", transcript_of(recording_id));
        read_json(&self.call(Method::Get, path, pairs, None).await?)
    }

    /// The transcript as plain text: one line of text for each line, with its time, its
    /// speaker and its newest correction. A text above the largest response that the
    /// client reads is refused; [`ServerApi::transcript_text_file`] has no such bound.
    pub async fn transcript_text(&self, recording_id: RecordingId) -> Result<String, ApiError> {
        let path = format!("{}/text", transcript_of(recording_id));
        let response = self.call(Method::Get, path, Vec::new(), None).await?;
        std::str::from_utf8(response.body.as_slice())
            .map(str::to_owned)
            .map_err(|_| ApiError::InvalidResponse {
                status: response.status,
                reason: "a transcript that is no text",
            })
    }

    /// The transcript as plain text, as a body that is read in parts. The server makes
    /// the text for each request and serves no range of it.
    pub async fn transcript_text_file(
        &self,
        recording_id: RecordingId,
    ) -> Result<FileBody, ApiError> {
        let path = format!("{}/text", transcript_of(recording_id));
        let opened = self.open(path, 0).await?;
        Ok(FileBody {
            partial: false,
            starts_at: None,
            body: opened.body,
        })
    }

    /// Corrects one line of a transcript. Every account that may read the recording can
    /// do this. The text has 1 to 2000 characters. The answer carries the text that the
    /// worker wrote as `original_text`.
    pub async fn correct_transcript_line(
        &self,
        recording_id: RecordingId,
        line: u32,
        text: &str,
    ) -> Result<TranscriptLine, ApiError> {
        let path = format!("{}/lines/{line}", transcript_of(recording_id));
        let body = CorrectLine {
            text: text.to_owned(),
        };
        self.send(Method::Put, path, &body).await
    }

    /// Gives a name to the voice with the label `Speaker <number>` in one recording.
    pub async fn rename_transcript_speaker(
        &self,
        recording_id: RecordingId,
        number: u32,
        name: &str,
    ) -> Result<TranscriptSpeaker, ApiError> {
        let path = format!("{}/speakers/{number}", transcript_of(recording_id));
        let body = RenameSpeaker {
            name: name.to_owned(),
        };
        self.send(Method::Put, path, &body).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use futures_util::StreamExt as _;
    use junto_protocol::ids::{ChannelId, ServerId};
    use junto_protocol::recording::SegmentSource;

    use super::*;
    use crate::address::ServerAddress;
    use crate::api::transport::{
        HttpRequest, HttpResponse, OpenFuture, OpenedResponse, Transport, TransportFuture,
    };
    use crate::api::{ApiClient, ErrorCategory, RetryPolicy};
    use crate::model::recording_tests::{detail, line, recording, speaker, transcript};
    use crate::secret::SecretToken;
    use crate::version::SupportedVersions;

    /// A request as the played server saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Asked {
        method: Method,
        path: String,
        query: Option<String>,
        body: String,
        /// The byte that a request for a file starts at.
        from: Option<u64>,
    }

    /// Answers each request with the next fixed answer.
    #[derive(Default)]
    struct Fixed {
        answers: Mutex<VecDeque<(u16, Vec<u8>)>>,
        asked: Mutex<Vec<Asked>>,
    }

    impl Fixed {
        fn take(&self, request: &HttpRequest, from: Option<u64>) -> (u16, Vec<u8>) {
            self.asked.lock().unwrap().push(Asked {
                method: request.method,
                path: request.url.path().to_owned(),
                query: request.url.query().map(str::to_owned),
                body: request
                    .body
                    .as_ref()
                    .map(|body| String::from_utf8_lossy(body.as_slice()).into_owned())
                    .unwrap_or_default(),
                from,
            });
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("an answer for the request")
        }
    }

    impl Transport for Fixed {
        fn send(&self, request: HttpRequest) -> TransportFuture {
            let (status, body) = self.take(&request, None);
            Box::pin(async move {
                Ok(HttpResponse {
                    status,
                    retry_after: None,
                    body: body.into(),
                    receipt: crate::api::Receipt::NotStated,
                })
            })
        }

        fn open(&self, request: HttpRequest, from: u64) -> OpenFuture {
            let (status, body) = self.take(&request, Some(from));
            Box::pin(async move {
                let body = bytes::Bytes::from(body);
                Ok(OpenedResponse {
                    status,
                    retry_after: None,
                    range_start: None,
                    body: Box::pin(futures_util::stream::once(async move { Ok(body) })),
                })
            })
        }
    }

    fn played(answers: Vec<(u16, Vec<u8>)>) -> (Arc<Fixed>, ServerApi) {
        let fixed = Arc::new(Fixed {
            answers: Mutex::new(answers.into()),
            asked: Mutex::default(),
        });
        let api = ApiClient::new(
            fixed.clone(),
            ServerAddress::parse("chat.example.test").unwrap(),
            ServerId::generate(),
            SupportedVersions::THIS_BUILD.negotiate(&[1]).unwrap(),
            RetryPolicy::default(),
        )
        .with_token(SecretToken::new("jst_test".to_owned()));
        (fixed, api)
    }

    fn json<T: serde::Serialize>(body: &T) -> (u16, Vec<u8>) {
        (200, serde_json::to_vec(body).unwrap())
    }

    fn asked(fixed: &Fixed) -> Vec<Asked> {
        std::mem::take(&mut fixed.asked.lock().unwrap())
    }

    fn get(path: String, query: Option<&str>) -> Asked {
        Asked {
            method: Method::Get,
            path,
            query: query.map(str::to_owned),
            body: String::new(),
            from: None,
        }
    }

    #[tokio::test]
    async fn the_recordings_of_a_workspace_are_read_in_pages() {
        let (workspace_id, before) = (WorkspaceId::generate(), RecordingId::generate());
        let held = recording(RecordingId::generate(), ChannelId::generate(), 0);
        let page = RecordingPage {
            recordings: vec![held.clone()],
            next_before: Some(held.id),
        };
        let (fixed, api) = played(vec![json(&page), json(&page)]);
        let path = format!("/api/v1/workspaces/{workspace_id}/recordings");

        assert_eq!(
            api.recordings(workspace_id, None, None).await.unwrap(),
            page
        );
        let read = api.recordings(workspace_id, Some(before), Some(20)).await;
        assert_eq!(read.unwrap().next_before, Some(held.id));
        assert_eq!(
            asked(&fixed),
            [
                get(path.clone(), None),
                get(path, Some(&format!("before={before}&limit=20"))),
            ]
        );
    }

    #[tokio::test]
    async fn a_recording_is_read_with_its_segments_and_removed() {
        let id = RecordingId::generate();
        let held = detail(recording(id, ChannelId::generate(), 0));
        let refusal = br#"{"error":{"code":"conflict","message":"it runs"}}"#.to_vec();
        let (fixed, api) = played(vec![json(&held), (204, Vec::new()), (409, refusal)]);
        let path = format!("/api/v1/recordings/{id}");

        let read = api.recording(id).await.unwrap();
        assert_eq!(read, held);
        assert_eq!(read.segments[0].source, SegmentSource::Composite);
        api.remove_recording(id).await.unwrap();
        // A recording that runs is not removed (ADR 0608).
        let refused = api.remove_recording(id).await.unwrap_err();
        assert_eq!(refused.category(), Some(ErrorCategory::Conflict));
        let delete = Asked {
            method: Method::Delete,
            ..get(path.clone(), None)
        };
        assert_eq!(asked(&fixed), [get(path, None), delete.clone(), delete]);
    }

    #[tokio::test]
    async fn the_transcript_its_lines_and_its_text_are_read() {
        let id = RecordingId::generate();
        let held = recording(id, ChannelId::generate(), 2);
        let mut header = transcript(&held);
        header.speakers = vec![speaker(1, Some("Noor"))];
        let page = TranscriptLinePage {
            lines: vec![line(5, "Good morning."), line(6, "Can you hear me?")],
            next_after: Some(6),
            previous_before: None,
        };
        let text = "[00:00:05] Ada: Good morning.\n";
        let (fixed, api) = played(vec![
            json(&header),
            json(&page),
            json(&page),
            (200, text.as_bytes().to_vec()),
            (200, vec![0xff, 0xfe]),
            (200, text.as_bytes().to_vec()),
        ]);
        let path = format!("/api/v1/recordings/{id}/transcript");

        assert_eq!(api.transcript(id).await.unwrap(), header);
        assert_eq!(api.transcript_lines(id, None, None).await.unwrap(), page);
        let read = api.transcript_lines(id, Some(4), Some(500)).await.unwrap();
        assert_eq!(read.next_after, Some(6));
        assert_eq!(api.transcript_text(id).await.unwrap(), text);
        // A body that is no text is no transcript.
        assert!(matches!(
            api.transcript_text(id).await,
            Err(ApiError::InvalidResponse { status: 200, .. })
        ));
        let mut body = api.transcript_text_file(id).await.unwrap().body;
        assert_eq!(body.next().await.unwrap().unwrap(), text.as_bytes());
        let lines = format!("{path}/lines");
        let whole = format!("{path}/text");
        assert_eq!(
            asked(&fixed),
            [
                get(path, None),
                get(lines.clone(), None),
                get(lines, Some("after=4&limit=500")),
                get(whole.clone(), None),
                get(whole.clone(), None),
                Asked {
                    from: Some(0),
                    ..get(whole, None)
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_correction_a_rename_and_a_new_request_are_sent_to_their_routes() {
        let id = RecordingId::generate();
        let held = recording(id, ChannelId::generate(), 2);
        let mut corrected = line(4, "eleven seconds");
        corrected.original_text = Some("a leaven seconds".to_owned());
        let renamed = speaker(2, Some("Noor"));
        let (fixed, api) = played(vec![json(&corrected), json(&renamed), json(&held)]);
        let path = format!("/api/v1/recordings/{id}/transcript");

        let read = api.correct_transcript_line(id, 4, "eleven seconds").await;
        assert_eq!(read.unwrap(), corrected);
        let read = api.rename_transcript_speaker(id, 2, "Noor").await;
        assert_eq!(read.unwrap(), renamed);
        assert_eq!(api.request_transcript(id).await.unwrap(), held);
        let sent = |method, path: String, body: &str| Asked {
            method,
            body: body.to_owned(),
            ..get(path, None)
        };
        assert_eq!(
            asked(&fixed),
            [
                sent(
                    Method::Put,
                    format!("{path}/lines/4"),
                    r#"{"text":"eleven seconds"}"#
                ),
                sent(
                    Method::Put,
                    format!("{path}/speakers/2"),
                    r#"{"name":"Noor"}"#
                ),
                sent(Method::Post, path, ""),
            ]
        );
    }

    #[tokio::test]
    async fn the_file_of_a_segment_is_asked_for_from_a_byte() {
        let (id, segment_id) = (RecordingId::generate(), RecordingSegmentId::generate());
        let gone = br#"{"error":{"code":"not_found","message":"no file"}}"#.to_vec();
        let (fixed, api) = played(vec![
            (200, b"whole".to_vec()),
            (206, b"le".to_vec()),
            (404, gone),
        ]);
        let path = format!("/api/v1/recordings/{id}/segments/{segment_id}/file");

        let whole = api.segment_file(id, segment_id, 0).await.unwrap();
        assert!(!whole.partial);
        let mut part = api.segment_file(id, segment_id, 3).await.unwrap();
        assert!(part.partial);
        assert_eq!(part.body.next().await.unwrap().unwrap(), &b"le"[..]);
        // A segment without a file, and a recording that the account may not read.
        let Err(refused) = api.segment_file(id, segment_id, 0).await else {
            panic!("a file that the server does not have was opened");
        };
        assert_eq!(refused.category(), Some(ErrorCategory::NotFound));
        let from = |from| Asked {
            from: Some(from),
            ..get(path.clone(), None)
        };
        assert_eq!(asked(&fixed), [from(0), from(3), from(0)]);
    }
}
