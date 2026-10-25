//! The processing of the files of a finished recording (ADR 0602, ADR 0606): the search for
//! each file, the join of the composite, the check, the recovery after a lost recorder, and
//! the final state.

use junto_protocol::events::Event;
use junto_protocol::recording::{
    FileCheck, FileProblem, RecordingFailure, RecordingState, SegmentSource, SegmentState,
};
use junto_protocol::transcript::TranscriptState;
use junto_server::media::{TrackKind, TrackSource};
use junto_server::recorder::samples;
use reqwest::StatusCode;
use time::Duration;

use crate::recordings_support::{Studio, parts, problems};
use crate::support::TestServer;

fn length() -> Duration {
    Duration::seconds(i64::from(samples::SECONDS))
}

#[tokio::test]
async fn a_stopped_recording_becomes_ready_with_a_video_and_a_file_for_each_track() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(length());
    let stopped = studio.stop(started.id).await;
    assert_eq!(stopped.recording.state, RecordingState::Processing);
    assert_eq!(stopped.recording.bytes, None);
    assert!(
        stopped
            .segments
            .iter()
            .all(|segment| segment.file.is_none())
    );
    assert_eq!(server.count("storage_reservations").await, 1);
    let mut stream = server.full_stream(owner).await;

    assert_eq!(server.process().await, 1);
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Ready);
    assert_eq!(done.recording.failure, None);
    assert_eq!(done.recording.transcript, TranscriptState::None);
    assert_eq!(server.count("storage_reservations").await, 0);
    assert_eq!(server.process().await, 0);

    let (composite, tracks) = parts(&done);
    let directory = server.recordings_dir().join(started.id.to_string());
    let video = composite.file.as_ref().unwrap();
    assert_eq!(video.media_type, "video/mp4");
    assert_eq!((video.width, video.height), (Some(320), Some(180)));
    assert_eq!(
        (video.check, &video.problems[..]),
        (FileCheck::Passed, &[][..])
    );
    assert!(!video.silent && !video.recovered);
    assert!((1900..2200).contains(&video.duration_ms), "{video:?}");
    let on_disk = std::fs::metadata(directory.join(format!("{}.mp4", composite.id))).unwrap();
    assert_eq!(video.bytes, on_disk.len());
    // The pieces are deleted after the video passed its check (ADR 0602).
    assert!(!directory.join(composite.id.to_string()).exists());
    // The composite is placed by its reported end and its length (ADR 0602).
    let placed =
        (began + length()) - Duration::milliseconds(i64::try_from(video.duration_ms).unwrap());
    assert_eq!(video.starts_at, Some(placed - Duration::milliseconds(135)));

    assert_eq!(tracks.len(), 2);
    for track in &tracks {
        let file = track.file.as_ref().unwrap();
        assert_eq!(file.media_type, "audio/ogg");
        assert_eq!((file.width, file.height), (None, None));
        assert_eq!(file.check, FileCheck::Passed);
        assert!(!file.silent && !file.recovered);
        assert!((1900..2100).contains(&file.duration_ms), "{file:?}");
        let on_disk = std::fs::metadata(directory.join(format!("{}.ogg", track.id))).unwrap();
        assert_eq!(file.bytes, on_disk.len());
        // An audio file is 47 ms later than its reported start (ADR 0602).
        assert_eq!(file.starts_at, Some(began - Duration::milliseconds(47)));
    }
    let bytes: u64 = done
        .segments
        .iter()
        .map(|segment| segment.file.as_ref().unwrap().bytes)
        .sum();
    assert_eq!(done.recording.bytes, Some(bytes));
    let earliest = done
        .segments
        .iter()
        .filter_map(|segment| segment.file.as_ref()?.starts_at)
        .min();
    assert_eq!(done.recording.timeline_started_at, earliest);
    assert!(earliest.is_some());

    assert_eq!(
        server.notes(scene.voice.id).await,
        [
            "call_started",
            "person_joined",
            "recording_started",
            "recording_stopped",
            "recording_ready"
        ]
    );
    let events = server.recording_events(owner, &mut stream).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "recording_updated");
    assert_eq!(events[0].1, done.recording);
}

