//! Proofs of the audit of the whole server: the event log. Each test fails while its defect
//! exists.

use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_server::recorder::samples;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, names};
use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};

/// ADR 0408 keeps the text of a message out of the event log, so that the log does not
/// hold a text after its deletion. The event of a transcript correction carries its text.
/// After the recording is removed, the log still delivers that text to every account that
/// sees the voice channel, an account that joined after the removal included.
#[tokio::test]
async fn audit_the_text_of_a_removed_recording_is_not_replayed_from_the_event_log() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ben) = (&scene.server, &scene.owner, &scene.ben);
    let done = studio
        .finished_after(Duration::seconds(i64::from(samples::SECONDS)))
        .await;
    let id = done.recording.id;
    let said = [
        Said(0, 500, 1500, "Good morning everyone.", None),
        Said(1, 700, 1900, "Can you hear me?", None),
    ];
    worker.give(&transcript_of(&done, (&[], &[]), &said));
    assert!(server.transcribe().await);
    let before = server.snapshot(owner).await.cursor;

    server
        .put(
            ben,
            &format!("/api/v1/recordings/{id}/transcript/lines/1"),
            &json!({ "text": "the merger closes in March" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .delete(owner, &format!("/api/v1/recordings/{id}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(owner, &format!("/api/v1/recordings/{id}"))
        .await
        .error(ErrorCode::NotFound);

    let late = server.join(owner, "Late").await;
    let mut stream = server.stream(&late).await;
    stream.subscribe(Level::Full, Some(before)).await;
    let replayed = server.traffic(&late, &mut stream).await;
    let texts: Vec<&Event> = replayed
        .iter()
        .map(|(_, event)| event)
        .filter(|event| {
            serde_json::to_string(event)
                .unwrap()
                .contains("the merger closes in March")
        })
        .collect();
    assert!(
        texts.is_empty(),
        "an account that joined after the removal received: {texts:?}"
    );
}

/// ADR 0514: a voice channel that hides who is inside shows the people outside only that
/// it is in use, and ADR 0408 states for the system lines of a call that a later join
/// does not grant earlier lines. The audience of an event of a call names the channel and
/// not the call, and the delivery asks whether the account is inside the call that the
/// channel has now. An account that enters a later call of the channel subscribes with an
/// older cursor and receives the events of the call that ended before it entered.
#[tokio::test]
async fn audit_a_later_call_does_not_replay_who_was_in_an_earlier_hidden_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    let before = server.snapshot(ben).await.cursor;
    server.enter_call(ann, voice).await;
    server.leave_call(ann).await;
    server.check().await;
    assert!(!server.channel_call(ben, voice).await.in_use);

    server.enter_call(ben, voice).await;
    let mut stream = server.stream(ben).await;
    stream.subscribe(Level::Full, Some(before)).await;
    let replayed: Vec<Event> = server
        .traffic(ben, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| {
            serde_json::to_string(event)
                .unwrap()
                .contains(&ann.account_id.to_string())
        })
        .collect();
    assert!(
        replayed.is_empty(),
        "an account that was never in a call with the person received: {:?}",
        names(&replayed)
    );
}

/// ADR 0517: the events of a call that hid who is inside stay with the people who were
/// inside it. A viewer of the channel does not replay them after the setting was turned
/// off, and the people of the later call get none of the earlier one.
#[tokio::test]
async fn audit_the_events_of_a_hidden_call_are_not_replayed_after_the_setting_is_turned_off() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    let before = server.snapshot(ben).await.cursor;
    server.enter_call(ann, voice).await;
    server
        .put_empty(ann, &format!("/api/v1/channels/{voice}/call/lock"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            owner,
            &format!("/api/v1/channels/{voice}/call/entry-requests"),
            &Value::Null,
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    server.leave_call(ann).await;
    server.check().await;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": false }))
        .await;

    let mut stream = server.stream(ben).await;
    stream.subscribe(Level::Full, Some(before)).await;
    let replayed: Vec<Event> = server
        .traffic(ben, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| {
            let text = serde_json::to_string(event).unwrap();
            text.contains(&ann.account_id.to_string())
                || text.contains(&owner.account_id.to_string())
        })
        .collect();
    assert!(
        replayed.is_empty(),
        "a viewer of the channel replayed {:?}",
        names(&replayed)
    );
}

/// ADR 0510: a connection whose account is in a call receives every event of that call at
/// any level. The list of accounts in calls that the process keeps in memory can be behind
/// the record: after a move, after a start of the process, and while the media server does
/// not answer. A background connection reads its seat in the transaction of each batch, so
/// it passes no event of its own call while the list is behind.
#[tokio::test]
async fn audit_a_background_connection_passes_no_event_of_its_call_while_the_list_is_behind() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    let mut background = server.subscribed(ben, Level::Background).await;
    server.traffic(ben, &mut background).await;

    server.state.streams.calls().leave(ben.account_id);
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let first = names(&server.call_events(ben, &mut background).await).join(", ");
    server.check().await;
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": false, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let second = names(&server.call_events(ben, &mut background).await).join(", ");
    assert_eq!(
        (first.as_str(), second.as_str()),
        ("call_participant_updated", "call_participant_updated"),
        "the change while the list was behind, and the change after the next check"
    );
}

/// A `subscribe` with an older cursor ran one statement for each mention event of the kept
/// log, also for the mentions of other accounts, so its cost grew with what other members
/// write. The number of statements of one `subscribe` does not depend on the number of
/// mention events.
#[tokio::test]
async fn audit_a_subscribe_runs_no_statement_per_mention_event_of_the_log() {
    use junto_server::events::delivery::{Start, subscribe};

    let server = crate::support::TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let other = server.join(&owner, "Other").await;
    let channel = crate::messages_support::channel(&server, &owner, "text").await;
    let who = crate::stream_review::opened(&reader);
    let mut observer = server.db.acquire().await.unwrap();
    let role: i64 =
        sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
            .fetch_one(&mut *observer)
            .await
            .unwrap();

    let mut counts = Vec::new();
    for mentions in [5, 25] {
        let start = server.snapshot(&reader).await.cursor;
        for _ in 0..mentions {
            crate::discovery_support::mention(&server, &owner, &channel, &other).await;
        }
        let before = crate::messages_queries::statements(&mut observer, role).await;
        let started = subscribe(&server.state, &who, Level::Full, Some(start))
            .await
            .unwrap();
        assert!(matches!(started, Start::Subscribed(_)));
        let run = crate::messages_queries::since(
            crate::messages_queries::statements(&mut observer, role).await,
            before,
        );
        let statements: i64 = run
            .iter()
            .filter(|(sql, _)| !sql.contains("pg_stat_statements"))
            .map(|(_, calls)| calls)
            .sum();
        counts.push(statements);
    }
    // Other connections of the test server add a few statements to either count. The 20
    // further mentions added 20 statements before the correction.
    assert!(
        counts[1] < counts[0] + 10,
        "one subscribe ran {} statements after 5 mentions of another account and {} after 25",
        counts[0],
        counts[1]
    );
    server.require_no_statement_eviction(evictions).await;
}
