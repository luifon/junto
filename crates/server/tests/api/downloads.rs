//! The download of the files of a recording, and the removal of a recording with its files
//! (ADR 0602, ADR 0608).

use junto_protocol::WorkspacePermission as W;
use junto_protocol::audit::AuditEvent;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{RecordingId, RecordingSegmentId};
use junto_protocol::recording::{RecordingDetail, RecordingState};
use junto_protocol::transcript::TranscriptState;
use junto_server::recorder::samples;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::recordings_support::{FakeWorker, Said, Studio, parts, transcript_of};
use crate::support::{Member, Reply, TestServer};

fn file_path(recording: RecordingId, segment: RecordingSegmentId) -> String {
    format!("/api/v1/recordings/{recording}/segments/{segment}/file")
}

async fn ranged(server: &TestServer, member: &Member, path: &str, range: &str) -> Reply {
    let request = reqwest::Client::new()
        .request(Method::GET, format!("{}{path}", server.base))
        .bearer_auth(&member.token)
        .header("Range", range);
    TestServer::reply(request).await
}

fn on_disk(server: &TestServer, detail: &RecordingDetail, name: &str) -> Vec<u8> {
    let directory = server
        .recordings_dir()
        .join(detail.recording.id.to_string());
    std::fs::read(directory.join(name)).unwrap()
}

#[tokio::test]
async fn the_video_and_each_audio_file_are_downloaded_whole() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ben) = (&scene.server, &scene.ben);
    let done = studio.finished().await;
    let (composite, tracks) = parts(&done);

    let video = server
        .get(ben, &file_path(done.recording.id, composite.id))
        .await;
    assert_eq!(video.status, StatusCode::OK);
    let bytes = on_disk(server, &done, &format!("{}.mp4", composite.id));
    assert_eq!(video.bytes(), bytes);
    assert_eq!(video.header("content-type"), Some("video/mp4"));
    assert_eq!(
        video.header("content-length"),
        Some(bytes.len().to_string().as_str())
    );
    assert_eq!(video.header("accept-ranges"), Some("bytes"));
    assert_eq!(video.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        video.header("content-disposition"),
        Some(format!("attachment; filename=\"{}.mp4\"", composite.id).as_str())
    );
    assert_eq!(video.header("content-range"), None);

    for track in tracks {
        let sound = server
            .get(ben, &file_path(done.recording.id, track.id))
            .await;
        assert_eq!(sound.status, StatusCode::OK);
        assert_eq!(sound.bytes(), samples::track().as_slice());
        assert_eq!(sound.header("content-type"), Some("audio/ogg"));
        assert_eq!(
            sound.header("content-disposition"),
            Some(format!("attachment; filename=\"{}.ogg\"", track.id).as_str())
        );
    }
}

#[tokio::test]
async fn one_range_of_a_file_is_downloaded() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ben) = (&scene.server, &scene.ben);
    let done = studio.finished().await;
    let (composite, _) = parts(&done);
    let path = file_path(done.recording.id, composite.id);
    let bytes = on_disk(server, &done, &format!("{}.mp4", composite.id));
    let length = bytes.len();
    assert!(length > 70_000, "{length}");

    let cases = [
        ("bytes=0-99".to_owned(), 0, 99),
        ("bytes=100-65999".to_owned(), 100, 65_999),
        (format!("bytes={}-", length - 10), length - 10, length - 1),
        ("bytes=-7".to_owned(), length - 7, length - 1),
        (format!("bytes=5-{}", length + 500), 5, length - 1),
    ];
    for (range, first, last) in cases {
        let part = ranged(server, ben, &path, &range).await;
        assert_eq!(part.status, StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(part.bytes(), &bytes[first..=last], "{range}");
        assert_eq!(
            part.header("content-range"),
            Some(format!("bytes {first}-{last}/{length}").as_str())
        );
        assert_eq!(
            part.header("content-length"),
            Some((last - first + 1).to_string().as_str())
        );
        assert_eq!(part.header("content-type"), Some("video/mp4"));
    }

    // A range outside the file is refused with the length of the file.
    for outside in [
        format!("bytes={length}-"),
        format!("bytes={}-{}", length + 5, length + 9),
    ] {
        let refused = ranged(server, ben, &path, &outside).await;
        refused.error(ErrorCode::RangeNotSatisfiable);
        assert_eq!(refused.status.as_u16(), 416);
        assert_eq!(
            refused.header("content-range"),
            Some(format!("bytes */{length}").as_str())
        );
    }
    // A header that is no single range of bytes asks for the whole file.
    for whole in ["bytes=0-9,20-29", "items=0-9", "bytes=9-1"] {
        let all = ranged(server, ben, &path, whole).await;
        assert_eq!(all.status, StatusCode::OK, "{whole}");
        assert_eq!(all.bytes().len(), length);
    }
}

