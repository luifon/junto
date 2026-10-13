//! Recordings and transcripts of the client core against the real server, which runs
//! inside the test process with its recorder in memory: the files of the recording are
//! real media files that the server processed.

use std::path::Path;
use std::sync::Arc;

use junto_client_core::ServerHandle;
use junto_client_core::api::ErrorCategory;
use junto_client_core::flow::access::AccessAction;
use junto_client_core::model::{OpenRecording, ServerModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::RecordingId;
use junto_client_core::protocol::recording::{
    FileCheck, RecordingSegment, RecordingState, SegmentSource, SegmentState,
};
use junto_client_core::protocol::secret::OwnerCode;
use junto_client_core::protocol::structure::{ChannelKind, SetChannelOverride};
use junto_client_core::protocol::transcript::{LineSource, TranscriptState};
use junto_client_core::transfers::{DownloadStage, TransferFailure, segment_file_name};

use crate::recording_server::{RecordingServer, Spoken};
use crate::stream::{channel, live, model_when, pair};
use crate::support::{Device, TestServer, join, logged_in};
use crate::threads::seed_transcript;

async fn recording_when(
    handle: &ServerHandle,
    id: RecordingId,
    what: &str,
    mut holds: impl FnMut(&OpenRecording) -> bool,
) -> Arc<ServerModel> {
    model_when(handle, what, |model| {
        model.recording(id).is_some_and(|open| holds(open))
    })
    .await
}

/// The file of a segment in the directory of the recordings of the server.
fn on_disk(server: &RecordingServer, id: RecordingId, segment: &RecordingSegment) -> Vec<u8> {
    let ending = match segment.source {
        SegmentSource::Composite => "mp4",
        _ => "ogg",
    };
    let directory = server.recordings_dir().join(id.to_string());
    std::fs::read(directory.join(format!("{}.{ending}", segment.id))).unwrap()
}

/// Runs `junto-cli recordings` to its end with the data directory of a device, and returns
/// what it printed.
fn tool(device: &Device, arguments: &[&str]) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_junto-cli"))
        .arg("--data-dir")
        .arg(device.directory.path().join("junto"))
        .arg("recordings")
        .args(arguments)
        .env_remove("JUNTO_DATA_DIR")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let printed = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{arguments:?}: {printed}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    printed
}

fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_reads_a_recording_its_files_and_its_transcript_and_follows_another_client() {
    let server = RecordingServer::start("Recordings").await;
    let (first, second) = (Device::new(), Device::new());
    let owner = first.add(&server.address()).await;
    let claim = AccessAction::ClaimOwner {
        code: OwnerCode::new(server.owner_code()),
        name: "Ada".to_owned(),
        device_name: "First device".to_owned(),
    };
    owner.dispatch(claim).await.unwrap();
    let code = crate::invite(&owner, 2).await;
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let (owner_id, member_id) = (logged_in(&owner), logged_in(&member));
    let workspace_id = owner.api().workspaces().await.unwrap()[0].id;
    live(&first, &owner).await;
    live(&second, &member).await;
    let voice = channel(&owner, workspace_id, "Daily", ChannelKind::Voice).await;
    model_when(&member, "the voice channel", |model| {
        model.channel(voice.id).is_some()
    })
    .await;

    // Ada and Grace are in a call. Ada shares the sound of a screen and records.
    let id = server.record(voice.id, &[&owner, &member]).await;
    let page = member
        .api()
        .recordings(workspace_id, None, None)
        .await
        .unwrap();
    assert_eq!(page.next_before, None);
    assert_eq!(page.recordings.len(), 1);
    assert_eq!(page.recordings[0].id, id);

    // The member opens the recording: its segments and its transcript are in the state.
    member.live().open_recording(id).await.unwrap();
    let model = member.live().view().model.unwrap();
    let open = model.recording(id).unwrap();
    let detail = open.detail.loaded().unwrap().clone();
    assert_eq!(detail, member.api().recording(id).await.unwrap());
    assert_eq!(detail.recording.state, RecordingState::Ready);
    assert_eq!(detail.recording.channel_id, voice.id);
    assert_eq!(detail.recording.started_by, owner_id);
    let sources: Vec<SegmentSource> = detail
        .segments
        .iter()
        .map(|segment| segment.source)
        .collect();
    assert_eq!(sources.len(), 4, "{sources:?}");
    for source in [
        SegmentSource::Composite,
        SegmentSource::Microphone,
        SegmentSource::ScreenShareAudio,
    ] {
        assert!(sources.contains(&source), "{sources:?}");
    }
    // The server has a worker, and the transcript waits for it.
    let header = open.transcript.loaded().unwrap();
    assert_eq!((header.state, header.lines), (TranscriptState::Waiting, 0));
    assert!(!open.lines.more);

    // Each file is downloaded and is the file that the server holds.
    let target = tempfile::tempdir().unwrap();
    let transfers = member.transfers();
    for (number, segment) in detail.segments.iter().enumerate() {
        let file = segment.file.as_ref().unwrap();
        assert_eq!(file.check, FileCheck::Passed);
        let stem = format!("Daily {number}");
        let download = transfers
            .download_segment(id, segment, &stem, target.path().into())
            .unwrap();
        let saved = download.finished().await.unwrap();
        assert_eq!(saved, target.path().join(segment_file_name(&stem, segment)));
        let held = on_disk(&server, id, segment);
        assert_eq!(held.len() as u64, file.bytes);
        assert!(std::fs::read(&saved).unwrap() == held, "{stem}");
        let progress = download.progress();
        assert_eq!(progress.stage, DownloadStage::Saved(saved));
        assert_eq!(
            (progress.received, progress.bytes),
            (file.bytes, file.bytes)
        );
    }
    assert_eq!(names(target.path()).len(), 4);

    // A download that was cut continues in its unfinished file: the server sends the
    // bytes after the ones that the file holds. The file of a recording has no hash, so
    // the first bytes stay as the unfinished file had them.
    let video = &detail.segments[0];
    assert_eq!(video.source, SegmentSource::Composite);
    let held = on_disk(&server, id, video);
    let half = held.len() / 2;
    let part = format!("Daily again.mp4.{}.part", video.id.as_uuid().simple());
    std::fs::write(target.path().join(&part), vec![0u8; half]).unwrap();
    let again = transfers
        .download_segment(id, video, "Daily again", target.path().into())
        .unwrap();
    let saved = again.finished().await.unwrap();
    let continued = std::fs::read(&saved).unwrap();
    assert_eq!(continued.len(), held.len());
    assert!(continued[..half].iter().all(|byte| *byte == 0));
    assert!(continued[half..] == held[half..]);
    assert!(!target.path().join(&part).exists());

    // The worker writes the transcript. The open recording follows `recording_updated`.
    let spoken = |account_id, speaker, start_ms, end_ms, text: &str| Spoken {
        account_id,
        speaker,
        start_ms,
        end_ms,
        text: text.to_owned(),
    };
    server
        .transcribe(
            id,
            &[
                spoken(owner_id, Some(1), 100, 900, "Thank you all for joining."),
                spoken(member_id, None, 300, 800, "Hello."),
                spoken(owner_id, None, 1000, 1400, "Can you hear me?"),
            ],
        )
        .await;
    let model = recording_when(&member, id, "the transcript", |open| {
        open.transcript
            .loaded()
            .is_some_and(|header| header.state == TranscriptState::Ready)
            && open
                .detail
                .loaded()
                .is_some_and(|detail| detail.recording.transcript == TranscriptState::Ready)
    })
    .await;
    let open = model.recording(id).unwrap();
    let header = open.transcript.loaded().unwrap();
    assert_eq!(*header, member.api().transcript(id).await.unwrap());
    assert_eq!(header.lines, 3);
    assert_eq!(header.speakers.len(), 1);
    assert_eq!(
        (header.speakers[0].number, header.speakers[0].account_id),
        (1, owner_id)
    );
    assert_eq!(open.detail.loaded().unwrap().recording.transcript_lines, 3);
    // The transcript has lines that the state does not hold. A reader asks for them.
    assert!(open.lines.more && open.lines.lines.is_empty());
    member.live().read_transcript_lines(id);
    let model = recording_when(&member, id, "the lines", |open| open.lines.loaded).await;
    let lines = &model.recording(id).unwrap().lines;
    assert!(!lines.more && lines.failed.is_none());
    let texts: Vec<&str> = lines.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        texts,
        ["Thank you all for joining.", "Hello.", "Can you hear me?"]
    );
    assert_eq!(lines.lines[0].source, LineSource::Shared);
    assert_eq!(lines.lines[0].speaker, Some(1));
    assert_eq!(lines.lines[1].account_id, member_id);

    // The owner corrects a line and names the voice. The state of the member follows
    // both events, and holds what the routes answer.
    let corrected = owner
        .api()
        .correct_transcript_line(id, 1, "Hello, everyone.")
        .await
        .unwrap();
    assert_eq!(corrected.original_text.as_deref(), Some("Hello."));
    let model = recording_when(&member, id, "the correction", |open| {
        open.line(1).is_some_and(|line| line.corrected_by.is_some())
    })
    .await;
    assert_eq!(**model.recording(id).unwrap().line(1).unwrap(), corrected);
    let named = owner
        .api()
        .rename_transcript_speaker(id, 1, "Noor")
        .await
        .unwrap();
    assert_eq!(named.renamed_by, Some(owner_id));
    let model = recording_when(&member, id, "the name of the voice", |open| {
        open.speaker(1)
            .is_some_and(|speaker| speaker.name.is_some())
    })
    .await;
    let open = model.recording(id).unwrap();
    assert_eq!(open.speaker(1), Some(&named));
    let page = member.api().transcript_lines(id, None, None).await.unwrap();
    let held: Vec<_> = open
        .lines
        .lines
        .iter()
        .map(|line| line.as_ref().clone())
        .collect();
    assert_eq!(held, page.lines);
    assert_eq!(page.next_after, None);
    let after_first = member.api().transcript_lines(id, Some(0), Some(1)).await;
    let after_first = after_first.unwrap();
    assert_eq!(after_first.lines.len(), 1);
    assert_eq!(after_first.next_after, Some(1));

    // The transcript as text, and as a file.
    let text = member.api().transcript_text(id).await.unwrap();
    assert_eq!(
        text,
        "[00:00:00] Noor (shared audio of Ada): Thank you all for joining.\n\
         [00:00:00] Grace: Hello, everyone.\n\
         [00:00:01] Ada: Can you hear me?\n"
    );
    let download = transfers.download_transcript_text(id, "Daily", target.path().into());
    let saved = download.finished().await.unwrap();
    assert_eq!(saved, target.path().join("Daily.txt"));
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), text);

    // The tool reads the same routes with the account of the member.
    let recording = id.to_string();
    let listed = tool(&second, &["list", &workspace_id.to_string()]);
    assert!(listed.contains(&format!("recording: {id}")), "{listed}");
    assert!(listed.ends_with("recordings: 1\n"), "{listed}");
    let shown = tool(&second, &["show", &recording]);
    assert_eq!(shown.matches("  segment: ").count(), 4, "{shown}");
    assert!(shown.contains("transcript: ready  lines: 3"), "{shown}");
    let header = tool(&second, &["transcript", &recording]);
    assert!(header.contains("state: ready  lines: 3"), "{header}");
    assert!(header.contains("name: Noor"), "{header}");
    let page = tool(
        &second,
        &["lines", &recording, "--after", "0", "--limit", "1"],
    );
    assert!(page.contains("  Hello, everyone.\n"), "{page}");
    assert!(page.contains("the worker wrote: Hello.\n"), "{page}");
    assert!(
        page.ends_with("lines: 1\ncontinue with: --after 1\n"),
        "{page}"
    );
    assert_eq!(tool(&second, &["text", &recording]), text);
    let fixed = tool(
        &second,
        &["correct", &recording, "2", "--text", "Can you all hear me?"],
    );
    assert!(fixed.contains("  Can you all hear me?\n"), "{fixed}");
    let renamed = tool(
        &second,
        &["rename-speaker", &recording, "1", "--name", "Noor E."],
    );
    assert!(renamed.contains("speaker: 1"), "{renamed}");
    assert!(renamed.contains("name: Noor E."), "{renamed}");
    let named = recording_when(&member, id, "the changes of the tool", |open| {
        open.line(2)
            .is_some_and(|line| line.text == "Can you all hear me?")
            && open
                .speaker(1)
                .is_some_and(|held| held.name.as_deref() == Some("Noor E."))
    })
    .await
    .recording(id)
    .unwrap()
    .speaker(1)
    .unwrap()
    .clone();
    let by_tool = tempfile::tempdir().unwrap();
    let to = by_tool.path().to_str().unwrap();
    let audio = detail.segments.last().unwrap();
    let printed = tool(
        &second,
        &["download", &recording, &audio.id.to_string(), "--to", to],
    );
    let saved = printed.trim_end().strip_prefix("saved: ").unwrap();
    assert!(std::fs::read(saved).unwrap() == on_disk(&server, id, audio));
    let printed = tool(&second, &["text", &recording, "--to", to]);
    let saved = printed.trim_end().strip_prefix("saved: ").unwrap();
    let written = std::fs::read_to_string(saved).unwrap();
    assert!(
        written.contains("Noor E. (shared audio of Ada)"),
        "{written}"
    );
    assert_eq!(names(by_tool.path()).len(), 2);

    // The channel is hidden from the members: the state of the member is read again, and
    // the server has no such recording for the member any more.
    let everyone = owner.live().view().model.unwrap().workspaces[0]
        .roles
        .iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    owner
        .api()
        .set_channel_override(voice.id, everyone, &hide)
        .await
        .unwrap();
    let model = recording_when(&member, id, "the lost access", |open| open.gone).await;
    assert!(model.channel(voice.id).is_none());
    let unread = member.api().recording(id).await.unwrap_err();
    assert_eq!(unread.category(), Some(ErrorCategory::NotFound));
    let segment_download = transfers
        .download_segment(id, video, "Hidden", target.path().into())
        .unwrap();
    assert!(matches!(
        segment_download.finished().await,
        Err(TransferFailure::Refused {
            code: ErrorCode::NotFound,
            ..
        })
    ));
    // The channel is shown again: the reload reads the open recording again.
    let show = SetChannelOverride {
        allow: Vec::new(),
        deny: Vec::new(),
    };
    owner
        .api()
        .set_channel_override(voice.id, everyone, &show)
        .await
        .unwrap();
    let model = recording_when(&member, id, "the recording again", |open| {
        !open.gone && !open.stale && open.detail.loaded().is_some()
    })
    .await;
    let open = model.recording(id).unwrap();
    assert_eq!(open.speaker(1), Some(&named));
    assert!(open.lines.more && !open.lines.loaded);

    // Only a person who manages recordings removes one. The member sees the removal.
    let refused = member.api().remove_recording(id).await.unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    owner.live().open_recording(id).await.unwrap();
    owner.api().remove_recording(id).await.unwrap();
    for handle in [&owner, &member] {
        let model = recording_when(handle, id, "the removal", |open| open.gone).await;
        assert!(model.recording(id).unwrap().lines.lines.is_empty());
    }
    let page = member
        .api()
        .recordings(workspace_id, None, None)
        .await
        .unwrap();
    assert!(page.recordings.is_empty());
    member.live().release_recording(id);
    model_when(&member, "no open recording", |model| {
        model.recording(id).is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_reads_the_recordings_of_its_workspace_page_by_page_and_of_no_other_workspace() {
    let server = TestServer::start("Recordings").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let none = member
        .api()
        .recordings(workspace_id, None, None)
        .await
        .unwrap();
    assert!(none.recordings.is_empty());
    assert_eq!(none.next_before, None);

    let voice = channel(owner, workspace_id, "Daily", ChannelKind::Voice).await;
    let older = RecordingId::from_uuid(seed_transcript(&scene, voice.id, "the first call").await);
    let newer = RecordingId::from_uuid(seed_transcript(&scene, voice.id, "the second call").await);
    let page = member
        .api()
        .recordings(workspace_id, None, None)
        .await
        .unwrap();
    let listed: Vec<RecordingId> = page.recordings.iter().map(|held| held.id).collect();
    assert_eq!(listed, [newer, older]);
    assert_eq!(page.next_before, None);
    let first = &page.recordings[0];
    assert_eq!(first.workspace_id, workspace_id);
    assert_eq!(first.channel_id, voice.id);
    assert_eq!(first.state, RecordingState::Ready);
    assert_eq!(first.started_by, logged_in(owner));
    assert_eq!(first.language, "auto");
    assert_eq!((first.segments, first.failed_segments), (1, 0));
    assert_eq!(first.transcript, TranscriptState::Ready);
    assert_eq!(first.transcript_lines, 1);
    assert!(first.ended_at.is_some());
    assert_eq!(
        owner
            .api()
            .recordings(workspace_id, None, None)
            .await
            .unwrap(),
        page
    );

    let api = member.api();
    let head = api.recordings(workspace_id, None, Some(1)).await.unwrap();
    assert_eq!(head.recordings, page.recordings[..1]);
    assert_eq!(head.next_before, Some(newer));
    let rest = api
        .recordings(workspace_id, head.next_before, Some(1))
        .await;
    let rest = rest.unwrap();
    assert_eq!(rest.recordings, page.recordings[1..]);
    assert_eq!(rest.next_before, None);

    let detail = api.recording(newer).await.unwrap();
    assert_eq!(&detail.recording, first);
    assert_eq!(detail.segments.len(), 1);
    assert_eq!(detail.segments[0].source, SegmentSource::Composite);
    assert_eq!(detail.segments[0].state, SegmentState::Ended);
    assert_eq!(detail.segments[0].file, None);

    // What the server refuses.
    let refused = api
        .recordings(workspace_id, None, Some(0))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest), "{refused}");
    let unknown = api.recording(RecordingId::generate()).await.unwrap_err();
    assert_eq!(unknown.category(), Some(ErrorCategory::NotFound));

    // A workspace that the account is not a member of is answered as an absent one.
    let owner_api = owner.api();
    let board = owner_api.create_workspace("Board".to_owned()).await;
    let board = board.unwrap();
    let elsewhere = api.recordings(board.id, None, None).await.unwrap_err();
    assert_eq!(elsewhere.code(), Some(ErrorCode::NotFound), "{elsewhere}");
    assert_eq!(elsewhere.category(), Some(ErrorCategory::NotFound));
    let seen_by_owner = owner_api.recordings(board.id, None, None).await.unwrap();
    assert!(seen_by_owner.recordings.is_empty());
}
