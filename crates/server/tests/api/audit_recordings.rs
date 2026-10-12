//! Proofs of the audit of the whole server: the downloads of recording files and of
//! attachments, the saved audio of lost jobs that no record names, and the note of a
//! failure of a recording. Each test fails while its defect exists.

use std::path::{Path, PathBuf};

use junto_protocol::WorkspacePermission as W;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{MessageId, RecordingId, RecordingSegmentId};
use junto_protocol::message::Message;
use junto_protocol::recording::{RecordingState, SegmentFailure, SegmentSource, SegmentState};
use junto_protocol::structure::Channel;
use junto_server::recordings::process::RESCUED_AT_ONCE;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::Scene;
use crate::messages_support::{channel, upload};
use crate::recordings_support::{Studio, composites, parts};
use crate::support::{Member, Scratch, TestServer};

/// A file that is larger than what a connection holds unsent, so that the server still
/// sends when a test changes the access.
const LONG_FILE: u64 = 256 * 1024 * 1024;

fn file_path(recording: RecordingId, segment: RecordingSegmentId) -> String {
    format!("/api/v1/recordings/{recording}/segments/{segment}/file")
}

/// Makes the video of a recording as long as [`LONG_FILE`]. The added bytes are a hole of
/// the file and take no space.
fn lengthen(server: &TestServer, recording: RecordingId, segment: RecordingSegmentId) {
    let video = server
        .recordings_dir()
        .join(recording.to_string())
        .join(format!("{segment}.mp4"));
    std::fs::OpenOptions::new()
        .write(true)
        .open(video)
        .unwrap()
        .set_len(LONG_FILE)
        .unwrap();
}

/// Starts a download and reads its first bytes. The response, and the bytes that arrived.
async fn begun(server: &TestServer, member: &Member, path: &str) -> (reqwest::Response, u64) {
    let mut response = reqwest::Client::new()
        .get(format!("{}{path}", server.base))
        .bearer_auth(&member.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first = response.chunk().await.unwrap().unwrap();
    (response, first.len() as u64)
}

/// Reads a download to its end. The bytes that arrived, and whether the response ended
/// as a whole.
async fn drained(mut response: reqwest::Response, mut received: u64) -> (u64, bool) {
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => received += chunk.len() as u64,
            Ok(None) => return (received, true),
            Err(_) => return (received, false),
        }
    }
}

/// Moves the time past the next decision of a running download.
fn pass_a_check(server: &TestServer) {
    let interval = server.state.config.limits.recordings.download_check;
    server.clock.advance(interval + Duration::seconds(1));
}