#[tokio::test]
async fn a_file_with_a_hole_in_its_timestamps_makes_the_recording_partly_failed() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let ben_track = server
        .fake()
        .track_id(
            &studio.ben_seat.room,
            &studio.ben_seat.identity,
            TrackSource::Microphone,
        )
        .unwrap();
    server
        .recorder()
        .set_track_of(&ben_track, samples::holed_track());

    // The file with the hole spans three seconds.
    let done = studio.finished_after(Duration::seconds(3)).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    assert_eq!(done.recording.failure, None);
    assert_eq!(done.recording.failed_segments, 0);
    let (composite, tracks) = parts(&done);
    assert_eq!(problems(composite), []);
    for track in tracks {
        let file = track.file.as_ref().unwrap();
        if track.account_id == Some(studio.scene.ben.account_id) {
            assert_eq!(file.check, FileCheck::Failed);
            assert_eq!(file.problems, [FileProblem::TimestampHole]);
        } else {
            assert_eq!(
                (file.check, &file.problems[..]),
                (FileCheck::Passed, &[][..])
            );
        }
        assert_eq!(track.state, SegmentState::Ended);
    }
    assert_eq!(server.count("storage_reservations").await, 0);
    let notes = server.notes(studio.scene.voice.id).await;
    assert_eq!(
        notes[notes.len() - 2..],
        ["recording_failed", "recording_ready"]
    );
}

#[tokio::test]
async fn a_dark_picture_and_a_video_without_sound_fail_the_check_and_keep_the_pieces() {
    for (pieces, problem) in [
        (samples::dark_composite(), FileProblem::PictureBlank),
        (samples::soundless_composite(), FileProblem::StreamMissing),
    ] {
        let studio = Studio::open().await;
        let server = &studio.scene.server;
        server.recorder().set_pieces(pieces);
        let done = studio.finished().await;
        assert_eq!(
            done.recording.state,
            RecordingState::PartlyFailed,
            "{problem:?}"
        );
        let (composite, tracks) = parts(&done);
        assert_eq!(problems(composite), [problem]);
        assert!(tracks.iter().all(|track| problems(track).is_empty()));
        // The video did not pass, so the pieces that it was joined from stay (ADR 0602).
        let directory = server.recordings_dir().join(done.recording.id.to_string());
        assert!(directory.join(format!("{}.mp4", composite.id)).exists());
        let pieces = directory.join(composite.id.to_string());
        assert!(pieces.join("part_00000.ts").exists(), "{problem:?}");
    }
}

/// A muted microphone gives a file of digital silence (ADR 0601). That is no defect.
#[tokio::test]
async fn a_track_of_digital_silence_is_stated_and_is_no_failure() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    server.recorder().set_track(samples::silent_track());
    server.recorder().set_pieces(samples::mute_composite());

    let done = studio.finished_after(length()).await;
    assert_eq!(done.recording.state, RecordingState::Ready, "{done:?}");
    for segment in &done.segments {
        let file = segment.file.as_ref().unwrap();
        assert!(file.silent, "{segment:?}");
        assert_eq!(file.check, FileCheck::Passed);
    }
}

/// ADR 0602: the composite is not silent in a range in which an audio file of the same time
/// has sound. A layout page that plays no track gives such a composite, and the recorder
/// reports it as complete.
#[tokio::test]
async fn a_composite_that_is_silent_while_a_microphone_has_sound_fails_the_check() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    server.recorder().set_pieces(samples::mute_composite());

    let done = studio.finished_after(length()).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (composite, tracks) = parts(&done);
    assert_eq!(problems(composite), [FileProblem::SoundMissing]);
    assert!(composite.file.as_ref().unwrap().silent);
    assert!(tracks.iter().all(|track| problems(track).is_empty()));
}

#[tokio::test]
async fn a_file_whose_length_differs_from_the_reported_length_fails_the_check() {
    let studio = Studio::open().await;
    // The recorder reports 12 seconds for files that hold 2 seconds, as a recorder that was
    // short of CPU does (ADR 0602).
    let done = studio.finished_after(Duration::seconds(12)).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    for segment in &done.segments {
        assert_eq!(segment.duration_ms, Some(12_000));
        assert_eq!(
            problems(segment),
            [FileProblem::LengthMismatch],
            "{segment:?}"
        );
    }

    // Within three seconds the lengths agree.
    let studio = Studio::open().await;
    let done = studio.finished_after(Duration::seconds(5)).await;
    assert_eq!(done.recording.state, RecordingState::Ready);
}

