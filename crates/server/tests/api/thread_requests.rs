//! The first authors of the replies of a thread, and the threads of a channel that an
//! account follows (ADR 0413).

use crate::discovery_support::*;
use crate::plan_support::{analyze, explained, index_names, visited_rows};

async fn read(server: &TestServer, reader: &Member, id: MessageId) -> Message {
    server.get(reader, &message(id)).await.ok(StatusCode::OK)
}

async fn remove(server: &TestServer, author: &Member, id: MessageId) {
    server
        .delete(author, &message(id))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_root_states_the_first_three_authors_of_its_replies() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let ben = server.join(&owner, "Ben").await;
    let cat = server.join(&owner, "Cat").await;
    let dan = server.join(&owner, "Dan").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let plain = send(&server, &owner, &channel, "no thread").await;
    assert!(root.reply_authors.is_empty());

    let first = reply(&server, &ann, root.id, "one").await;
    assert_eq!(
        read(&server, &owner, root.id).await.reply_authors,
        [ann.account_id]
    );
    for author in [&ben, &ann, &cat, &dan, &owner] {
        reply(&server, author, root.id, "more").await;
    }
    let expected = [ann.account_id, ben.account_id, cat.account_id];
    assert_eq!(read(&server, &dan, root.id).await.reply_authors, expected);
    assert_eq!(
        thread(&server, &dan, root.id).await.root.reply_authors,
        expected
    );
    let history: MessagePage = server
        .get(&dan, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .ok(StatusCode::OK);
    let listed = |id: MessageId| {
        history
            .messages
            .iter()
            .find(|message| message.id == id)
            .map(|message| message.reply_authors.clone())
    };
    assert_eq!(listed(root.id), Some(expected.to_vec()));
    assert_eq!(listed(plain.id), Some(Vec::new()));
    // A reply has no thread of its own.
    assert!(read(&server, &dan, first.id).await.reply_authors.is_empty());
}

#[tokio::test]
async fn a_deleted_reply_removes_its_author_when_no_other_reply_of_them_is_left() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let ben = server.join(&owner, "Ben").await;
    let cat = server.join(&owner, "Cat").await;
    let dan = server.join(&owner, "Dan").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let of_ann = reply(&server, &ann, root.id, "one").await;
    let of_ben = reply(&server, &ben, root.id, "two").await;
    let again = reply(&server, &ann, root.id, "three").await;
    reply(&server, &cat, root.id, "four").await;
    // A reply that another reply answers stays as a marker when it is deleted.
    let answered: Message = server
        .post(
            &dan,
            &format!("{}/thread", message(root.id)),
            &json!({ "document": text("five")["document"], "reply_to": of_ben.id }),
        )
        .await
        .ok(StatusCode::CREATED);
    let authors = |message: Message| message.reply_authors;

    remove(&server, &ben, of_ben.id).await;
    assert_eq!(
        authors(read(&server, &owner, root.id).await),
        [ann.account_id, cat.account_id, dan.account_id]
    );
    // One of two replies of an author: the author stays, at the place of the first reply.
    remove(&server, &ann, of_ann.id).await;
    assert_eq!(
        authors(read(&server, &owner, root.id).await),
        [ann.account_id, cat.account_id, dan.account_id]
    );
    remove(&server, &ann, again.id).await;
    assert_eq!(
        authors(read(&server, &owner, root.id).await),
        [cat.account_id, dan.account_id]
    );
    remove(&server, &dan, answered.id).await;
    assert_eq!(
        authors(read(&server, &owner, root.id).await),
        [cat.account_id]
    );
    let rows: i64 = sqlx::query_scalar("select count(*) from thread_reply_authors")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

/// A thread in which 5,000 accounts wrote: the read of the root reads three rows.
#[tokio::test]
async fn reply_authors_are_read_by_an_index() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &owner, root.id, "reply").await;
    sqlx::query(
        "with people as (
             insert into accounts (id, name, status, created_at)
             select gen_random_uuid(), 'filler', 'active', $2 from generate_series(1, 5000)
             returning id)
         insert into thread_reply_authors (root_id, account_id, first_position, replies)
         select $1, id, 10 + row_number() over (), 1 from people",
    )
    .bind(root.id)
    .bind(server.clock.now())
    .execute(&server.db)
    .await
    .unwrap();
    analyze(&server.db, &["thread_reply_authors"]).await;

    assert_eq!(read(&server, &owner, root.id).await.reply_authors.len(), 3);
    let statement = format!(
        "select array({}) from messages m where m.id = $1",
        junto_server::messages::REPLY_AUTHORS
    );
    let plan: Value = sqlx::query_scalar(explained(&statement))
        .bind(root.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(
        index_names(&plan)
            .iter()
            .any(|name| name == "thread_reply_authors_order"),
        "{plan}"
    );
    assert!(visited_rows(&plan[0]["Plan"]) < 20, "{plan}");
}

async fn followed(
    server: &TestServer,
    member: &Member,
    channel: &Channel,
    query: &str,
) -> FollowedThreads {
    server
        .get(
            member,
            &format!("/api/v1/channels/{}/threads{query}", channel.id),
        )
        .await
        .ok(StatusCode::OK)
}

