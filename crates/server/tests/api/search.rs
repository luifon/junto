use crate::discovery_support::*;

#[tokio::test]
async fn filters_without_text_find_captionless_pictures_and_page_authorized_results() {
    let files = Files::open().await;
    let (server, owner, channel) = (&files.server, &files.owner, &files.channel);
    let reader = server.join(owner, "Reader").await;
    let mentioned = mention(server, owner, channel, &reader).await;
    let linked = send(server, owner, channel, "https://example.test/plan").await;
    let picture = upload(
        server,
        owner,
        json!({"channel_id": channel.id}),
        &junto_server::image::samples::png(4, 4),
    )
    .await;
    let picture: Message = server
        .post(
            owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":{"blocks":[]},"attachments":[picture.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
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
            &text("Private notes"),
        )
        .await
        .ok(StatusCode::CREATED);
    for (filter, expected) in [
        ("has_file", picture.id),
        ("has_picture", picture.id),
        ("has_link", linked.id),
        ("mentions_me", mentioned.id),
        ("direct", direct.id),
    ] {
        let page = search(server, &reader, &format!("&{filter}=true&limit=1")).await;
        assert_eq!(page.results.len(), 1, "{filter}");
        assert_eq!(page.results[0].message_id, Some(expected), "{filter}");
        assert!(page.next.is_none(), "{filter}");
    }
    let without_q: SearchPage = server
        .get(&reader, "/api/v1/search?has_picture=true")
        .await
        .ok(StatusCode::OK);
    assert_eq!(without_q.results[0].message_id, Some(picture.id));
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let suffix = cursor
            .as_ref()
            .map(|value| format!("&cursor={value}"))
            .unwrap_or_default();
        let page = search(
            server,
            &reader,
            &format!("&author_id={}&limit=1{suffix}", owner.account_id),
        )
        .await;
        seen.extend(page.results.iter().filter_map(|hit| hit.message_id));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
        assert!(seen.len() < 10);
    }
    for id in [mentioned.id, linked.id, picture.id] {
        assert!(seen.contains(&id));
    }
    assert!(!seen.contains(&direct.id));
    server
        .delete(owner, &message(picture.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        search(server, &reader, "&has_picture=true")
            .await
            .results
            .is_empty()
    );
    let role: junto_protocol::roles::WorkspaceRole = server
        .post(
            owner,
            &format!("/api/v1/workspaces/{}/roles", channel.workspace_id),
            &json!({"name":"Readers"}),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                channel.workspace_id, reader.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut mentions = vec![mentioned.id];
    for inline in [
        json!({"kind":"everyone"}),
        json!({"kind":"role","role_id":role.id}),
    ] {
        let sent: Message = server
            .post(
                owner,
                &format!("/api/v1/channels/{}/messages", channel.id),
                &json!({"document":{"blocks":[{"kind":"paragraph","content":[inline]}]}}),
            )
            .await
            .ok(StatusCode::CREATED);
        mentions.push(sent.id);
    }
    let page = search(server, &reader, "&mentions_me=true").await;
    assert_eq!(page.results.len(), mentions.len());
    assert!(
        mentions
            .iter()
            .all(|id| page.results.iter().any(|hit| hit.message_id == Some(*id)))
    );
    let private_upload = files.upload(b"private file").await;
    let private = files.attach(&[private_upload.id]).await;
    sqlx::query("update messages set readers=$2 where id=$1")
        .bind(private.id)
        .bind(vec![owner.account_id.as_uuid()])
        .execute(&server.db)
        .await
        .unwrap();
    assert!(
        search(server, &reader, "&has_file=true")
            .await
            .results
            .is_empty()
    );
    assert_eq!(
        search(server, owner, "&has_file=true").await.results[0].message_id,
        Some(private.id)
    );
    let file_scope = format!("&files=true&channel_id={}", channel.id);
    assert!(
        search(server, &reader, &file_scope)
            .await
            .results
            .is_empty()
    );
    assert_eq!(
        search(server, owner, &file_scope).await.results[0].message_id,
        Some(private.id)
    );
    let page = search(
        server,
        &reader,
        &format!(
            "&from={}&until={}",
            (mentioned.created_at - time::Duration::seconds(1))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            (direct.created_at + time::Duration::seconds(1))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        ),
    )
    .await;
    assert!(
        page.results
            .iter()
            .any(|hit| hit.message_id == Some(linked.id))
    );
    server
        .make_private(owner, channel.workspace_id, channel.id, role.id)
        .await;
    server
        .delete(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                channel.workspace_id, reader.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        search(server, &reader, "&mentions_me=true")
            .await
            .results
            .is_empty()
    );
}
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
    let studio = Studio::hidden_on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let ann = &studio.scene.ann;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 0, 500, "quartz transcript", None)],
    ));
    assert!(server.transcribe().await);
    // While the call runs and hides who is inside, only a person inside finds the lines
    // (ADR 0604), also after the setting is turned off (ADR 0517).
    for turn in 0..2 {
        assert!(search(server, owner, "quartz").await.results.is_empty());
        assert_eq!(search(server, ann, "quartz").await.results.len(), 1);
        if turn == 0 {
            server
                .call_settings(
                    owner,
                    studio.scene.voice.id,
                    json!({"hides_occupants":false}),
                )
                .await;
        }
    }
    server.leave_call(ann).await;
    server.leave_call(&studio.scene.ben).await;
    server.check().await;
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
    assert_eq!(search(server, ann, "amethyst").await.results.len(), 1);
    let reader = server.join(owner, "Search reader").await;
    let public = channel(server, owner, "text").await;
    let visible = mention(server, owner, &public, &reader).await;
    let private = channel(server, owner, "text").await;
    let everyone = server.everyone_role(owner, private.workspace_id).await;
    server
        .put(
            owner,
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                private.id, everyone.id
            ),
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
    // The reader finds the message of the channel that it sees and the line of the
    // transcript, whose call has ended; nothing of the private channel and of the direct
    // conversation.
    let hits = search(server, &reader, "quartz%20OR%20amethyst")
        .await
        .results;
    let messages: Vec<_> = hits.iter().filter_map(|hit| hit.message_id).collect();
    assert_eq!(messages, [visible.id]);
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter()
            .any(|hit| hit.recording_id == Some(done.recording.id))
    );
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
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                channel.id, everyone.id
            ),
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
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "quartz").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select
             gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join
             generate_series(1,199) n where id=$1")
        .bind(root.id)
        .execute(&mut server.test_data().await)
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
    server.require_no_statement_eviction(evictions).await;
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
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
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
    server.require_no_statement_eviction(evictions).await;
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
        .bind(done.recording.id).execute(&mut server.test_data().await).await.unwrap();
    let root = send(server, owner, &studio.scene.voice, "quartz").await;
    sqlx::query(
        "insert into messages(id,place_id,position,author_id,created_at,document)
        select gen_random_uuid(),place_id,position+n,author_id,created_at,$2 from messages
        cross join generate_series(1,5000) n where id=$1",
    )
    .bind(root.id)
    .bind(sqlx::types::Json(text("ordinary")["document"].clone()))
    .execute(&mut server.test_data().await)
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
    .bind(false)
    .bind(false)
    .bind(false)
    .bind(false)
    .bind(false)
    .bind(None::<uuid::Uuid>)
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