#[tokio::test]
async fn a_recording_of_which_no_file_is_found_fails() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    server.recorder().lose_files(true);
    let mut stream = server.full_stream(&scene.owner).await;

    let done = studio.finished().await;
    assert_eq!(done.recording.state, RecordingState::Failed);
    assert_eq!(done.recording.failure, Some(RecordingFailure::FilesMissing));
    assert_eq!(done.recording.bytes, Some(0));
    assert_eq!(done.recording.timeline_started_at, None);
    assert!(done.segments.iter().all(|segment| segment.file.is_none()));
    assert!(
        done.segments
            .iter()
            .all(|segment| segment.state == SegmentState::Ended)
    );
    assert_eq!(server.count("storage_reservations").await, 0);
    let notes = server.notes(scene.voice.id).await;
    assert_eq!(notes.last().map(String::as_str), Some("recording_failed"));
    assert!(!notes.contains(&"recording_ready".to_owned()));
    let events = server.recording_events(&scene.owner, &mut stream).await;
    assert_eq!(events.last().unwrap().1.state, RecordingState::Failed);
}

#[tokio::test]
async fn one_missing_file_makes_the_recording_partly_failed() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    let (_, tracks) = parts(&stopped);
    let directory = server.recordings_dir().join(started.id.to_string());
    std::fs::remove_file(directory.join(format!("{}.ogg", tracks[0].id))).unwrap();

    assert_eq!(server.process().await, 1);
    let done = server.recording(&studio.scene.ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (composite, tracks) = parts(&done);
    assert!(tracks[0].file.is_none());
    assert_eq!(tracks[0].state, SegmentState::Ended);
    assert!(tracks[1].file.is_some() && composite.file.is_some());
}

/// ADR 0601 and ADR 0602: after a lost recorder, the server recovers the files that are on
/// disk. The pieces of the composite are where the recorder copied them; the audio files
/// are where the start script of the recorder saved them.
#[tokio::test]
async fn the_files_of_a_lost_recorder_are_recovered() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    server.recorder().rescue_files(true);
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(15));
    server.recorder().kill();
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    server.recorder().participants_gone();
    server.tend().await;
    let lost = server.recording(ann, started.id).await;
    assert_eq!(lost.recording.state, RecordingState::Processing);
    assert_eq!(lost.recording.failed_segments, 3);
    let jobs = server.recorder().jobs();
    let root = server.recordings_dir();
    let rescued = |job: usize| root.join("rescued").join(&jobs[job].id);
    assert!(rescued(1).is_dir() && rescued(2).is_dir());
    let directory = root.join(started.id.to_string());
    let (_, tracks) = parts(&lost);
    assert!(!directory.join(format!("{}.ogg", tracks[0].id)).exists());

    assert_eq!(server.process().await, 1);
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (composite, tracks) = parts(&done);
    for track in &tracks {
        assert_eq!(track.state, SegmentState::Failed);
        let file = track.file.as_ref().unwrap();
        assert!(file.recovered);
        assert_eq!(
            (file.check, &file.problems[..]),
            (FileCheck::Passed, &[][..])
        );
        assert_eq!(file.starts_at, Some(began - Duration::milliseconds(47)));
        assert!(directory.join(format!("{}.ogg", track.id)).is_file());
    }
    // The composite has no end marker and no reported end. Its pieces are joined, and the
    // tone of the samples has no feature that places it.
    let video = composite.file.as_ref().unwrap();
    assert!(video.recovered);
    assert_eq!(
        (video.check, &video.problems[..]),
        (FileCheck::Passed, &[][..])
    );
    assert_eq!(video.starts_at, None);
    assert!(directory.join(format!("{}.mp4", composite.id)).is_file());

    // The directories of the saved files are deleted through the queue of removals.
    assert_eq!(server.count("file_removals").await, 3);
    server.process().await;
    assert_eq!(server.count("file_removals").await, 0);
    assert!(!rescued(1).exists() && !rescued(2).exists());
    assert!(directory.is_dir());
}

/// Without the start script, the audio files of a lost recorder are gone (ADR 0602).
#[tokio::test]
async fn a_lost_recorder_without_saved_files_leaves_the_video_only() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(15));
    server.recorder().kill();
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    server.recorder().participants_gone();
    server.tend().await;

    assert_eq!(server.process().await, 1);
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (composite, tracks) = parts(&done);
    assert!(composite.file.is_some());
    assert!(tracks.iter().all(|track| track.file.is_none()));
    assert_eq!(done.recording.transcript, TranscriptState::None);
}