#[tokio::test]
async fn the_followed_threads_of_a_channel_include_read_ones_and_continue() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let other = server.channel(&owner, workspace.id, "other").await;
    let mut roots = Vec::new();
    for index in 0..5 {
        let root = send(&server, &owner, &channel, &format!("root {index}")).await;
        reply(&server, &reader, root.id, "of the reader").await;
        roots.push(root.id);
    }
    // Not in the list: a thread of another channel, a thread that the reader does not
    // follow, one that the reader chose not to follow, and one without a reply.
    let elsewhere = send(&server, &owner, &other, "elsewhere").await;
    reply(&server, &reader, elsewhere.id, "of the reader").await;
    let unfollowed = send(&server, &owner, &channel, "not followed").await;
    reply(&server, &owner, unfollowed.id, "of the owner").await;
    let chosen = send(&server, &owner, &channel, "chosen against").await;
    reply(&server, &reader, chosen.id, "of the reader").await;
    server
        .put(
            &reader,
            &format!("{}/thread/follow", message(chosen.id)),
            &json!({ "following": false }),
        )
        .await
        .ok::<ThreadState>(StatusCode::OK);
    let emptied = send(&server, &owner, &channel, "emptied").await;
    let only = reply(&server, &reader, emptied.id, "of the reader").await;
    server
        .delete(&reader, &message(only.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    // A newer reply moves the oldest thread to the front, and it is unread.
    reply(&server, &owner, roots[0], "newest reply").await;

    let all = followed(&server, &reader, &channel, "").await;
    let order: Vec<MessageId> = all.threads.iter().map(|thread| thread.root_id).collect();
    assert_eq!(order, [roots[0], roots[4], roots[3], roots[2], roots[1]]);
    assert!(!all.more);
    assert_eq!(all.next, None);
    assert_eq!(all.threads[0].unread, 1);
    assert!(all.threads[1..].iter().all(|thread| thread.unread == 0));
    assert!(all.threads.iter().all(|thread| thread.following));
    // The snapshot lists the unread one only.
    assert_eq!(server.snapshot(&reader).await.threads.len(), 1);

    let mut paged = Vec::new();
    let mut query = "?limit=2".to_owned();
    for size in [2, 2, 1] {
        let page = followed(&server, &reader, &channel, &query).await;
        assert_eq!(page.threads.len(), size);
        assert_eq!(page.more, size == 2);
        paged.extend(page.threads.iter().map(|thread| thread.root_id));
        match page.next {
            Some(next) => query = format!("?limit=2&before={next}"),
            None => assert_eq!(size, 1),
        }
    }
    assert_eq!(paged, order);
    // The list is the list of the caller: the owner follows every thread that it wrote in.
    assert_eq!(
        followed(&server, &owner, &channel, "").await.threads.len(),
        7
    );
    assert_eq!(
        followed(&server, &reader, &other, "").await.threads.len(),
        1
    );
}

#[tokio::test]
async fn the_list_of_followed_threads_needs_sight_of_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &member, root.id, "of the member").await;
    let path = format!("/api/v1/channels/{}/threads", channel.id);
    assert_eq!(
        followed(&server, &member, &channel, "").await.threads.len(),
        1
    );

    for query in ["?limit=0", "?limit=201", "?before=x", "?before=x:y"] {
        server
            .get(&member, &format!("{path}{query}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }

    // An archived channel is read as its messages are.
    let archive = format!("/api/v1/channels/{}/archive", channel.id);
    server
        .put_empty(&owner, &archive)
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        followed(&server, &member, &channel, "").await.threads.len(),
        1
    );
    server
        .delete(&owner, &archive)
        .await
        .ok::<Value>(StatusCode::OK);

    // The member loses sight of the channel and keeps the row of the thread.
    let everyone = server.everyone_role(&owner, workspace.id).await;
    server
        .put(
            &owner,
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                channel.id, everyone.id
            ),
            &json!({ "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let reply = server.get(&member, &path).await;
    reply.error(ErrorCode::NotFound);
    assert!(
        !reply.text().contains(&root.id.to_string()),
        "{}",
        reply.text()
    );
    server
        .get(
            &member,
            &format!("/api/v1/channels/{}/threads", ChannelId::generate()),
        )
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        followed(&server, &owner, &channel, "").await.threads.len(),
        1
    );
}

/// An account that follows 20,000 threads of a channel and as many of another channel:
/// one page reads its own rows only.
#[tokio::test]
async fn the_list_of_followed_threads_is_read_by_an_index() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let other = server.channel(&owner, workspace.id, "other").await;
    for place in [&channel, &other] {
        let root = send(&server, &owner, place, "root").await;
        for (first, last) in crate::support::fixture_batches(20_000) {
            sqlx::query(
                "with roots as (
                 insert into messages (id, place_id, position, author_id, created_at, document)
                 select gen_random_uuid(), place_id, position + n, author_id, created_at, document
                 from messages cross join generate_series($3::int, $4::int) n where id = $1
                 returning id)
             insert into thread_reads (root_id, account_id, following, rule_following)
             select id, $2, true, true from roots",
            )
            .bind(root.id)
            .bind(owner.account_id)
            .bind(first)
            .bind(last)
            .execute(&server.db)
            .await
            .unwrap();
        }
    }
    sqlx::query("update thread_reads set latest = 1, latest_id = root_id where account_id = $1")
        .bind(owner.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    analyze(&server.db, &["thread_reads", "messages"]).await;

    let plan: Value =
        sqlx::query_scalar(explained(junto_server::messages::threads::FOLLOWED_QUERY))
            .bind(owner.account_id)
            .bind(channel.id)
            .bind(uuid::Uuid::max())
            .bind(uuid::Uuid::max())
            .bind(51_i64)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(
        index_names(&plan)
            .iter()
            .any(|name| name == "thread_reads_followed"),
        "{plan}"
    );
    assert!(visited_rows(&plan[0]["Plan"]) < 200, "{plan}");
    let page = followed(&server, &owner, &channel, "").await;
    assert_eq!(page.threads.len(), 50);
    assert!(page.more);
}
