//! Read cost is bounded independently of the number of messages or channels.
use crate::messages_support::*;
use crate::support::TestServer;
use junto_protocol::message::MessagePage;
use reqwest::StatusCode;

type Counts = std::collections::BTreeMap<String, i64>;

pub(crate) async fn statements(connection: &mut sqlx::PgConnection, role: i64) -> Counts {
    sqlx::query_as::<_, (String, i64)>(
        "select query, sum(calls)::bigint from pg_stat_statements
         where dbid=(select oid from pg_database where datname=current_database())
           and userid=$1::oid group by query",
    )
    .bind(role)
    .fetch_all(connection)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

pub(crate) fn since(after: Counts, before: Counts) -> Counts {
    after
        .into_iter()
        .filter_map(|(query, count)| {
            let change = count - before.get(&query).copied().unwrap_or(0);
            assert!(change >= 0, "query statistics were lost: {query}");
            (change > 0).then_some((query, change))
        })
        .collect()
}

#[tokio::test]
async fn history_and_snapshot_statement_counts_do_not_grow_with_messages_or_channels() {
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "content").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join generate_series(1,199) n where id=$1").bind(root.id).execute(&server.db).await.unwrap();
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let route = format!("/api/v1/channels/{}/messages", channel.id);
    server
        .get(&owner, &format!("{route}?limit=200"))
        .await
        .ok::<MessagePage>(StatusCode::OK);
    let before = statements(observer, role).await;
    server
        .get(&owner, &format!("{route}?limit=1"))
        .await
        .ok::<MessagePage>(StatusCode::OK);
    let small = since(statements(observer, role).await, before);
    let before = statements(observer, role).await;
    let page: MessagePage = server
        .get(&owner, &format!("{route}?limit=200"))
        .await
        .ok(StatusCode::OK);
    let large = since(statements(observer, role).await, before);
    assert!(!small.is_empty(), "request statements must be counted");
    assert_eq!(page.messages.len(), 200);
    assert_eq!(large, small, "page statements must be constant");
    server.snapshot(&owner).await;
    let before = statements(observer, role).await;
    server.snapshot(&owner).await;
    let small = since(statements(observer, role).await, before);
    for _ in 0..5 {
        crate::messages_support::channel(&server, &owner, "text").await;
    }
    server.snapshot(&owner).await;
    let before = statements(observer, role).await;
    server.snapshot(&owner).await;
    let large = since(statements(observer, role).await, before);
    assert_eq!(
        large, small,
        "snapshot statements must not grow per channel"
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn unread_count_is_exact_below_one_hundred_and_capped_above_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "content").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join generate_series(1,299) n where id=$1")
        .bind(root.id).execute(&server.db).await.unwrap();
    let route = format!("/api/v1/channels/{}/messages/read", channel.id);
    let state: junto_protocol::message::MessageState =
        server.get(&reader, &route).await.ok(StatusCode::OK);
    assert_eq!(state.unread, 100);
    let state: junto_protocol::message::MessageState = server
        .put(
            &reader,
            &route,
            &serde_json::json!({"position":root.position+200}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.unread, 99);
}

pub(crate) async fn measure_requests(
    server: &TestServer,
) -> (Vec<sqlx::pool::PoolConnection<sqlx::Postgres>>, i64) {
    // Keep one warmed connection for HTTP requests; driver type lookups are not route work.
    let mut held = Vec::new();
    for _ in 1..server.db.options().get_max_connections() {
        held.push(server.db.acquire().await.unwrap());
    }
    let observer = &mut **held.first_mut().unwrap();
    // The event watcher has its own pool. Attribute only request SQL to this test role.
    let role_name = format!("measured_requests_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "create role {role_name} superuser"
    )))
    .execute(&mut *observer)
    .await
    .unwrap();
    let role: i64 = sqlx::query_scalar("select oid::bigint from pg_roles where rolname=$1")
        .bind(&role_name)
        .fetch_one(&mut *observer)
        .await
        .unwrap();
    let mut request_connection = server.db.acquire().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("set role {role_name}")))
        .execute(&mut *request_connection)
        .await
        .unwrap();
    drop(request_connection);
    (held, role)
}