/// Denies or returns the sight of a channel to every member of its workspace who does not
/// administer the server.
async fn hide(server: &TestServer, owner: &Member, of: &Channel, hidden: bool) {
    let everyone = server.everyone_role(owner, of.workspace_id).await;
    let deny: &[&str] = if hidden { &["view_channel"] } else { &[] };
    server
        .put(
            owner,
            &format!("/api/v1/channels/{}/overrides/{}", of.id, everyone.id),
            &json!({ "allow": [], "deny": deny }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

async fn ban(server: &TestServer, owner: &Member, member: &Member) {
    server
        .put_empty(
            owner,
            &format!("/api/v1/accounts/{}/ban", member.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

async fn log_out(server: &TestServer, member: &Member) {
    server
        .delete(member, "/api/v1/auth/session")
        .await
        .empty(StatusCode::NO_CONTENT);
}

/// The server sends 32 files of recordings at one time, and one account
/// could hold every place. An account has a share of the places, as for attachments
/// (ADR 0608).
#[tokio::test]
async fn audit_one_account_holds_a_bounded_share_of_the_downloads_of_recording_files() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let done = studio.finished().await;
    let (composite, _) = parts(&done);
    lengthen(server, done.recording.id, composite.id);
    let path = file_path(done.recording.id, composite.id);
    let limits = &server.state.config.limits.recordings;
    assert!(limits.downloads_per_account < limits.most_downloads);

    let mut held = Vec::new();
    for _ in 0..limits.downloads_per_account {
        held.push(begun(server, ben, &path).await);
    }
    // The body of a file is not read here: a request that is not refused sends the file.
    let refused = reqwest::Client::new()
        .get(format!("{}{path}", server.base))
        .bearer_auth(&ben.token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused.headers()["retry-after"], "1");
    let refusal: Value = refused.json().await.unwrap();
    assert_eq!(refusal["error"]["code"], "busy");
    // The places of one account do not count against another account.
    let other = begun(server, ann, &path).await;

    // A download that ended gives its place back to its account.
    drop(held.pop());
    let waited = std::time::Instant::now();
    loop {
        let again = reqwest::Client::new()
            .get(format!("{}{path}", server.base))
            .bearer_auth(&ben.token)
            .send()
            .await
            .unwrap();
        if again.status() == StatusCode::OK {
            break;
        }
        assert!(waited.elapsed() < std::time::Duration::from_secs(20));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    drop((held, other));
}

/// The download of a recording file was decided once, before its body.
/// A download that runs is decided again at an interval, as the download of an export is
/// (ADR 0608): after a ban, a logout, the loss of the channel and the removal of the
/// recording, the rest of the file does not arrive.
#[tokio::test]
async fn audit_a_running_download_of_a_recording_file_ends_when_the_access_ends() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let done = studio.finished().await;
    let (composite, _) = parts(&done);
    lengthen(server, done.recording.id, composite.id);
    let path = file_path(done.recording.id, composite.id);
    let stays = server.join(owner, "Stays").await;
    let banned = server.join(owner, "Banned").await;
    let leaves = server.join(owner, "Leaves").await;

    // While the access stays, the download is complete, also after the time of a check.
    let (response, first) = begun(server, &stays, &path).await;
    pass_a_check(server);
    assert_eq!(drained(response, first).await, (LONG_FILE, true));

    let (response, first) = begun(server, &banned, &path).await;
    ban(server, owner, &banned).await;
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(received < LONG_FILE && !whole, "ban: {received} {whole}");

    let (response, first) = begun(server, &leaves, &path).await;
    log_out(server, &leaves).await;
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(received < LONG_FILE && !whole, "logout: {received} {whole}");

    let (response, first) = begun(server, &stays, &path).await;
    hide(server, owner, &scene.voice, true).await;
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(
        received < LONG_FILE && !whole,
        "channel: {received} {whole}"
    );
    hide(server, owner, &scene.voice, false).await;

    let (response, first) = begun(server, &stays, &path).await;
    server
        .delete(owner, &format!("/api/v1/recordings/{}", done.recording.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(
        received < LONG_FILE && !whole,
        "removal: {received} {whole}"
    );
}

/// The same for the rule of the recording itself (ADR 0604): while its call runs and hides
/// who is inside, only the people inside read a recording. A person who leaves that call
/// while a file is sent does not get the rest of it.
#[tokio::test]
async fn audit_a_running_download_of_a_recording_file_ends_when_its_reader_leaves_a_hidden_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server
        .call_settings(owner, scene.voice.id, json!({ "hides_occupants": true }))
        .await;
    server
        .grant_workspace(owner, ann, scene.workspace_id, &[W::RecordCalls])
        .await;
    let ann_seat = server.enter_call(ann, scene.voice.id).await;
    let ben_seat = server.enter_call(ben, scene.voice.id).await;
    server.speak(&ann_seat);
    server.speak(&ben_seat);
    let studio = Studio {
        scene,
        ann_seat,
        ben_seat,
    };
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let done = studio.finished().await;
    assert_eq!(done.recording.state, RecordingState::Ready);
    let (composite, _) = parts(&done);
    lengthen(server, done.recording.id, composite.id);
    let path = file_path(done.recording.id, composite.id);
    server.get(owner, &path).await.error(ErrorCode::NotFound);

    let (of_ben, first_of_ben) = begun(server, ben, &path).await;
    let (of_ann, first_of_ann) = begun(server, ann, &path).await;
    server.leave_call(ben).await;
    server.check().await;
    server.get(ben, &path).await.error(ErrorCode::NotFound);
    pass_a_check(server);
    let (received, whole) = drained(of_ben, first_of_ben).await;
    assert!(received < LONG_FILE && !whole, "{received} {whole}");
    // The person who is still inside gets the whole file.
    assert_eq!(drained(of_ann, first_of_ann).await, (LONG_FILE, true));
}

/// ADR 0408: the download of an attachment that runs is
/// decided again at the same interval. After a ban, a logout, the loss of the channel and
/// the deletion of the message, the rest of the file does not arrive.
#[tokio::test]
async fn audit_a_running_download_of_an_attachment_ends_when_the_access_ends() {
    const LENGTH: usize = 24 * 1024 * 1024;
    let directory = Scratch::new();
    let server = TestServer::start_tuned(
        |config| {
            config.attachments_directory = Some(directory.0.clone());
        },
        Some(16 * 1024),
    )
    .await;
    let server = &server;
    let enough = std::sync::Arc::new(junto_server::storage::FakeDisk::new(
        crate::support::FREE_DISK,
    ));
    server.state.attachment_space.use_disk(enough);
    let owner = &server.owner().await;
    let stays = server.join(owner, "Stays").await;
    let banned = server.join(owner, "Banned").await;
    let leaves = server.join(owner, "Leaves").await;
    let text = channel(server, owner, "text").await;
    let file = vec![7u8; LENGTH];
    let stored = upload(server, owner, json!({ "channel_id": text.id }), &file).await;
    let message: Message = server
        .post(
            owner,
            &format!("/api/v1/channels/{}/messages", text.id),
            &json!({ "document": { "blocks": [] }, "attachments": [stored.id] }),
        )
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/attachments/{}/file", stored.id);
    let length = LENGTH as u64;

    // While the access stays, the download is complete, also after the time of a check.
    let (response, first) = begun(server, &stays, &path).await;
    pass_a_check(server);
    assert_eq!(drained(response, first).await, (length, true));

    let (response, first) = begun(server, &banned, &path).await;
    ban(server, owner, &banned).await;
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "ban: {received} {whole}");

    let (response, first) = begun(server, &leaves, &path).await;
    log_out(server, &leaves).await;
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "logout: {received} {whole}");

    let (response, first) = begun(server, &stays, &path).await;
    hide(server, owner, &text, true).await;
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "channel: {received} {whole}");
    hide(server, owner, &text, false).await;

    let (response, first) = begun(server, &stays, &path).await;
    server
        .delete(owner, &format!("/api/v1/messages/{}", message.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    pass_a_check(server);
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "deletion: {received} {whole}");
}

/// A directory below `rescued/` with one file, as the start script of the recorder leaves
/// it for a lost job.
fn rescued(server: &TestServer, name: &str) -> PathBuf {
    let directory = server.recordings_dir().join("rescued").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("segment.ogg"), b"partial").unwrap();
    directory
}

async fn maintain(server: &TestServer) {
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Request of the recording run: when a recording was removed before the recorder started
/// again, the start script saves the audio of its lost job afterwards, and no row of the
/// queue of removals names that directory. Maintenance deletes a directory below
/// `rescued/` that no segment names when it is older than a day. A directory that a
/// segment names stays for the processing, a young one stays for the script that may still
/// write it, and nothing is deleted while a backup holds deletions (ADR 0606, ADR 0018).
#[tokio::test]
async fn audit_the_saved_audio_of_a_job_that_no_segment_names_is_deleted_after_a_day() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    studio.record().await;
    let job: String =
        sqlx::query_scalar("select job_id from recording_segments order by id limit 1")
            .fetch_one(&server.db)
            .await
            .unwrap();
    let of_a_segment = rescued(server, &job);
    let orphan = rescued(server, "EG_removed");
    let kept_outside = Scratch::new();
    std::fs::write(kept_outside.0.join("kept"), b"kept").unwrap();
    let root = server.recordings_dir().join("rescued");
    std::os::unix::fs::symlink(&kept_outside.0, root.join("EG_link")).unwrap();
    std::fs::create_dir_all(root.join("not a job")).unwrap();
    let age = server.state.config.limits.recordings.rescued_age;

    maintain(server).await;
    server.clock.advance(age - Duration::minutes(1));
    maintain(server).await;
    assert_eq!(
        names_in(&root),
        [job.as_str(), "EG_link", "EG_removed", "not a job"]
    );

    // A backup that runs holds the deletion back.
    server.clock.advance(Duration::minutes(2));
    let backup = junto_server::backup::begin(&server.state.database, server.clock.now())
        .await
        .unwrap();
    maintain(server).await;
    assert!(orphan.join("segment.ogg").is_file());
    assert!(
        junto_server::backup::end(&server.state.database, backup.id)
            .await
            .unwrap()
    );

    maintain(server).await;
    // The directory of a job of a segment stays, and so does a name that is no job. The
    // link is deleted and not followed.
    assert_eq!(names_in(&root), [job.as_str(), "not a job"]);
    assert!(of_a_segment.join("segment.ogg").is_file());
    assert!(kept_outside.0.join("kept").is_file());
}

/// One pass of the sweep examines a fixed number of directories, and the next pass
/// continues after them, so that directories which stay do not keep the sweep from the
/// others.
#[tokio::test]
async fn audit_the_sweep_of_saved_audio_examines_a_bounded_number_of_directories_per_pass() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    studio.record().await;
    let mut jobs: Vec<String> =
        sqlx::query_scalar("select job_id from recording_segments order by id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    jobs.sort();
    assert_eq!(jobs.len(), 3);
    for job in &jobs {
        rescued(server, job);
    }
    // In the order of the names, the directories of the jobs come first.
    let orphans = RESCUED_AT_ONCE + 10;
    for number in 0..orphans {
        rescued(server, &format!("ZZ_{number:04}"));
    }
    let root = server.recordings_dir().join("rescued");
    let age = server.state.config.limits.recordings.rescued_age;
    server.clock.advance(age + Duration::minutes(1));

    maintain(server).await;
    let examined_orphans = RESCUED_AT_ONCE - jobs.len();
    assert_eq!(
        names_in(&root).len(),
        jobs.len() + orphans - examined_orphans
    );
    maintain(server).await;
    assert_eq!(names_in(&root), jobs);
}

async fn failure_notes(server: &TestServer) -> Vec<MessageId> {
    sqlx::query_scalar("select id from call_notes where kind = 'recording_failed' order by id")
        .fetch_all(&server.db)
        .await
        .unwrap()
}

/// Request of the recording run: a job that was declared lost while its recorder lived
/// reports a complete file when the recording ends, and its segment becomes `ended`. The
/// note of a failure of the recording, and its line in the text chat, do not stay when no
/// segment of the recording failed in the end (ADR 0604).
#[tokio::test]
async fn audit_the_note_of_a_failure_goes_when_a_file_result_replaces_the_lost_declaration() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(11));
    let composite = composites(&server.recorder().jobs())[0].id.clone();
    server.fake().leave(&studio.ann_seat.room, &composite);
    server.tend().await;
    let lost = server.recording(owner, started.id).await;
    assert_eq!(lost.recording.failed_segments, 1);
    let noted = failure_notes(server).await;
    assert_eq!(noted.len(), 1);
    let line: Message = server
        .get(ann, &format!("/api/v1/messages/{}", noted[0]))
        .await
        .ok(StatusCode::OK);
    assert_eq!(line.system.unwrap().action, "recording_failed");

    let mut stream = server.full_stream(ann).await;
    server.clock.advance(Duration::seconds(9));
    let done = studio.stop(started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(done.recording.failed_segments, 0);
    assert!(failure_notes(server).await.is_empty());
    server
        .get(ann, &format!("/api/v1/messages/{}", noted[0]))
        .await
        .error(ErrorCode::NotFound);
    let removed = server
        .traffic(ann, &mut stream)
        .await
        .into_iter()
        .any(|(_, event)| {
            matches!(event, Event::MessageChanged { message_id, deleted: true, .. }
                if message_id == noted[0])
        });
    assert!(
        removed,
        "the people in the call are told that the line is gone"
    );
}

/// The case that the recording run met: the media server closed the room in the second in
/// which the call ended, so every job is declared lost and replaced by its file result in
/// the same run of the controller. No note of a failure is left.
#[tokio::test]
async fn audit_no_note_of_a_failure_is_left_when_every_job_ends_complete_after_its_room_closed() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(12));
    server.leave_call(ann).await;
    server.leave_call(ben).await;
    for job in server.recorder().jobs() {
        server.fake().leave(&studio.ann_seat.room, &job.id);
    }
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(done.recording.failed_segments, 0);
    assert!(failure_notes(server).await.is_empty());
    let notes = server.notes(scene.voice.id).await;
    assert!(!notes.contains(&"recording_failed".to_owned()), "{notes:?}");
}

