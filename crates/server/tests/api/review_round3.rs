use crate::discovery_support::*;
use sqlx::Connection;

#[tokio::test]
async fn round3_mention_root_index() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let mut db = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan=off")
        .execute(&mut *db)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(
        "explain(format json) delete from message_mention_accounts where thread_root=$1",
    )
    .bind(root.id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(
        plan.to_string().contains("message_mention_thread_root"),
        "cascade predicate: {plan}"
    );
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(format json) {}",
        junto_server::messages::threads::STATES_QUERY
    )))
    .bind(reader.account_id)
    .bind(vec![root.id])
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(
        plan.to_string().contains("message_mention_thread_root"),
        "mention-only states: {plan}"
    );
}

#[tokio::test]
async fn round3_mention_head_update_is_indexed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    sqlx::query("with copies as (insert into messages(id,place_id,position,author_id,created_at,document,thread_root) select gen_random_uuid(),place_id,position+n,author_id,created_at,document,id from messages cross join generate_series(1,100) n where id=$1 returning *) insert into message_mention_accounts(place_id,account_id,position,message_id,thread_root) select place_id,$2,position,id,thread_root from copies")
        .bind(root.id).bind(reader.account_id).execute(&server.db).await.unwrap();
    sqlx::query("analyze message_mention_accounts")
        .execute(&server.db)
        .await
        .unwrap();
    let mut db = server.db.acquire().await.unwrap();
    let definition: String =
        sqlx::query_scalar("select pg_get_functiondef('refresh_mention_head()'::regprocedure)")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let update = definition
        .split("update message_mention_accounts set thread_head=true")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let update = update
        .replace("newest_position", "$4")
        .replace("subject", "$1")
        .replace("root", "$2")
        .replace("thread_$2", "thread_root")
        .replace("newest", "$3");
    let mut tx = db.begin().await.unwrap();
    sqlx::query("set local enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Bind all four values on both versions of the trigger statement.
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("explain(format json) update message_mention_accounts set thread_head=true {update} and $3::uuid is not null and $4::bigint is not null")))
        .bind(reader.account_id).bind(root.id).bind(root.id).bind(1_i64).fetch_one(&mut *tx).await.unwrap();
    let text = plan.to_string();
    assert!(
        text.contains("message_mention_threads")
            && plan[0]["Plan"]["Plans"][0]["Index Cond"]
                .as_str()
                .unwrap()
                .contains("\"position\" ="),
        "head update must seek one recipient position: {plan}"
    );
}

#[tokio::test]
async fn round3_unread_indexes_cover_place() {
    let server = TestServer::start().await;
    let definitions: Vec<String> = sqlx::query_scalar("select indexdef from pg_indexes where indexname in ('thread_reads_unread','message_mention_inbox') order by indexname")
        .fetch_all(&server.db).await.unwrap();
    assert_eq!(definitions.len(), 2);
    for definition in definitions {
        assert!(
            definition.contains("INCLUDE (place_id)"),
            "unread place filter must be covered: {definition}"
        );
    }
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &reader, &channel, "followed").await;
    reply(&server, &owner, root.id, "reply").await;
    let root = send(&server, &owner, &channel, "mentioned").await;
    server
        .post(
            &owner,
            &format!("{}/thread", message(root.id)),
            &json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}}),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    let mut db = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan=off")
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query("set enable_bitmapscan=off")
        .execute(&mut *db)
        .await
        .unwrap();
    fn covered(plan: &Value, index: &str) -> bool {
        (plan["Index Name"] == index
            && plan["Node Type"] == "Index Only Scan"
            && plan["Heap Fetches"] == 0)
            || plan["Plans"]
                .as_array()
                .is_some_and(|children| children.iter().any(|child| covered(child, index)))
    }
    let indexes = ["thread_reads_unread", "message_mention_inbox"];
    // VACUUM marks a page as visible to all only when no transaction of this database holds
    // a snapshot from before the last change of the page. The tasks of the server hold such
    // snapshots for moments, so the test repeats VACUUM until one ran without them.
    let mut plan = Value::Null;
    for _ in 0..200 {
        for table in ["thread_reads", "message_mention_accounts"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("vacuum analyze {table}")))
                .execute(&mut *db)
                .await
                .unwrap();
        }
        plan = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "explain(analyze,format json) {}",
            junto_server::messages::threads::UNREAD_QUERY
        )))
        .bind(reader.account_id)
        .bind(vec![uuid::Uuid::nil()])
        .bind(uuid::Uuid::max())
        .bind(uuid::Uuid::max())
        .bind(201_i64)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        if indexes.iter().all(|index| covered(&plan[0]["Plan"], index)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    for index in indexes {
        assert!(
            covered(&plan[0]["Plan"], index),
            "unreadable places must not fetch heap rows: {plan}"
        );
    }
}

#[tokio::test]
async fn round3_concurrent_mentions_keep_one_head() {
    use crate::support::drive_until;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let group =
        json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}});
    for sending in [false, true] {
        let root = send(&server, &owner, &channel, "root").await;
        let first = reply(&server, &owner, root.id, "first").await;
        let second = reply(&server, &owner, root.id, "second").await;
        let first_route = message(first.id);
        let second_route = if sending {
            format!("{}/thread", message(root.id))
        } else {
            message(second.id)
        };
        server.gate().arm(0);
        let one = server.put(&owner, &first_route, &group);
        tokio::pin!(one);
        drive_until(&mut one, server.gate().arrived()).await;
        let two = async {
            if sending {
                server.post(&owner, &second_route, &group).await
            } else {
                server.put(&owner, &second_route, &group).await
            }
        };
        tokio::pin!(two);
        drive_until(&mut two, server.until_blocked()).await;
        server.gate().release();
        one.await.ok::<Message>(StatusCode::OK);
        let newest: Message = two.await.ok(if sending {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        });
        let heads: Vec<MessageId> = sqlx::query_scalar("select message_id from message_mention_accounts where account_id=$1 and thread_root=$2 and thread_head")
            .bind(reader.account_id).bind(root.id).fetch_all(&server.db).await.unwrap();
        assert_eq!(heads, vec![newest.id]);
        let state = server
            .snapshot(&reader)
            .await
            .threads
            .into_iter()
            .find(|s| s.root_id == root.id)
            .unwrap();
        assert_eq!((state.unread, state.mentions), (0, 2));
    }
}

#[tokio::test]
async fn round3_hidden_fork_conflict_has_no_details() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    server
        .grant_workspace(
            &owner,
            &reader,
            channel.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageChannel],
        )
        .await;
    let root = send(&server, &owner, &channel, "root").await;
    let copy = fork(&server, &owner, root.id, "text").await;
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", copy.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(thread(&server, &reader, root.id).await.forks.is_empty());
    let answer = server
        .post(
            &reader,
            &format!("{}/fork", message(root.id)),
            &json!({"name":"another","kind":"text"}),
        )
        .await;
    let body: Value = serde_json::from_str(&answer.text()).unwrap();
    answer.error(ErrorCode::Conflict);
    assert_eq!(
        body,
        json!({"error":{"code":"conflict","message":"the thread has already been forked"}})
    );
}