/// ADR 0602: a composite without an end time is placed by its sound, at the shift at which
/// its loudness agrees best with one microphone file.
#[tokio::test]
async fn a_composite_without_a_reported_end_is_placed_by_its_sound() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    server.recorder().rescue_files(true);
    server.recorder().set_track(samples::bursts());
    server.recorder().set_pieces(samples::late_composite());
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(12));
    server.recorder().kill();
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    server.recorder().participants_gone();
    server.tend().await;

    assert_eq!(server.process().await, 1);
    let done = server.recording(ann, started.id).await;
    let (composite, tracks) = parts(&done);
    assert_eq!(composite.ended_at, None);
    let video = composite.file.as_ref().unwrap();
    assert_eq!(video.problems, []);
    // The sound is 1.6 seconds later in the composite than in the microphone file, and the
    // microphone file is 47 ms later than its reported start.
    let expected = began - Duration::milliseconds(samples::LATE_MS + 47);
    let placed = video.starts_at.expect("a place on the timeline");
    assert!(
        (placed - expected).abs() < Duration::milliseconds(40),
        "{placed} against {expected}"
    );
    assert_eq!(done.recording.timeline_started_at, Some(placed));
    assert!(tracks.iter().all(|track| track.file.is_some()));
}

/// A program that does not run is no defect of a file. The attempt is repeated after a wait,
/// and the third attempt states what could not be checked.
#[tokio::test]
async fn processing_is_tried_three_times_when_a_program_does_not_run() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let started = studio.record().await;
    studio.stop(started.id).await;
    let broken = server
        .strict(|config| {
            config.recorder.as_mut().unwrap().ffprobe = "/nonexistent/ffprobe".into();
        })
        .await;

    assert_eq!(broken.process().await, 1);
    let waiting = server.recording(ann, started.id).await;
    assert_eq!(waiting.recording.state, RecordingState::Processing);
    assert!(
        waiting
            .segments
            .iter()
            .all(|segment| segment.file.is_none())
    );
    assert_eq!(server.count("storage_reservations").await, 1);
    // The next attempt waits.
    assert_eq!(broken.process().await, 0);
    broken.clock.advance(Duration::seconds(31));
    assert_eq!(broken.process().await, 1);
    assert_eq!(
        server.recording(ann, started.id).await.recording.state,
        RecordingState::Processing
    );

    broken.clock.advance(Duration::seconds(31));
    assert_eq!(broken.process().await, 1);
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    for segment in &done.segments {
        let file = segment.file.as_ref().unwrap();
        assert_eq!(file.problems, [FileProblem::NotChecked], "{segment:?}");
        assert!(file.bytes > 0);
    }
    assert_eq!(server.count("storage_reservations").await, 0);
}

/// An attempt that did not end leaves the video that it joined. The next attempt uses it.
#[tokio::test]
async fn an_attempt_after_a_failed_one_finishes_the_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    let (composite, _) = parts(&stopped);
    let directory = server.recordings_dir().join(started.id.to_string());
    let video = directory.join(format!("{}.mp4", composite.id));
    let broken = server
        .strict(|config| {
            config.recorder.as_mut().unwrap().ffprobe = "/nonexistent/ffprobe".into();
        })
        .await;
    assert_eq!(broken.process().await, 1);
    assert!(video.is_file());
    assert!(directory.join(composite.id.to_string()).is_dir());
    let joined = std::fs::metadata(&video).unwrap().modified().unwrap();

    server.clock.advance(Duration::seconds(31));
    assert_eq!(server.process().await, 1);
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Ready);
    assert_eq!(
        std::fs::metadata(&video).unwrap().modified().unwrap(),
        joined
    );
    assert!(!directory.join(composite.id.to_string()).exists());
    let leftovers: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".partial"))
        .collect();
    assert_eq!(leftovers, Vec::<String>::new());
}

/// The recorder reports the name of each file. The server never uses it as a path.
#[cfg(unix)]
#[tokio::test]
async fn a_link_in_the_place_of_a_file_is_not_read() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    let (_, tracks) = parts(&stopped);
    let directory = server.recordings_dir().join(started.id.to_string());
    let track = directory.join(format!("{}.ogg", tracks[0].id));
    let other = directory.join(format!("{}.ogg", tracks[1].id));
    std::fs::remove_file(&track).unwrap();
    std::os::unix::fs::symlink(&other, &track).unwrap();

    // The link is no file of the recording: the attempt does not end, and the last attempt
    // records no file for the segment.
    for _ in 0..3 {
        server.process().await;
        server.clock.advance(Duration::seconds(31));
    }
    let done = server.recording(&scene.ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (_, tracks) = parts(&done);
    assert!(tracks[0].file.is_none());
    assert!(tracks[1].file.is_some());
}