/// The note stays while another segment of the recording has a failure.
#[tokio::test]
async fn audit_the_note_of_a_failure_stays_while_another_segment_failed() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(11));
    let jobs = server.recorder().jobs();
    let composite = composites(&jobs)[0].id.clone();
    let track = jobs.iter().find(|job| !job.is_composite()).unwrap();
    server.fake().leave(&studio.ann_seat.room, &composite);
    server.recorder().fail(&track.id);
    server.tend().await;
    assert_eq!(failure_notes(server).await.len(), 1);

    server.clock.advance(Duration::seconds(9));
    let done = studio.stop(started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    let failed: Vec<_> = done
        .segments
        .iter()
        .filter(|segment| segment.state == SegmentState::Failed)
        .map(|segment| (segment.source, segment.failure))
        .collect();
    assert_eq!(
        failed,
        [(
            SegmentSource::Microphone,
            Some(SegmentFailure::RecorderError)
        )]
    );
    assert_eq!(failure_notes(server).await.len(), 1);
}

/// For every event: the log of events is append-only, so an event that
/// holds content keeps it readable after the content was removed. No stored event holds
/// the text of a transcript line, of a correction, of a speaker name, of a glossary term,
/// of a message or of the name of an attached file (ADR 0408, ADR 0607).
#[tokio::test]
async fn audit_no_stored_event_holds_text_of_a_transcript_a_glossary_or_a_message() {
    use crate::recordings_support::{FakeWorker, Said, transcript_of};
    use junto_server::media::{TrackKind, TrackSource};

    let worker = FakeWorker::new();
    let directory = Scratch::new();
    let settings = worker.settings();
    let server = TestServer::start_for_calls_with(|config| {
        config.recorder.as_mut().unwrap().transcriber = Some(settings);
        config.attachments_directory = Some(directory.0.clone());
    })
    .await;
    let enough = std::sync::Arc::new(junto_server::storage::FakeDisk::new(
        crate::support::FREE_DISK,
    ));
    server.state.attachment_space.use_disk(enough);
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server.publish(
        &studio.ann_seat,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio,
    );
    server
        .put(
            owner,
            &format!("/api/v1/workspaces/{}/glossary", scene.workspace_id),
            &json!({ "terms": ["Quarzwerk Nordhafen"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let done = studio.finished_after(Duration::seconds(2)).await;
    let id = done.recording.id;
    let shared = done
        .segments
        .iter()
        .filter(|segment| segment.source != SegmentSource::Composite && segment.file.is_some())
        .position(|segment| segment.source == SegmentSource::ScreenShareAudio)
        .unwrap();
    worker.give(&transcript_of(
        &done,
        (&["Quarzwerk Nordhafen"], &[]),
        &[Said(
            shared,
            100,
            900,
            "The Quarzwerk closes in spring.",
            Some(1),
        )],
    ));
    assert!(server.transcribe().await);
    server
        .put(
            ben,
            &format!("/api/v1/recordings/{id}/transcript/lines/0"),
            &json!({ "text": "The Quarzwerk closes in autumn." }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .put(
            ben,
            &format!("/api/v1/recordings/{id}/transcript/speakers/1"),
            &json!({ "name": "Wilhelmina Oakhurst" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let stored = upload(
        server,
        ann,
        json!({ "channel_id": scene.voice.id }),
        b"attached",
    )
    .await;
    let mut body = crate::messages_support::text("The merger closes in March.");
    body["attachments"] = json!([stored.id]);
    let message: Message = server
        .post(
            ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
            &body,
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            ann,
            &format!("/api/v1/messages/{}", message.id),
            &crate::messages_support::text("The merger closes in April."),
        )
        .await
        .ok::<Message>(StatusCode::OK);

    let needles = ["Quarzwerk", "closes in", "Oakhurst", "merger", "file.bin"];
    for needle in needles {
        let kinds: Vec<String> = sqlx::query_scalar(
            "select distinct kind from events
             where payload::text ilike '%' || $1 || '%' or audience::text ilike '%' || $1 || '%'",
        )
        .bind(needle)
        .fetch_all(&server.db)
        .await
        .unwrap();
        assert!(kinds.is_empty(), "{needle:?} is in the events {kinds:?}");
    }
    // The content exists, so the search above had something to find.
    let lines = server.transcript_lines(ann, id, 10).await;
    assert_eq!(lines[0].text, "The Quarzwerk closes in autumn.");
    let transcript = server.transcript(ann, id).await;
    assert_eq!(
        transcript.speakers[0].name.as_deref(),
        Some("Wilhelmina Oakhurst")
    );
    assert!(server.count("events").await > 20);
}

/// The migration that takes the text out of the events of transcripts (ADR 0607) takes it
/// out of the events that the log already holds, and leaves the log append-only.
#[tokio::test]
async fn audit_the_migration_takes_the_text_out_of_the_stored_events_of_transcripts() {
    let server = TestServer::start().await;
    let (recording, workspace, channel, account) = (
        uuid::Uuid::now_v7(),
        uuid::Uuid::now_v7(),
        uuid::Uuid::now_v7(),
        uuid::Uuid::now_v7(),
    );
    let common = json!({
        "recording_id": recording, "workspace_id": workspace, "channel_id": channel,
    });
    let with = |more: Value| {
        let mut payload = common.clone();
        for (name, value) in more.as_object().unwrap() {
            payload[name] = value.clone();
        }
        payload
    };
    let stored = [
        with(json!({
            "type": "transcript_line_corrected", "line": 4,
            "text": "the merger closes in March", "corrected_by": account,
            "corrected_at": "2026-01-01T00:00:00Z",
        })),
        with(json!({
            "type": "transcript_speaker_renamed",
            "speaker": {
                "number": 3, "account_id": account, "name": "Wilhelmina Oakhurst",
                "renamed_by": account, "renamed_at": "2026-01-01T00:00:00Z",
            },
        })),
        with(json!({ "type": "transcript_speaker_renamed", "speaker": 2 })),
        with(json!({ "type": "recording_removed" })),
    ];
    for payload in &stored {
        sqlx::query(
            "insert into events (id, recorded_at, class, kind, audience, payload)
             values ($1, now(), 'general', $2, $3, $4)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(payload["type"].as_str().unwrap())
        .bind(json!({ "to": "nobody" }))
        .bind(payload)
        .execute(&server.db)
        .await
        .unwrap();
    }

    let migration = include_str!("../../migrations/0012_recordings_after_audit.sql");
    let last = "alter table events enable trigger events_no_change;";
    let start = migration
        .find("alter table events disable trigger")
        .unwrap();
    let end = migration.find(last).unwrap() + last.len();
    sqlx::raw_sql(sqlx::AssertSqlSafe(migration[start..end].to_owned()))
        .execute(&server.db)
        .await
        .unwrap();

    let migrated: Vec<Value> = sqlx::query_scalar(
        "select payload from events where payload ->> 'recording_id' = $1 order by position",
    )
    .bind(recording.to_string())
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(
        migrated,
        [
            with(json!({ "type": "transcript_line_corrected", "line": 4 })),
            with(json!({ "type": "transcript_speaker_renamed", "speaker": 3 })),
            with(json!({ "type": "transcript_speaker_renamed", "speaker": 2 })),
            with(json!({ "type": "recording_removed" })),
        ]
    );
    for payload in migrated {
        serde_json::from_value::<Event>(payload).unwrap();
    }
    let changed = sqlx::query("update events set recorded_at = now()")
        .execute(&server.db)
        .await;
    assert!(changed.is_err(), "the log of events takes a change again");
}