#[tokio::test]
async fn snapshot_and_list_statements_do_not_grow_per_conversation() {
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let mut counts = Vec::new();
    for size in [1, 6] {
        for index in 0..size {
            let reader = server.join(&owner, &format!("Reader {size} {index}")).await;
            server
                .post(
                    &owner,
                    "/api/v1/conversations",
                    &serde_json::json!({"account_id":reader.account_id}),
                )
                .await
                .ok::<junto_protocol::message::Conversation>(StatusCode::OK);
        }
        server.snapshot(&owner).await;
        server
            .get(&owner, "/api/v1/conversations")
            .await
            .ok::<Vec<junto_protocol::message::Conversation>>(StatusCode::OK);
        let before = statements(observer, role).await;
        server.snapshot(&owner).await;
        server
            .get(&owner, "/api/v1/conversations")
            .await
            .ok::<Vec<junto_protocol::message::Conversation>>(StatusCode::OK);
        counts.push(since(statements(observer, role).await, before));
    }
    assert!(!counts[0].is_empty());
    assert_eq!(
        counts[0], counts[1],
        "statements must not grow per conversation"
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn mention_event_statements_do_not_grow_per_recipient() {
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let route = format!("/api/v1/channels/{}/messages", channel.id);
    let body = serde_json::json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}});
    let mut counts = Vec::new();
    for size in [1, 6] {
        for index in 0..size {
            server.join(&owner, &format!("Reader {size} {index}")).await;
        }
        server
            .post(&owner, &route, &body)
            .await
            .ok::<junto_protocol::message::Message>(StatusCode::CREATED);
        let before = statements(observer, role).await;
        server
            .post(&owner, &route, &body)
            .await
            .ok::<junto_protocol::message::Message>(StatusCode::CREATED);
        counts.push(since(statements(observer, role).await, before));
    }
    assert!(!counts[0].is_empty());
    assert_eq!(
        counts[0], counts[1],
        "statements must not grow per recipient"
    );
    server.require_no_statement_eviction(evictions).await;
}

fn visited_rows(plan: &serde_json::Value) -> u64 {
    let loops = plan["Actual Loops"].as_u64().unwrap_or(1);
    let rows = plan["Actual Rows"].as_u64().unwrap_or(0)
        + plan["Rows Removed by Filter"].as_u64().unwrap_or(0);
    loops * rows
        + plan["Plans"]
            .as_array()
            .map_or(0, |children| children.iter().map(visited_rows).sum())
}

#[tokio::test]
async fn unread_query_reads_only_the_indexed_unread_tail_of_a_large_history() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "content").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join generate_series(1,29999) n where id=$1")
        .bind(root.id).execute(&server.db).await.unwrap();
    sqlx::query("insert into message_reads(place_id,account_id,position) values($1,$2,29992)")
        .bind(channel.id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("analyze messages")
        .execute(&server.db)
        .await
        .unwrap();
    let query = format!(
        "explain (analyze, format json) {}",
        junto_server::messages::state_query()
    );
    let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(vec![channel.id.as_uuid()])
        .bind(reader.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(
        visited_rows(&plan[0]["Plan"]) < 1000,
        "unread must not scan read history: {plan}"
    );
}

#[tokio::test]
async fn maintenance_does_not_scan_live_attachments() {
    let files = Files::open().await;
    let root = send(&files.server, &files.owner, &files.channel, "content").await;
    let pending = files.start(b"content").await;
    sqlx::query("insert into attachments(id,storage_id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,expires_at,touched_at,attached,staging_cleaned) select gen_random_uuid(),storage_id,place_id,owner_id,$2,name,bytes,bytes,sha256,true,expires_at,touched_at,true,true from attachments cross join generate_series(1,20000) n where id=$1")
        .bind(pending.id).bind(root.id).execute(&files.server.db).await.unwrap();
    sqlx::query("analyze attachments")
        .execute(&files.server.db)
        .await
        .unwrap();
    let query = format!(
        "explain (analyze, format json) {}",
        junto_server::messages::uploads::EXPIRE_QUERY
    );
    let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(files.server.clock.now())
        .bind(100_i64)
        .fetch_one(&files.server.db)
        .await
        .unwrap();
    assert!(
        visited_rows(&plan[0]["Plan"]) < 1000,
        "maintenance must not scan live attachments: {plan}"
    );
}

const COPIES: &str = "insert into messages(id,place_id,position,author_id,created_at,document)
     select gen_random_uuid(),place_id,position+n,author_id,created_at,document
     from messages cross join generate_series(1,$2::int) n where id=$1";

async fn rows_read_by_the_state_query(
    server: &TestServer,
    place: uuid::Uuid,
    account: junto_protocol::ids::AccountId,
) -> (u64, serde_json::Value) {
    for table in ["messages", "message_mentions", "message_mention_accounts"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("analyze {table}")))
            .execute(&server.db)
            .await
            .unwrap();
    }
    let query = format!(
        "explain (analyze, format json) {}",
        junto_server::messages::state_query()
    );
    let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(vec![place])
        .bind(account)
        .fetch_one(&server.db)
        .await
        .unwrap();
    (visited_rows(&plan[0]["Plan"]), plan)
}

