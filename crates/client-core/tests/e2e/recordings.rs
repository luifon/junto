//! The recordings of a workspace as the client core reads them from the real server
//! (ADR 0604).

use junto_client_core::api::ErrorCategory;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::RecordingId;
use junto_client_core::protocol::recording::{
    RecordingQuery, RecordingState, SegmentSource, SegmentState,
};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::transcript::TranscriptState;

use crate::stream::{channel, pair};
use crate::support::{TestServer, logged_in};
use crate::threads::seed_transcript;

#[tokio::test(flavor = "multi_thread")]
async fn a_member_reads_the_recordings_of_its_workspace_page_by_page_and_of_no_other_workspace() {
    let server = TestServer::start("Recordings").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let all = RecordingQuery::default();

    let none = member.api().recordings(workspace_id, &all).await.unwrap();
    assert!(none.recordings.is_empty());
    assert_eq!(none.next_before, None);

    let voice = channel(owner, workspace_id, "Daily", ChannelKind::Voice).await;
    let older = RecordingId::from_uuid(seed_transcript(&scene, voice.id, "the first call").await);
    let newer = RecordingId::from_uuid(seed_transcript(&scene, voice.id, "the second call").await);
    let page = member.api().recordings(workspace_id, &all).await.unwrap();
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
        owner.api().recordings(workspace_id, &all).await.unwrap(),
        page
    );

    let one = |before| RecordingQuery {
        before,
        limit: Some(1),
    };
    let api = member.api();
    let head = api.recordings(workspace_id, &one(None)).await.unwrap();
    assert_eq!(head.recordings, page.recordings[..1]);
    assert_eq!(head.next_before, Some(newer));
    let rest = api.recordings(workspace_id, &one(head.next_before)).await;
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
    let no_limit = RecordingQuery {
        before: None,
        limit: Some(0),
    };
    let refused = api.recordings(workspace_id, &no_limit).await.unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest), "{refused}");
    let unknown = api.recording(RecordingId::generate()).await.unwrap_err();
    assert_eq!(unknown.category(), Some(ErrorCategory::NotFound));

    // A workspace that the account is not a member of is answered as an absent one.
    let owner_api = owner.api();
    let board = owner_api.create_workspace("Board".to_owned()).await;
    let board = board.unwrap();
    let elsewhere = api.recordings(board.id, &all).await.unwrap_err();
    assert_eq!(elsewhere.code(), Some(ErrorCode::NotFound), "{elsewhere}");
    assert_eq!(elsewhere.category(), Some(ErrorCategory::NotFound));
    let seen_by_owner = owner_api.recordings(board.id, &all).await.unwrap();
    assert!(seen_by_owner.recordings.is_empty());
}
