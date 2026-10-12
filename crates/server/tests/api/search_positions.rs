//! What a search hit states about its place, and the lines of a transcript around a line
//! (ADR 0413).

use junto_protocol::transcript::TranscriptLinePage;

use crate::discovery_support::*;
use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};

#[tokio::test]
async fn a_hit_states_the_position_and_the_thread_root_of_its_message() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "filler").await;
    let root = send(&server, &owner, &channel, "quartz root").await;
    let answer = reply(&server, &reader, root.id, "quartz reply").await;
    let conversation: Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({ "account_id": reader.account_id }),
        )
        .await
        .ok(StatusCode::OK);
    let direct: Message = server
        .post(
            &owner,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("quartz direct"),
        )
        .await
        .ok(StatusCode::CREATED);

    let hits = search(&server, &reader, "quartz").await.results;
    assert_eq!(hits.len(), 3);
    for (message, thread_root) in [(&root, None), (&answer, Some(root.id)), (&direct, None)] {
        let hit = hits
            .iter()
            .find(|hit| hit.message_id == Some(message.id))
            .expect("a hit for the message");
        assert_eq!(hit.position, Some(message.position));
        assert_eq!(hit.thread_root, thread_root);
        assert_eq!(hit.start_ms, None);
        assert_eq!(hit.line, None);
    }
    assert!(root.position > 1);
    assert_ne!(root.position, answer.position);
}

#[tokio::test]
async fn a_transcript_hit_states_the_start_of_its_line() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[
            Said(0, 0, 500, "ordinary words", None),
            Said(0, 1200, 1700, "quartz transcript", None),
        ],
    ));
    assert!(server.transcribe().await);
    let lines: TranscriptLinePage = server
        .get(
            owner,
            &format!("/api/v1/recordings/{}/transcript/lines", done.recording.id),
        )
        .await
        .ok(StatusCode::OK);
    let spoken = lines
        .lines
        .iter()
        .find(|line| line.text == "quartz transcript")
        .expect("the line");

    let hits = search(server, owner, "quartz").await.results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].recording_id, Some(done.recording.id));
    assert_eq!(hits[0].line, Some(spoken.line));
    assert_eq!(hits[0].start_ms, Some(spoken.start_ms));
    assert!(spoken.start_ms > 0);
    assert_eq!(hits[0].position, None);
    assert_eq!(hits[0].thread_root, None);
}

/// A recording with a transcript of 41 lines, at the positions 0 to 40.
async fn long_transcript(studio: &Studio, worker: &FakeWorker) -> RecordingId {
    let server = &studio.scene.server;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 0, 500, "first line", None)],
    ));
    assert!(server.transcribe().await);
    sqlx::query(
        "insert into transcript_lines
             (recording_id, line, start_ms, end_ms, segment_id, source, account_id, speaker,
              text, language)
         select recording_id, line + n, start_ms + n, end_ms + n, segment_id, source,
                account_id, speaker, 'line ' || n, language
         from transcript_lines cross join generate_series(1, 40) n
         where recording_id = $1 and line = 0",
    )
    .bind(done.recording.id)
    .execute(&server.db)
    .await
    .unwrap();
    done.recording.id
}

async fn page(
    server: &TestServer,
    member: &Member,
    id: RecordingId,
    query: &str,
) -> crate::support::Reply {
    server
        .get(
            member,
            &format!("/api/v1/recordings/{id}/transcript/lines?{query}"),
        )
        .await
}