/// A screen share with sound gives a third kind of segment, and its file is checked as the
/// microphone files are.
#[tokio::test]
async fn the_sound_of_a_screen_share_is_a_file_of_its_own() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    server.publish(
        &studio.ann_seat,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio,
    );
    let done = studio.finished().await;
    assert_eq!(done.recording.state, RecordingState::Ready);
    let shared: Vec<_> = done
        .segments
        .iter()
        .filter(|segment| segment.source == SegmentSource::ScreenShareAudio)
        .collect();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].account_id, Some(studio.scene.ann.account_id));
    assert_eq!(shared[0].file.as_ref().unwrap().media_type, "audio/ogg");
}

/// The state `ready` reaches the people who see the voice channel after its call has ended,
/// also a person who was never in the call (ADR 0604).
#[tokio::test]
async fn the_final_state_is_an_event_for_the_people_who_see_the_channel() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    studio.stop(started.id).await;
    server.leave_call(&scene.ann).await;
    server.leave_call(&scene.ben).await;
    let outsider = server.join(owner, "Cy").await;
    let mut stream = server.full_stream(&outsider).await;

    assert_eq!(server.process().await, 1);
    let events: Vec<Event> = server
        .traffic(&outsider, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    let [
        Event::MessageChanged {
            message_id,
            deleted: false,
            ..
        },
        Event::RecordingUpdated { recording },
    ] = &events[..]
    else {
        panic!("{events:?}");
    };
    let post: junto_protocol::message::Message = server
        .get(&outsider, &format!("/api/v1/messages/{message_id}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(post.system.unwrap().recording_id, Some(started.id));
    assert_eq!(recording.state, RecordingState::Ready);
    assert_eq!(recording.id, started.id);
}

#[tokio::test]
async fn a_server_without_a_recorder_processes_nothing() {
    let server = TestServer::start().await;
    assert_eq!(server.process().await, 0);
    assert!(!server.transcribe().await);
}

/// With its tasks running, the server processes a recording when its last job has ended and
/// hands it to the worker when its files are processed. The intervals of both tasks are one
/// hour here, so only those two changes make them run.
#[tokio::test]
async fn the_tasks_process_and_transcribe_a_recording_after_its_stop() {
    use crate::recordings_support::{FakeWorker, Said, transcript_of, until};

    let worker = FakeWorker::new();
    worker.behave("while [ ! -f \"$here/transcript.json\" ]; do sleep 0.05; done");
    let settings = worker.settings();
    let server = TestServer::start_with_tasks(|config| {
        config.recorder.as_mut().unwrap().transcriber = Some(settings);
        config.limits.recordings.process_interval = Duration::hours(1);
        config.limits.recordings.transcription.check_interval = Duration::hours(1);
    })
    .await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);

    let started = studio.record().await;
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    until("the files of the recording are processed", async || {
        server.recording(ann, started.id).await.recording.state == RecordingState::Ready
    })
    .await;
    until("the worker runs", async || {
        server.recording(ann, started.id).await.recording.transcript == TranscriptState::Running
    })
    .await;
    let done = server.recording(ann, started.id).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 500, 1500, "Good morning.", None)],
    ));
    until("the transcript is stored", async || {
        server.recording(ann, started.id).await.recording.transcript == TranscriptState::Ready
    })
    .await;
    assert_eq!(server.transcript_lines(ann, started.id, 200).await.len(), 1);
}

