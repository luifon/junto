use crate::discovery_support::*;
#[tokio::test]
async fn rule_39_41_search_finds_threads_and_directs_with_channel_author_date_filters() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "quartz plan").await;
    let child = reply(&server, &reader, root.id, "quartz reply").await;
    let conversation: Conversation = server
        .post(
            &reader,
            "/api/v1/conversations",
            &json!({"account_id":owner.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let direct: Message = server
        .post(
            &reader,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("quartz direct"),
        )
        .await
        .ok(StatusCode::CREATED);
    let hits = search(&server, &reader, "quartz").await.results;
    assert_eq!(hits.len(), 3);
    for id in [root.id, child.id, direct.id] {
        assert!(hits.iter().any(|hit| hit.message_id == Some(id)));
    }
    let hits = search(
        &server,
        &reader,
        &format!(
            "quartz&channel_id={}&author_id={}",
            channel.id, reader.account_id
        ),
    )
    .await
    .results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, Some(child.id));
    let now = server
        .clock
        .now()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    assert_eq!(
        search(&server, &reader, &format!("quartz&from={now}"))
            .await
            .results
            .len(),
        3
    );
    assert!(
        search(&server, &reader, &format!("quartz&until={now}"))
            .await
            .results
            .is_empty()
    );
}

#[tokio::test]
async fn rule_40_transcripts_search_current_corrections_and_speaker() {
    use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 0, 500, "quartz transcript", None)],
    ));
    assert!(server.transcribe().await);
    let hits = search(server, owner, "quartz").await.results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].recording_id, Some(done.recording.id));
    assert_eq!(
        search(
            server,
            owner,
            &format!("quartz&author_id={}", hits[0].author_id)
        )
        .await
        .results
        .len(),
        1
    );
    server
        .put(
            owner,
            &format!(
                "/api/v1/recordings/{}/transcript/lines/0",
                done.recording.id
            ),
            &json!({"text":"amethyst correction"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(search(server, owner, "quartz").await.results.is_empty());
    assert_eq!(
        search(server, owner, "amethyst").await.results[0].text,
        "amethyst correction"
    );
    server
        .call_settings(
            owner,
            studio.scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    assert!(search(server, owner, "amethyst").await.results.is_empty());
    assert_eq!(
        search(server, &studio.scene.ann, "amethyst")
            .await
            .results
            .len(),
        1
    );
    let reader = server.join(owner, "Search reader").await;
    let public = channel(server, owner, "text").await;
    let visible = mention(server, owner, &public, &reader).await;
    let private = channel(server, owner, "text").await;
    let everyone = server.everyone_role(owner, private.workspace_id).await;
    server
        .put(
            owner,
            &format!("/api/v1/channels/{}/overrides/{}", private.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let hidden = mention(server, owner, &private, &reader).await;
    let conversation: Conversation = server
        .post(
            &studio.scene.ann,
            "/api/v1/conversations",
            &json!({"account_id":studio.scene.ben.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    server
        .post(
            &studio.scene.ann,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("quartz amethyst direct"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    let hits = search(server, &reader, "quartz%20OR%20amethyst")
        .await
        .results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, Some(visible.id));
    let notices = notices(server, &reader).await;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].message_id, visible.id);
    assert!(!notices.iter().any(|n| n.message_id == hidden.id));
}

#[tokio::test]
async fn rule_42_ranked_pages_are_stable_and_invalid_queries_refuse() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let mut expected = Vec::new();
    for _ in 0..3 {
        expected.push(send(&server, &owner, &channel, "quartz").await.id);
    }
    expected.sort_by(|a, b| b.cmp(a));
    let mut cursor = String::new();
    let mut ids = Vec::new();
    let mut last_rank = f32::MAX;
    loop {
        let page = search(&server, &owner, &format!("quartz&limit=1{cursor}")).await;
        assert_eq!(page.results.len(), 1);
        assert!(page.results[0].rank <= last_rank);
        last_rank = page.results[0].rank;
        ids.push(page.results[0].message_id.unwrap());
        if let Some(next) = page.next {
            cursor = format!("&cursor={next}");
        } else {
            break;
        }
    }
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(unique.len(), 3);
    assert_eq!(ids, expected);
    for query in [
        "q=",
        "q=quartz&limit=0",
        "q=quartz&limit=201",
        "q=quartz&cursor=NaN:x:0",
        "q=quartz&from=wrong",
    ] {
        server
            .get(&owner, &format!("/api/v1/search?{query}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    server
        .get(&owner, &format!("/api/v1/search?q={}", "x".repeat(1025)))
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn rule_43_44_45_search_is_current_and_hides_other_channels_and_conversations() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &author, &channel, "quartz public").await;
    assert_eq!(search(&server, &reader, "quartz").await.results.len(), 1);
    server
        .put(&author, &message(root.id), &text("amethyst edited"))
        .await
        .ok::<Message>(StatusCode::OK);
    assert!(search(&server, &reader, "quartz").await.results.is_empty());
    assert_eq!(search(&server, &reader, "amethyst").await.results.len(), 1);
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    server
        .post(
            &author,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("amethyst private"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    assert_eq!(search(&server, &owner, "amethyst").await.results.len(), 1);
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let hits = search(&server, &reader, "amethyst").await.results;
    assert_eq!(hits.len(), 1);
    assert!(hits[0].place_id == conversation.id.as_uuid());
    server
        .delete(&owner, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(search(&server, &owner, "amethyst").await.results.is_empty());
}

#[tokio::test]
async fn rule_46_search_statement_count_is_independent_of_page_size() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "quartz").await;
    sqlx::query("create extension pg_stat_statements")
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select
             gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join
             generate_series(1,199) n where id=$1")
        .bind(root.id)
        .execute(&server.db)
        .await
        .unwrap();
    let (mut held, role) = crate::messages_queries::measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    search(&server, &owner, "quartz&limit=200").await;
    let before = crate::messages_queries::statements(observer, role).await;
    assert_eq!(
        search(&server, &owner, "quartz&limit=1")
            .await
            .results
            .len(),
        1
    );
    let small = crate::messages_queries::since(
        crate::messages_queries::statements(observer, role).await,
        before,
    );
    let before = crate::messages_queries::statements(observer, role).await;
    assert_eq!(
        search(&server, &owner, "quartz&limit=200")
            .await
            .results
            .len(),
        200
    );
    let large = crate::messages_queries::since(
        crate::messages_queries::statements(observer, role).await,
        before,
    );
    assert!(!small.is_empty());
    assert_eq!(small, large);
}

#[tokio::test]
async fn a_search_result_holds_the_text_of_the_message_once() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "quartz plan").await;

    let hits = search(&server, &owner, "quartz").await.results;

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, Some(sent.id));
    assert_eq!(hits[0].text, "quartz plan");
}

#[tokio::test]
async fn a_search_query_with_a_nul_byte_is_an_invalid_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    server
        .get(&owner, "/api/v1/search?q=quartz%00")
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn fix_search_statement_count_does_not_grow_per_workspace() {
    use crate::messages_queries::{measure_requests, since, statements};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    sqlx::query("create extension pg_stat_statements")
        .execute(&server.db)
        .await
        .unwrap();
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    search(&server, &owner, "quartz").await;
    let before = statements(observer, role).await;
    search(&server, &owner, "quartz").await;
    let small = since(statements(observer, role).await, before);
    for n in 0..5 {
        server
            .post(
                &owner,
                "/api/v1/workspaces",
                &json!({"name":format!("workspace {n}")}),
            )
            .await
            .ok::<Value>(StatusCode::CREATED);
    }
    search(&server, &owner, "quartz").await;
    let before = statements(observer, role).await;
    search(&server, &owner, "quartz").await;
    let large = since(statements(observer, role).await, before);
    assert_eq!(
        large, small,
        "search authorization must not issue statements per workspace"
    );
}

#[tokio::test]
async fn fix_transcript_search_uses_a_stored_indexed_vector() {
    use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 0, 500, "quartz", None)],
    ));
    assert!(server.transcribe().await);
    sqlx::query("insert into transcript_lines(recording_id,line,start_ms,end_ms,segment_id,source,account_id,speaker,text,language)
        select recording_id,line+n,start_ms,end_ms,segment_id,source,account_id,speaker,'ordinary',language
        from transcript_lines cross join generate_series(1,5000) n where recording_id=$1 and line=0")
        .bind(done.recording.id).execute(&server.db).await.unwrap();
    let root = send(server, owner, &studio.scene.voice, "quartz").await;
    sqlx::query(
        "insert into messages(id,place_id,position,author_id,created_at,document)
        select gen_random_uuid(),place_id,position+n,author_id,created_at,$2 from messages
        cross join generate_series(1,5000) n where id=$1",
    )
    .bind(root.id)
    .bind(sqlx::types::Json(text("ordinary")["document"].clone()))
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::raw_sql("analyze messages; analyze transcript_lines")
        .execute(&server.db)
        .await
        .unwrap();
    let (_held, _role) = crate::messages_queries::measure_requests(server).await;
    assert_eq!(search(server, owner, "quartz").await.results.len(), 2);
    let mut connection = server.db.acquire().await.unwrap();
    let actual:String=sqlx::query_scalar("select statement from pg_prepared_statements where statement like 'with query as%' limit 1")
        .fetch_one(&mut *connection).await.unwrap();
    sqlx::query("set enable_seqscan=off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(format json) {actual}"
    )))
    .bind("quartz")
    .bind(vec![studio.scene.voice.id.as_uuid()])
    .bind(owner.account_id)
    .bind(None::<uuid::Uuid>)
    .bind(None::<uuid::Uuid>)
    .bind(None::<time::OffsetDateTime>)
    .bind(None::<time::OffsetDateTime>)
    .bind(f32::MAX)
    .bind(uuid::Uuid::max())
    .bind(i32::MAX)
    .bind(201_i64)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    for index in ["messages_search", "transcript_lines_search"] {
        assert!(
            plan.to_string().contains(index),
            "actual search statement must use {index}: {plan}"
        );
    }
}

#[tokio::test]
async fn fix_equal_rank_transcript_lines_page_in_order_and_unready_text_is_hidden() {
    use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[
            Said(0, 0, 500, "quartz", None),
            Said(0, 600, 1000, "quartz", None),
        ],
    ));
    assert!(server.transcribe().await);
    let first = search(server, owner, "quartz&limit=1").await;
    assert_eq!(first.results[0].line, Some(1));
    let next = search(
        server,
        owner,
        &format!("quartz&limit=1&cursor={}", first.next.unwrap()),
    )
    .await;
    assert_eq!(next.results[0].line, Some(0));
    assert_eq!(first.results[0].rank, next.results[0].rank);
    assert!(next.next.is_none());
    for state in ["waiting", "running", "failed"] {
        sqlx::query("update transcripts set state=$2,lines=0,finished_at=case when $2= 'failed' then requested_at
             end,failure=case when $2= 'failed' then 'interrupted' end,claimed_by=case when $2= 'running' then
             gen_random_uuid() end,claimed_at=case when $2= 'running' then requested_at end,heartbeat_at=case when $2=
             'running' then requested_at end where recording_id=$1")
            .bind(done.recording.id)
            .bind(state)
            .execute(&server.db)
            .await
            .unwrap();
        assert!(
            search(server, owner, "quartz").await.results.is_empty(),
            "state {state}"
        );
    }
}