#[tokio::test]
async fn content_mentions_and_direct_filters_combine_before_pagination() {
    let files = Files::open().await;
    let server = &files.server;
    let owner = &files.owner;
    let channel = &files.channel;
    let reader = server.join(owner, "Reader").await;
    send(server, owner, channel, "quartz ordinary").await;
    let mentioned = mention(server, owner, channel, &reader).await;
    let linked: Message = server
        .post(
            owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":{"blocks":[{"kind":"paragraph","content":[
                {"kind":"link","text":"quartz link","url":"https://example.test/plan"}
            ]}]}}),
        )
        .await
        .ok(StatusCode::CREATED);
    let uploaded = upload(
        server,
        owner,
        json!({"channel_id":channel.id}),
        b"quartz file",
    )
    .await;
    let file: Message = server
        .post(
            owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":text("quartz file")["document"],"attachments":[uploaded.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
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
    for (filter, expected) in [
        ("mentions_me", mentioned.id),
        ("has_link", linked.id),
        ("has_file", file.id),
        ("direct", direct.id),
    ] {
        let page = search(server, &reader, &format!("quartz&{filter}=true&limit=1")).await;
        assert_eq!(page.results.len(), 1, "{filter}");
        assert_eq!(page.results[0].message_id, Some(expected), "{filter}");
        assert!(
            page.next.is_none(),
            "filtering must precede pagination: {filter}"
        );
    }
    assert!(
        search(server, &reader, "quartz&has_picture=true")
            .await
            .results
            .is_empty()
    );
    assert!(
        search(server, &reader, "quartz&has_file=true&has_link=true")
            .await
            .results
            .is_empty()
    );
    assert!(
        search(server, owner, "quartz&mentions_me=true")
            .await
            .results
            .is_empty()
    );
    let picture_upload = upload(
        server,
        owner,
        json!({"channel_id":channel.id}),
        &junto_server::image::samples::png(4, 4),
    )
    .await;
    let picture: Message = server.post(owner,
        &format!("/api/v1/channels/{}/messages", channel.id),
        &json!({"document":text("quartz picture")["document"],"attachments":[picture_upload.id]}))
        .await.ok(StatusCode::CREATED);
    let page = search(server, &reader, "quartz&has_picture=true").await;
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].message_id, Some(picture.id));
    sqlx::query("update attachments set removed_at=now() where id=$1")
        .bind(picture_upload.id)
        .execute(&server.db)
        .await
        .unwrap();
    assert!(
        search(server, &reader, "quartz&has_picture=true")
            .await
            .results
            .is_empty()
    );
    server
        .delete(owner, &message(file.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        search(server, &reader, "quartz&has_file=true")
            .await
            .results
            .is_empty()
    );
}