/// The account wrote every message after its read position. The unread part must not read
/// one row for each of them.
#[tokio::test]
async fn unread_query_does_not_read_every_own_message_after_the_read_position() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "content").await;
    sqlx::query(COPIES)
        .bind(root.id)
        .bind(19_999)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update message_places set head=(select max(position) from messages where place_id=$1) where id=$1")
        .bind(channel.id).execute(&server.db).await.unwrap();
    let last = send(&server, &owner, &channel, "last own message").await;
    let state: junto_protocol::message::MessageState = server
        .get(
            &owner,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.last_read, last.position);
    assert_eq!(state.unread, 0);
    let (rows, plan) =
        rows_read_by_the_state_query(&server, channel.id.as_uuid(), owner.account_id).await;
    assert!(
        rows < 1000,
        "the unread count read {rows} rows for 20,000 own messages: {plan}"
    );
}

/// Every message after the read position mentions another account. The mention part must
/// not read one row for each mention.
#[tokio::test]
async fn unread_query_does_not_read_every_mention_of_other_accounts_after_the_read_position() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let other = server.join(&owner, "Other").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "content").await;
    sqlx::query(COPIES)
        .bind(root.id)
        .bind(19_999)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query(
        "insert into message_mentions(place_id,position,message_id,account_id)
         select place_id,position,id,$2 from messages where place_id=$1",
    )
    .bind(channel.id)
    .bind(other.account_id)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into message_mention_accounts(place_id,position,message_id,account_id)
        select place_id,position,id,$2 from messages where place_id=$1",
    )
    .bind(channel.id)
    .bind(other.account_id)
    .execute(&server.db)
    .await
    .unwrap();
    let state: junto_protocol::message::MessageState = server
        .get(
            &other,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.mentions, 100);
    let (rows, plan) =
        rows_read_by_the_state_query(&server, channel.id.as_uuid(), reader.account_id).await;
    assert!(
        rows < 1000,
        "the mention count read {rows} rows for 20,000 mentions of another account: {plan}"
    );
}

#[tokio::test]
async fn round2_read_mentions_do_not_scan_the_read_history() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = crate::discovery_support::mention(&server, &owner, &channel, &reader).await;
    sqlx::query("with copied as (insert into messages(id,place_id,position,author_id,created_at,document) select gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join generate_series(1,5000) n where id=$1 returning *) insert into message_mention_accounts(place_id,account_id,position,message_id) select place_id,$2,position,id from copied").bind(root.id).bind(reader.account_id).execute(&server.db).await.unwrap();
    sqlx::query("insert into message_reads(place_id,account_id,position) values($1,$2,99999)")
        .bind(channel.id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("with copied as (insert into messages(id,place_id,position,author_id,created_at,document,thread_root) select gen_random_uuid(),place_id,position+n+5000,author_id,created_at,document,id from messages cross join generate_series(1,1001) n where id=$1 returning *) insert into message_mention_accounts(place_id,account_id,position,message_id,thread_root,unread) select place_id,$2,position,id,thread_root,false from copied")
        .bind(root.id).bind(reader.account_id).execute(&server.db).await.unwrap();
    sqlx::query("analyze message_mention_accounts")
        .execute(&server.db)
        .await
        .unwrap();
    let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(analyze,format json) {}",
        junto_server::messages::state_query()
    )))
    .bind(vec![channel.id.as_uuid()])
    .bind(reader.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(
        visited_rows(&plan[0]["Plan"]) < 1000,
        "read mentions must not be scanned: {plan}"
    );
    let (_held, _) = measure_requests(&server).await;
    server.snapshot(&reader).await;
    let mut connection = server.db.acquire().await.unwrap();
    let actual: String = sqlx::query_scalar("select statement from pg_prepared_statements where statement like 'select p.id,policy.level%' limit 1").fetch_one(&mut *connection).await.unwrap();
    let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(analyze,format json) {actual}"
    )))
    .bind(vec![channel.id.as_uuid()])
    .bind(reader.account_id)
    .bind(server.state.now())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        visited_rows(&plan[0]["Plan"]) < 1000,
        "policy mention counts must not scan read history: {plan}"
    );
}