#[tokio::test]
async fn a_file_is_downloaded_by_the_rule_of_its_recording() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let done = studio.finished().await;
    let id = done.recording.id;
    let (composite, tracks) = parts(&done);
    let outsider = server.join(owner, "Cy").await;
    let absent = server
        .get(&outsider, &file_path(RecordingId::generate(), composite.id))
        .await
        .observable();
    assert_eq!(absent.0, 404);

    // A segment that the recording does not have, and a segment of another recording.
    let other = studio.finished().await;
    let (other_composite, _) = parts(&other);
    for unknown in [RecordingSegmentId::generate(), other_composite.id] {
        assert_eq!(
            server.get(ann, &file_path(id, unknown)).await.observable(),
            absent
        );
    }

    // While the call runs and hides who is inside, only the people inside download a file
    // (ADR 0604). Turning the setting off changes nothing for that call (ADR 0517).
    for turn in 0..2 {
        for path in [file_path(id, composite.id), file_path(id, tracks[0].id)] {
            assert_eq!(server.get(&outsider, &path).await.observable(), absent);
            assert_eq!(server.get(owner, &path).await.observable(), absent);
            assert_eq!(server.get(ann, &path).await.status, StatusCode::OK);
        }
        if turn == 0 {
            server
                .call_settings(owner, scene.voice.id, json!({ "hides_occupants": false }))
                .await;
        }
    }
    let unsigned = server
        .send::<Value>(Method::GET, &file_path(id, composite.id), None, None, None)
        .await;
    unsigned.error(ErrorCode::Unauthenticated);

    // When the call has ended, every person who sees the channel downloads the file.
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server.check().await;
    for reader in [&outsider, owner] {
        let whole = server.get(reader, &file_path(id, composite.id)).await;
        assert_eq!(whole.status, StatusCode::OK);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_segment_without_a_file_and_a_link_in_its_place_give_no_download() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    let (composite, tracks) = parts(&stopped);
    // Before the processing no segment has a file.
    server
        .get(ann, &file_path(started.id, composite.id))
        .await
        .error(ErrorCode::NotFound);

    assert_eq!(server.process().await, 1);
    let directory = server.recordings_dir().join(started.id.to_string());
    let track = directory.join(format!("{}.ogg", tracks[0].id));
    assert_eq!(
        server
            .get(ann, &file_path(started.id, tracks[0].id))
            .await
            .status,
        StatusCode::OK
    );
    // A link where the file was is not followed.
    std::fs::remove_file(&track).unwrap();
    std::os::unix::fs::symlink(directory.join(format!("{}.mp4", composite.id)), &track).unwrap();
    server
        .get(ann, &file_path(started.id, tracks[0].id))
        .await
        .error(ErrorCode::NotFound);
    std::fs::remove_file(&track).unwrap();
    server
        .get(ann, &file_path(started.id, tracks[0].id))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn the_server_sends_a_bounded_number_of_files_at_one_time() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished().await;
    let (composite, _) = parts(&done);
    let none = server
        .strict(|config| config.limits.recordings.most_downloads = 0)
        .await;
    let refused = none
        .get(ann, &file_path(done.recording.id, composite.id))
        .await;
    refused.error(ErrorCode::Busy);
    assert_eq!(refused.header("retry-after"), Some("1"));

    // A download that ended gives its place back.
    let one = server
        .strict(|config| config.limits.recordings.most_downloads = 1)
        .await;
    for _ in 0..3 {
        let sent = one
            .get(ann, &file_path(done.recording.id, composite.id))
            .await;
        assert_eq!(sent.status, StatusCode::OK);
        let waited = std::time::Instant::now();
        while one.state.recordings.download(ann.account_id).is_none() {
            assert!(waited.elapsed() < std::time::Duration::from_secs(20));
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

#[tokio::test]
async fn a_person_with_the_permission_removes_a_recording_with_its_files_and_its_transcript() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let done = studio.finished_after(Duration::seconds(2)).await;
    let id = done.recording.id;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 500, 1500, "Good morning.", None)],
    ));
    assert!(server.transcribe().await);
    server
        .put(
            ben,
            &format!("/api/v1/recordings/{id}/transcript/lines/0"),
            &json!({ "text": "Good day." }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let kept = studio.finished().await;
    let directory = server.recordings_dir().join(id.to_string());
    assert!(directory.join("transcript.json").is_file());
    let manager = server.join(owner, "Manager").await;
    server
        .grant_workspace(owner, &manager, scene.workspace_id, &[W::ManageRecordings])
        .await;
    let path = format!("/api/v1/recordings/{id}");

    // The person who recorded has no right to remove (ADR 0602).
    server.delete(ann, &path).await.error(ErrorCode::Forbidden);
    assert!(directory.is_dir());
    assert_eq!(server.count("recordings").await, 2);

    let mut stream = server.full_stream(ben).await;
    server
        .delete(&manager, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&manager, &path)
        .await
        .error(ErrorCode::NotFound);
    server
        .recording_reply(ann, id)
        .await
        .error(ErrorCode::NotFound);
    let listed = server.recordings(ann, scene.workspace_id).await;
    assert_eq!(listed.recordings.len(), 1);
    assert_eq!(listed.recordings[0].id, kept.recording.id);
    for (table, left) in [
        ("recordings", 1),
        ("recording_segments", 3),
        ("transcripts", 1),
        ("transcript_lines", 0),
        ("transcript_corrections", 0),
        ("transcript_segments", 0),
        ("storage_reservations", 0),
    ] {
        assert_eq!(server.count(table).await, left, "{table}");
    }
    let events: Vec<Event> = server
        .traffic(ben, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    let [
        removed,
        Event::MessageChanged {
            message_id: post,
            deleted: true,
            ..
        },
        Event::MessageChanged {
            message_id: line,
            deleted: false,
            ..
        },
    ] = &events[..]
    else {
        panic!("{events:?}");
    };
    assert_eq!(
        *removed,
        Event::RecordingRemoved {
            recording_id: id,
            workspace_id: scene.workspace_id,
            channel_id: scene.voice.id
        }
    );
    server
        .get(ben, &format!("/api/v1/messages/{post}"))
        .await
        .error(ErrorCode::NotFound);
    let system: junto_protocol::message::Message = server
        .get(ben, &format!("/api/v1/messages/{line}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(system.system.unwrap().action, "recording_removed");
    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let last = entries.last().unwrap();
    assert_eq!(
        last.event,
        AuditEvent::RecordingRemoved { recording_id: id }
    );
    assert_eq!(last.actor_id, Some(manager.account_id));
    assert_eq!(last.channel_id, Some(scene.voice.id));

    // The notes of the start and the stop stay without the recording; the note of the post
    // goes with it.
    let notes: Vec<(String, Option<uuid::Uuid>)> =
        sqlx::query_as("select kind, recording_id from call_notes order by id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    let of_kept = Some(kept.recording.id.as_uuid());
    assert_eq!(
        notes,
        [
            ("call_started".to_owned(), None),
            ("recording_started".to_owned(), None),
            ("recording_stopped".to_owned(), None),
            ("recording_started".to_owned(), of_kept),
            ("recording_stopped".to_owned(), of_kept),
            ("recording_ready".to_owned(), of_kept),
        ]
    );

    // The directory is deleted after the transaction, through the queue of removals.
    assert_eq!(server.count("file_removals").await, 1);
    assert_eq!(server.process().await, 0);
    assert_eq!(server.count("file_removals").await, 0);
    assert!(!directory.exists());
    assert!(
        server
            .recordings_dir()
            .join(kept.recording.id.to_string())
            .is_dir()
    );
    let (composite, _) = parts(&kept);
    assert_eq!(
        server
            .get(ann, &file_path(kept.recording.id, composite.id))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        server
            .recording(ann, kept.recording.id)
            .await
            .recording
            .transcript,
        TranscriptState::Waiting
    );
}

#[tokio::test]
async fn a_recording_that_runs_or_is_processed_is_not_removed() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    let path = format!("/api/v1/recordings/{}", started.id);

    server.delete(owner, &path).await.error(ErrorCode::Conflict);
    let stopped = studio.stop(started.id).await;
    assert_eq!(stopped.recording.state, RecordingState::Processing);
    server.delete(owner, &path).await.error(ErrorCode::Conflict);
    assert_eq!(server.count("recordings").await, 1);
    assert_eq!(server.count("file_removals").await, 0);

    assert_eq!(server.process().await, 1);
    server
        .delete(owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("recordings").await, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_recording_is_removed_and_a_directory_that_cannot_be_deleted_stays_queued() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    server.recorder().lose_files(true);
    let failed = studio.finished().await;
    assert_eq!(failed.recording.state, RecordingState::Failed);
    let path = format!("/api/v1/recordings/{}", failed.recording.id);
    // The directory of the recording does not exist: its removal is no error.
    server
        .delete(owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("file_removals").await, 1);
    server.process().await;
    assert_eq!(server.count("file_removals").await, 0);

    server.recorder().lose_files(false);
    let done = studio.finished().await;
    let directory = server.recordings_dir().join(done.recording.id.to_string());
    server
        .delete(owner, &format!("/api/v1/recordings/{}", done.recording.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    // The directory of the recordings refuses the deletion.
    let root = server.recordings_dir();
    let mode =
        |mode| std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(mode));
    mode(0o500).unwrap();
    server.process().await;
    let still_queued = server.count("file_removals").await;
    mode(0o700).unwrap();
    assert_eq!(still_queued, 1);
    assert!(directory.exists());
    // The deletion is tried again after five minutes.
    server.process().await;
    assert_eq!(server.count("file_removals").await, 1);
    server.clock.advance(Duration::seconds(299));
    server.process().await;
    assert!(directory.exists());
    server.clock.advance(Duration::seconds(1));
    server.process().await;
    assert_eq!(server.count("file_removals").await, 0);
    assert!(!directory.exists());
}