#[tokio::test]
async fn scoped_file_lists_include_empty_documents_and_thread_files_without_other_places() {
    let files = Files::open().await;
    let (server, owner, channel) = (&files.server, &files.owner, &files.channel);
    let reader = server.join(owner, "Reader").await;
    let first = files.upload(b"file one").await;
    let root: Message = server
        .post(
            owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":{"blocks":[]},"attachments":[first.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let second = files.upload(b"file two").await;
    let child: Message = server
        .post(
            owner,
            &format!("{}/thread", message(root.id)),
            &json!({"document":{"blocks":[]},"attachments":[second.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let query = format!(
        "&files=true&has_file=true&channel_id={}&limit=1",
        channel.id
    );
    let page = search(server, &reader, &query).await;
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].message_id, Some(child.id));
    assert_eq!(page.results[0].attachments[0].id, second.id);
    let next = search(
        server,
        &reader,
        &format!("{query}&cursor={}", page.next.unwrap()),
    )
    .await;
    assert_eq!(next.results[0].message_id, Some(root.id));
    assert_eq!(next.results[0].attachments[0].id, first.id);
    assert!(next.next.is_none());
    let conversation: Conversation = server
        .post(
            owner,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let upload = upload(
        server,
        owner,
        json!({"conversation_id":conversation.id}),
        b"direct file",
    )
    .await;
    let direct: Message = server
        .post(
            owner,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &json!({"document":{"blocks":[]},"attachments":[upload.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let scoped = search(
        server,
        &reader,
        &format!("&files=true&has_file=true&channel_id={}", channel.id),
    )
    .await;
    assert_eq!(scoped.results.len(), 2);
    assert!(
        scoped
            .results
            .iter()
            .all(|hit| hit.place_id == channel.id.as_uuid())
    );
    let query = format!(
        "&files=true&has_file=true&conversation_id={}",
        conversation.id
    );
    let page = search(server, &reader, &query).await;
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].message_id, Some(direct.id));
    assert_eq!(page.results[0].attachments[0].id, upload.id);
    let outsider = server.join(owner, "Outsider").await;
    server
        .get(&outsider, &format!("/api/v1/search?q={query}"))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(owner, &message(direct.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(search(server, &reader, &query).await.results.is_empty());
}

#[tokio::test]
async fn scoped_file_history_uses_a_chronological_index_on_first_and_later_sparse_pages() {
    let files = Files::open().await;
    let (server, owner, channel) = (&files.server, &files.owner, &files.channel);
    let upload = files.upload(b"seed file").await;
    let seed = files.attach(&[upload.id]).await;
    let mut fixture = server.test_data().await;
    for first in (1_i32..=300_000).step_by(10_000) {
        sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document,permanent)
            select gen_random_uuid(),place_id,position+n,author_id,created_at+n*interval '1 second',document,((position+n)%2000=0)
            from messages cross join generate_series($2::integer,$3::integer) n where id=$1")
            .bind(seed.id).bind(first).bind(first + 9_999)
            .execute(&mut fixture).await.unwrap();
    }
    sqlx::query("insert into attachments(id,storage_id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,expires_at,touched_at,attached,position)
        select gen_random_uuid(),a.storage_id,m.place_id,m.author_id,m.id,a.name,a.bytes,a.bytes,a.sha256,true,a.expires_at,a.touched_at,true,p
        from messages m cross join generate_series(0,1) p join attachments a on a.id=$1
        where m.place_id=$2 and m.position%1000=0")
        .bind(upload.id).bind(channel.id).execute(&mut fixture).await.unwrap();
    sqlx::raw_sql("analyze messages; analyze attachments")
        .execute(&mut fixture)
        .await
        .unwrap();
    drop(fixture);
    let capabilities: junto_protocol::discovery::SearchCapabilities = server
        .get(owner, "/api/v1/search/capabilities")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        capabilities,
        junto_protocol::discovery::SearchCapabilities::ALL
    );
    let (_held, _role) = crate::messages_queries::measure_requests(server).await;
    let query = format!(
        "&files=true&has_file=true&channel_id={}&limit=50",
        channel.id
    );
    let started = std::time::Instant::now();
    let first = search(server, owner, &query).await;
    let first_time = started.elapsed();
    assert_eq!(first.results.len(), 50);
    assert!(first.results.iter().all(|hit| hit.attachments.len() == 2));
    let started = std::time::Instant::now();
    let second = search(
        server,
        owner,
        &format!("{query}&cursor={}", first.next.as_ref().unwrap()),
    )
    .await;
    let second_time = started.elapsed();
    assert_eq!(second.results.len(), 50);
    let all: Vec<_> = first.results.iter().chain(&second.results).collect();
    eprintln!(
        "files over 300000 messages with 1/1000 file messages: first {} ms, later {} ms",
        first_time.as_millis(),
        second_time.as_millis()
    );
    assert!(
        all.windows(2)
            .all(|pair| (pair[0].created_at, pair[0].message_id)
                > (pair[1].created_at, pair[1].message_id))
    );
    assert!(all.iter().all(|hit| hit.rank == 0.0));
    assert!(first_time.as_millis() < 8000 && second_time.as_millis() < 8000);
    let mut connection = server.db.acquire().await.unwrap();
    let actual: String = sqlx::query_scalar("select statement from pg_prepared_statements where statement like 'with file_page as%' limit 1").fetch_one(&mut *connection).await.unwrap();
    for cursor in [None, first.next.as_deref()] {
        let (before, id) = match cursor {
            None => (
                time::OffsetDateTime::from_unix_timestamp(253_402_300_799).unwrap(),
                uuid::Uuid::max(),
            ),
            Some(cursor) => {
                let (before, id) = cursor.strip_prefix("f:").unwrap().split_once(':').unwrap();
                (
                    time::OffsetDateTime::from_unix_timestamp_nanos(before.parse().unwrap())
                        .unwrap(),
                    id.parse::<uuid::Uuid>().unwrap(),
                )
            }
        };
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "explain(analyze,format json) {actual}"
        )))
        .bind(channel.id)
        .bind(owner.account_id)
        .bind(before)
        .bind(id)
        .bind(51_i64)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert!(
            plan.to_string().contains("attachments_file_history"),
            "{plan}"
        );
        fn visited(node: &Value) -> u64 {
            node["Actual Rows"].as_u64().unwrap_or(0) * node["Actual Loops"].as_u64().unwrap_or(0)
                + node["Plans"]
                    .as_array()
                    .map_or(0, |children| children.iter().map(visited).sum())
        }
        fn file_index_rows(node: &Value) -> u64 {
            let own = if node["Index Name"] == "attachments_file_history" {
                node["Actual Rows"].as_u64().unwrap_or(0)
                    * node["Actual Loops"].as_u64().unwrap_or(0)
            } else {
                0
            };
            own + node["Plans"]
                .as_array()
                .map_or(0, |children| children.iter().map(file_index_rows).sum())
        }
        let work = visited(&plan[0]["Plan"]);
        let candidates = file_index_rows(&plan[0]["Plan"]);
        eprintln!("file page plan visited {work} rows, {candidates} attachment index rows: {plan}");
        assert!(
            work < 4000,
            "file pages must not visit the candidate message history: {plan}"
        );
        assert!(
            candidates <= 256,
            "the index must stop after enough visible file messages: {plan}"
        );
    }
}

#[tokio::test]
async fn a_filter_only_link_search_conceals_spoiler_labels_and_urls() {
    let files = Files::open().await;
    let (server, owner, channel) = (&files.server, &files.owner, &files.channel);
    let sent: Message = server.post(owner, &format!("/api/v1/channels/{}/messages", channel.id), &json!({"document":{"blocks":[{"kind":"paragraph","content":[
        {"kind":"text","text":"Visible before "},
        {"kind":"spoiler","content":[{"kind":"link","text":"concealed label","url":"https://example.test/concealed-url"}]},
        {"kind":"text","text":" visible after"}
    ]}]}})).await.ok(StatusCode::CREATED);
    let found: SearchPage = server
        .get(owner, "/api/v1/search?has_link=true")
        .await
        .ok(StatusCode::OK);
    assert_eq!(found.results.len(), 1);
    assert_eq!(found.results[0].message_id, Some(sent.id));
    let preview = &found.results[0].text;
    assert!(preview.contains("Visible before") && preview.contains("visible after"));
    assert!(preview.contains("Spoiler"));
    assert!(!preview.contains("concealed label") && !preview.contains("concealed-url"));
}