/// A query, the first and the last line of its page, `next_after` and `previous_before`.
type Window = (&'static str, u32, u32, Option<u32>, Option<u32>);

#[tokio::test]
async fn lines_are_read_around_a_line_and_before_a_line() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let id = long_transcript(&studio, &worker).await;

    let cases: [Window; 12] = [
        ("before=41&limit=5", 36, 40, None, Some(36)),
        ("before=100&limit=5", 36, 40, None, Some(36)),
        ("before=40&limit=5", 35, 39, Some(39), Some(35)),
        ("around=20&limit=5", 18, 22, Some(22), Some(18)),
        ("around=20&limit=4", 19, 22, Some(22), Some(19)),
        ("around=1&limit=5", 0, 4, Some(4), None),
        ("around=40&limit=5", 38, 40, None, Some(38)),
        ("before=18&limit=5", 13, 17, Some(17), Some(13)),
        ("before=3&limit=5", 0, 2, Some(2), None),
        ("before=5&limit=5", 0, 4, Some(4), None),
        ("after=22&limit=5", 23, 27, Some(27), Some(23)),
        ("limit=5", 0, 4, Some(4), None),
    ];
    for (query, first, last, next_after, previous_before) in cases {
        let read: TranscriptLinePage = page(server, owner, id, query).await.ok(StatusCode::OK);
        let positions: Vec<u32> = read.lines.iter().map(|line| line.line).collect();
        let expected: Vec<u32> = (first..=last).collect();
        assert_eq!(positions, expected, "{query}");
        assert_eq!(read.next_after, next_after, "{query}");
        assert_eq!(read.previous_before, previous_before, "{query}");
    }
    let beyond: TranscriptLinePage = page(server, owner, id, "around=1000&limit=5")
        .await
        .ok(StatusCode::OK);
    assert!(beyond.lines.is_empty());
    // The page is empty, and the lines before it are read from where it would start.
    assert_eq!(
        (beyond.next_after, beyond.previous_before),
        (None, Some(998))
    );
    let last: TranscriptLinePage = page(server, owner, id, "before=998&limit=5")
        .await
        .ok(StatusCode::OK);
    assert_eq!(last.lines.last().map(|line| line.line), Some(40));
    // Positions with a gap: the page after the gap states that lines come before it.
    sqlx::query("delete from transcript_lines where recording_id = $1 and line < 10")
        .bind(id)
        .execute(&server.db)
        .await
        .unwrap();
    let first: TranscriptLinePage = page(server, owner, id, "limit=5").await.ok(StatusCode::OK);
    assert_eq!(first.lines.first().map(|line| line.line), Some(10));
    assert_eq!(first.previous_before, None);
    // An empty page before the first line states where the lines follow.
    let empty: TranscriptLinePage = page(server, owner, id, "before=10&limit=5")
        .await
        .ok(StatusCode::OK);
    assert!(empty.lines.is_empty());
    assert_eq!((empty.next_after, empty.previous_before), (Some(9), None));
    let follows: TranscriptLinePage = page(server, owner, id, "after=9&limit=2")
        .await
        .ok(StatusCode::OK);
    assert_eq!(follows.lines.first().map(|line| line.line), Some(10));

    for query in [
        "after=1&before=9",
        "around=4&before=9",
        "around=4&after=1",
        "around=4&limit=0",
        "before=4&limit=501",
        "around=x",
    ] {
        page(server, owner, id, query)
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

/// While the call of a recording runs and hides who is inside, only the people inside read
/// its lines (ADR 0604). The new parameters read by the same rule.
#[tokio::test]
async fn lines_around_a_line_follow_the_access_of_the_recording() {
    let worker = FakeWorker::new();
    let studio = Studio::hidden_on(worker.server().await).await;
    let server = &studio.scene.server;
    let (owner, ann) = (&studio.scene.owner, &studio.scene.ann);
    let id = long_transcript(&studio, &worker).await;
    let outsider = server.join(owner, "Outsider").await;

    for query in ["around=20&limit=5", "before=20&limit=5", "after=20&limit=5"] {
        page(server, ann, id, query)
            .await
            .ok::<TranscriptLinePage>(StatusCode::OK);
        for member in [owner, &outsider] {
            let reply = page(server, member, id, query).await;
            reply.error(ErrorCode::NotFound);
            assert!(!reply.text().contains("line "), "{}", reply.text());
        }
    }
}