/// The audio that was saved of a lost job is deleted only when the server has taken it. A
/// directory of a recording that the server cannot write to keeps the saved files where
/// they are.
#[cfg(unix)]
#[tokio::test]
async fn saved_audio_that_could_not_be_taken_is_not_deleted() {
    use std::os::unix::fs::PermissionsExt;

    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    server.recorder().rescue_files(true);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(15));
    server.recorder().kill();
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    server.recorder().participants_gone();
    server.tend().await;
    let root = server.recordings_dir();
    let directory = root.join(started.id.to_string());
    let jobs = server.recorder().jobs();
    let lost = server.recording(ann, started.id).await;
    let (_, tracks) = parts(&lost);
    let saved: Vec<_> = (1..3)
        .flat_map(|job| {
            let directory = root.join("rescued").join(&jobs[job].id);
            tracks
                .iter()
                .map(move |track| directory.join(format!("{}.ogg", track.id)))
        })
        .filter(|path| path.is_file())
        .collect();
    assert_eq!(saved.len(), 2);

    let mode = |mode| std::fs::set_permissions(&directory, PermissionsExt::from_mode(mode));
    mode(0o500).unwrap();
    for _ in 0..3 {
        server.process().await;
        server.clock.advance(Duration::seconds(31));
    }
    server.process().await;
    mode(0o700).unwrap();
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Failed);
    assert!(done.segments.iter().all(|segment| segment.file.is_none()));
    for path in &saved {
        assert!(path.is_file(), "{} was deleted", path.display());
    }

    // The saved audio goes with the recording when a person removes it.
    server
        .delete(&scene.owner, &format!("/api/v1/recordings/{}", started.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.process().await;
    for path in &saved {
        assert!(!path.exists(), "{} was left", path.display());
    }
    assert!(!directory.exists());
    assert_eq!(server.count("file_removals").await, 0);
}

/// The process that holds the attempt deletes the unfinished videos that it finds: an
/// attempt that is over left them. The file of an attempt that another process holds stays
/// (`an_attempt_of_another_server_process_is_left_alone_until_that_process_is_gone`).
#[tokio::test]
async fn an_unfinished_video_of_an_earlier_attempt_is_deleted() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    let (composite, _) = parts(&stopped);
    let directory = server.recordings_dir().join(started.id.to_string());
    let left = directory.join(format!("{}.22222222.partial", composite.id));
    std::fs::write(&left, b"a stopped process left this").unwrap();

    assert_eq!(server.process().await, 1);
    let done = server.recording(&scene.ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Ready);
    assert!(!left.exists());
}

/// A server that stops during every attempt has used the attempts of the recording. The
/// next start finishes the recording with what is on disk and runs no further examination.
#[tokio::test]
async fn a_recording_without_an_attempt_left_is_finished_without_another_examination() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    sqlx::query("update recordings set processing_attempts = 3")
        .execute(&server.db)
        .await
        .unwrap();

    assert_eq!(server.process().await, 1);
    let done = server.recording(&scene.ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (composite, tracks) = parts(&done);
    for track in tracks {
        assert_eq!(problems(track), [FileProblem::NotChecked]);
    }
    // The video was never joined, and no attempt joins it now.
    assert!(composite.file.is_none());
    let (stopped_composite, _) = parts(&stopped);
    let directory = server.recordings_dir().join(started.id.to_string());
    assert!(
        !directory
            .join(format!("{}.mp4", stopped_composite.id))
            .exists()
    );
    assert_eq!(server.count("storage_reservations").await, 0);
    assert_eq!(server.process().await, 0);
}

/// One server process holds an attempt. Another process leaves the recording alone, also
/// when the attempts are used: the process that holds the last attempt can still finish it.
/// When that process is gone, its attempt is over.
#[tokio::test]
async fn an_attempt_of_another_server_process_is_left_alone_until_that_process_is_gone() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    let started = studio.record().await;
    let stopped = studio.stop(started.id).await;
    let (composite, _) = parts(&stopped);
    let directory = server.recordings_dir().join(started.id.to_string());
    let joining = directory.join(format!("{}.11111111.partial", composite.id));
    std::fs::write(&joining, b"another process writes this").unwrap();
    // Another server process runs the third attempt.
    sqlx::query(
        "update recordings
         set processing_attempts = 3, processing_by = $1, processing_heartbeat_at = $2",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(server.clock.now())
    .execute(&server.db)
    .await
    .unwrap();

    assert_eq!(server.process().await, 0);
    server.clock.advance(Duration::seconds(89));
    assert_eq!(server.process().await, 0);
    let waiting = server.recording(&scene.ann, started.id).await;
    assert_eq!(waiting.recording.state, RecordingState::Processing);
    assert!(joining.is_file());

    // The heartbeat of that process is old: the process is gone, and no attempt is left.
    server.clock.advance(Duration::seconds(2));
    assert_eq!(server.process().await, 1);
    let done = server.recording(&scene.ann, started.id).await;
    assert_eq!(done.recording.state, RecordingState::PartlyFailed);
    let (composite, tracks) = parts(&done);
    assert!(composite.file.is_none());
    assert!(
        tracks
            .iter()
            .all(|track| problems(track) == [FileProblem::NotChecked])
    );
}
