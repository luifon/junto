//! One request for every notice, the choice to follow a thread, a workspace default that is
//! not set, and the quiet time by the name of a time zone (ADR 0412).

use junto_protocol::events::Level;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::discovery_support::*;
use crate::plan_support::{analyze, explained, index_names, visited_rows};

const NIL: uuid::Uuid = uuid::Uuid::nil();

async fn mark_all(server: &TestServer, member: &Member, body: Value) -> NotificationsRead {
    server
        .put(member, "/api/v1/notifications/read", &body)
        .await
        .ok(StatusCode::OK)
}

/// A reader who is notified of every message of the channel.
async fn reader_of_everything(server: &TestServer, owner: &Member, name: &str) -> Member {
    let reader = server.join(owner, name).await;
    settings(server, &reader, NIL, json!({ "level": "all" })).await;
    reader
}

#[tokio::test]
async fn one_request_marks_every_notice_read() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    for words in ["one", "two", "three"] {
        send(&server, &owner, &channel, words).await;
        server.clock.advance(time::Duration::seconds(1));
    }
    assert_eq!(notices(&server, &reader).await.len(), 3);
    let cursor = server.snapshot(&reader).await.cursor;

    let done = mark_all(&server, &reader, json!({})).await;
    assert_eq!(done.marked, 3);
    assert_eq!(done.until, done.read_at);
    let listed = notices(&server, &reader).await;
    assert_eq!(listed.len(), 3);
    assert!(
        listed
            .iter()
            .all(|notice| notice.read_at == Some(done.read_at))
    );
    assert!(
        server
            .snapshot(&reader)
            .await
            .notifications
            .iter()
            .all(|notice| notice.read_at.is_some())
    );
    // The devices of the account get one event, at the background level too.
    let told = events(&server, &reader, cursor, Level::Background).await;
    assert_eq!(
        told,
        [Event::NotificationsRead {
            until: done.until,
            read_at: done.read_at
        }]
    );

    // A request that marks nothing records no event.
    let cursor = server.snapshot(&reader).await.cursor;
    assert_eq!(mark_all(&server, &reader, json!({})).await.marked, 0);
    assert!(
        events(&server, &reader, cursor, Level::Full)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_notice_after_the_stated_time_stays_unread() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "one").await;
    server.clock.advance(time::Duration::seconds(1));
    send(&server, &owner, &channel, "two").await;
    let seen = notices(&server, &reader).await;
    let newest = seen[0].created_at;
    server.clock.advance(time::Duration::seconds(1));
    let later = send(&server, &owner, &channel, "three").await;

    let until = newest.format(&Rfc3339).unwrap();
    let done = mark_all(&server, &reader, json!({ "until": until })).await;
    assert_eq!(done.marked, 2);
    assert_eq!(done.until, newest);
    for notice in notices(&server, &reader).await {
        assert_eq!(notice.read_at.is_none(), notice.message_id == later.id);
    }
    // A time in the future is the time of the request: a later notice is not covered.
    let future = (server.clock.now() + time::Duration::days(1))
        .format(&Rfc3339)
        .unwrap();
    let done = mark_all(&server, &reader, json!({ "until": future })).await;
    assert_eq!(done.marked, 1);
    assert_eq!(done.until, done.read_at);
    server
        .put(
            &reader,
            "/api/v1/notifications/read",
            &json!({ "until": "yesterday" }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .put(
            &reader,
            "/api/v1/notifications/read",
            &json!({ "before": until }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn the_request_marks_no_notice_of_another_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let other = reader_of_everything(&server, &owner, "Other").await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "one").await;
    send(&server, &owner, &channel, "two").await;
    let cursor = server.snapshot(&other).await.cursor;

    assert_eq!(mark_all(&server, &reader, json!({})).await.marked, 2);
    let theirs = notices(&server, &other).await;
    assert_eq!(theirs.len(), 2);
    assert!(theirs.iter().all(|notice| notice.read_at.is_none()));
    assert!(
        events(&server, &other, cursor, Level::Full)
            .await
            .is_empty()
    );
    // A request without a session marks nothing.
    server
        .send(
            reqwest::Method::PUT,
            "/api/v1/notifications/read",
            None,
            Some(&json!({})),
            None,
        )
        .await
        .error(ErrorCode::Unauthenticated);
}

/// A notice of a channel that the account can no longer see is in no list. The request
/// does not count it, records no event for it, and leaves it as it is.
#[tokio::test]
async fn the_request_marks_and_counts_only_the_notices_that_the_account_reads() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    send(&server, &owner, &channel, "one").await;
    assert_eq!(notices(&server, &reader).await.len(), 1);
    let everyone = server.everyone_role(&owner, workspace.id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({ "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(notices(&server, &reader).await.is_empty());
    let cursor = server.snapshot(&reader).await.cursor;

    assert_eq!(mark_all(&server, &reader, json!({})).await.marked, 0);
    assert!(
        events(&server, &reader, cursor, Level::Full)
            .await
            .is_empty()
    );
    let unread: i64 = sqlx::query_scalar(
        "select count(*) from notifications where account_id = $1 and read_at is null",
    )
    .bind(reader.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(unread, 1);
}

/// The new requests that change something count against the limit of message changes of
/// the account, 120 in a minute (ADR 0408).
#[tokio::test]
async fn the_new_changes_count_against_the_limit_of_message_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    for _ in 0..119 {
        mark_all(&server, &owner, json!({})).await;
    }
    let follow_path = format!("{}/thread/follow", message(root.id));
    server
        .put(&owner, "/api/v1/notifications/read", &json!({}))
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(&owner, &follow_path, &json!({ "following": true }))
        .await
        .error(ErrorCode::RateLimited);
    server
        .delete(&owner, &follow_path)
        .await
        .error(ErrorCode::RateLimited);
    server
        .delete(&owner, &default_path(workspace.id))
        .await
        .error(ErrorCode::RateLimited);
}

/// An account with many read notices and a server with many notices of other accounts: the
/// statement reads the unread notices of the account and no other row. The read notices are
/// twenty times the bound of the rows that the plan may visit, so a plan that reads them
/// fails; the data is prepared in bounded batches under the normal statement time limit.
#[tokio::test]
async fn marking_all_read_reads_the_index_of_unread_notices() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "content").await;
    let others: Vec<uuid::Uuid> = sqlx::query_scalar(
        "insert into accounts (id, name, status, created_at)
         select gen_random_uuid(), 'filler', 'active', $1 from generate_series(1, 4)
         returning id",
    )
    .bind(server.clock.now())
    .fetch_all(&mut server.test_data().await)
    .await
    .unwrap();
    let (first, last) = (1, 2_000);
    sqlx::query(
        "with copies as (
         insert into messages (id, place_id, position, author_id, created_at, document)
         select gen_random_uuid(), place_id, position + n, author_id, created_at, document
         from messages cross join generate_series($4::int, $5::int) n where id = $1
         returning id, created_at),
     people as (select unnest($3::uuid[]) as id),
     theirs as (
         insert into notifications (id, account_id, message_id, kind, created_at)
         select c.id, p.id, c.id, 'message', c.created_at from copies c cross join people p)
     insert into notifications (id, account_id, message_id, kind, created_at, read_at)
     select id, $2, id, 'message', created_at, created_at from copies",
    )
    .bind(root.id)
    .bind(reader.account_id)
    .bind(&others)
    .bind(first)
    .bind(last)
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    analyze(&server.db, &["notifications", "messages"]).await;

    let statement = junto_server::messages::notifications::mark_all_read_statement();
    let plan: Value = sqlx::query_scalar(explained(&statement))
        .bind(reader.account_id)
        .bind(server.clock.now())
        .bind(vec![channel.id.as_uuid()])
        .bind(server.clock.now())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(
        index_names(&plan)
            .iter()
            .any(|name| name == "notifications_unread"),
        "{plan}"
    );
    assert!(visited_rows(&plan[0]["Plan"]) < 100, "{plan}");
    let unread: i64 = sqlx::query_scalar(
        "select count(*) from notifications where account_id = $1 and read_at is null",
    )
    .bind(reader.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(unread, 0);
}

async fn follow(
    server: &TestServer,
    member: &Member,
    root: MessageId,
    following: bool,
) -> ThreadState {
    server
        .put(
            member,
            &format!("{}/thread/follow", message(root)),
            &json!({ "following": following }),
        )
        .await
        .ok(StatusCode::OK)
}

async fn unfollow_choice(server: &TestServer, member: &Member, root: MessageId) {
    server
        .delete(member, &format!("{}/thread/follow", message(root)))
        .await
        .empty(StatusCode::NO_CONTENT);
}

async fn thread_state(server: &TestServer, member: &Member, root: MessageId) -> ThreadState {
    server
        .get(member, &format!("{}/thread/read", message(root)))
        .await
        .ok(StatusCode::OK)
}

async fn named_reply(
    server: &TestServer,
    author: &Member,
    root: MessageId,
    named: &Member,
) -> Message {
    server
        .post(
            author,
            &format!("{}/thread", message(root)),
            &json!({"document":{"blocks":[{"kind":"paragraph","content":[
                {"kind":"text","text":"for"},{"kind":"account","account_id":named.account_id}]}]}}),
        )
        .await
        .ok(StatusCode::CREATED)
}

#[tokio::test]
async fn an_account_that_stops_following_a_thread_gets_no_reply_notice_and_no_unread_reply() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &reader, root.id, "of the reader").await;
    let before = thread_state(&server, &reader, root.id).await;
    assert!(before.following);
    assert_eq!(before.follow_setting, None);
    let noticed = reply(&server, &owner, root.id, "while following").await;
    assert_eq!(notices(&server, &reader).await[0].message_id, noticed.id);
    assert_eq!(thread_state(&server, &reader, root.id).await.unread, 1);

    let after = follow(&server, &reader, root.id, false).await;
    assert!(!after.following);
    assert_eq!(after.follow_setting, Some(false));
    assert_eq!(after.unread, 0);
    let quiet = reply(&server, &owner, root.id, "while not following").await;
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .all(|notice| notice.message_id != quiet.id)
    );
    assert_eq!(thread_state(&server, &reader, root.id).await.unread, 0);
    let snapshot = server.snapshot(&reader).await;
    assert!(
        snapshot
            .threads
            .iter()
            .all(|thread| thread.root_id != root.id)
    );
    let unread: UnreadThreads = server
        .get(&reader, "/api/v1/threads/unread")
        .await
        .ok(StatusCode::OK);
    assert!(unread.threads.is_empty());
    // The other writer of the thread is still notified.
    let heard = reply(&server, &reader, root.id, "the reader writes").await;
    assert_eq!(notices(&server, &owner).await[0].message_id, heard.id);
}

#[tokio::test]
async fn a_mention_still_notifies_in_a_thread_that_is_not_followed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &reader, root.id, "of the reader").await;
    follow(&server, &reader, root.id, false).await;

    let named = named_reply(&server, &owner, root.id, &reader).await;
    let listed = notices(&server, &reader).await;
    assert_eq!(listed[0].message_id, named.id);
    assert_eq!(listed[0].kind, NotificationKind::Mention);
    let state = thread_state(&server, &reader, root.id).await;
    assert_eq!((state.mentions, state.unread), (1, 0));
    // Being named does not undo the choice.
    assert!(!state.following);
    assert_eq!(state.follow_setting, Some(false));
}

#[tokio::test]
async fn an_account_follows_a_thread_that_it_did_not_write_in() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let watcher = server.join(&owner, "Watcher").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &writer, root.id, "before the choice").await;
    server
        .get(&watcher, &format!("{}/thread/read", message(root.id)))
        .await
        .error(ErrorCode::NotFound);

    let chosen = follow(&server, &watcher, root.id, true).await;
    assert!(chosen.following);
    assert_eq!(chosen.follow_setting, Some(true));
    // The account follows from now: the replies before the choice are read.
    assert_eq!(chosen.unread, 0);
    assert_eq!(chosen.last_read, chosen.latest);

    let heard = reply(&server, &writer, root.id, "after the choice").await;
    let listed = notices(&server, &watcher).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].message_id, heard.id);
    assert_eq!(listed[0].kind, NotificationKind::Thread);
    assert_eq!(thread_state(&server, &watcher, root.id).await.unread, 1);
    let snapshot = server.snapshot(&watcher).await;
    assert_eq!(snapshot.threads.len(), 1);
    assert!(snapshot.threads[0].following);
    assert_eq!(snapshot.threads[0].follow_setting, Some(true));
}

#[tokio::test]
async fn the_choice_stays_when_the_account_writes_again() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    // The choice of an account that has not written yet stays at its first reply.
    follow(&server, &reader, root.id, false).await;
    reply(&server, &reader, root.id, "first reply").await;
    let state = thread_state(&server, &reader, root.id).await;
    assert!(!state.following);
    assert_eq!(state.follow_setting, Some(false));
    reply(&server, &reader, root.id, "second reply").await;
    let unheard = reply(&server, &owner, root.id, "of the owner").await;
    assert!(!thread_state(&server, &reader, root.id).await.following);
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .all(|notice| notice.message_id != unheard.id)
    );
}

#[tokio::test]
async fn removing_the_choice_returns_to_the_rule() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let watcher = server.join(&owner, "Watcher").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &writer, root.id, "of the writer").await;

    // A writer who chose not to follow follows again, from the newest reply.
    follow(&server, &writer, root.id, false).await;
    reply(&server, &owner, root.id, "missed on purpose").await;
    unfollow_choice(&server, &writer, root.id).await;
    let state = thread_state(&server, &writer, root.id).await;
    assert!(state.following);
    assert_eq!(state.follow_setting, None);
    assert_eq!(state.unread, 0);
    let heard = reply(&server, &owner, root.id, "heard again").await;
    assert_eq!(notices(&server, &writer).await[0].message_id, heard.id);
    assert_eq!(thread_state(&server, &writer, root.id).await.unread, 1);

    // An account that only chose to follow does not follow without the choice.
    follow(&server, &watcher, root.id, true).await;
    unfollow_choice(&server, &watcher, root.id).await;
    let state = thread_state(&server, &watcher, root.id).await;
    assert!(!state.following);
    assert_eq!(state.follow_setting, None);
    let unheard = reply(&server, &owner, root.id, "not for the watcher").await;
    assert!(
        notices(&server, &watcher)
            .await
            .iter()
            .all(|notice| notice.message_id != unheard.id)
    );
    // Removing a choice that does not exist changes nothing.
    unfollow_choice(&server, &watcher, root.id).await;
}

#[tokio::test]
async fn the_follow_setting_needs_sight_of_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let hidden = server.channel(&owner, workspace.id, "hidden").await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", hidden.id, everyone.id),
            &json!({ "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let root = send(&server, &owner, &hidden, "root").await;
    let answer = reply(&server, &owner, root.id, "reply").await;

    let path = format!("{}/thread/follow", message(root.id));
    server
        .put(&member, &path, &json!({ "following": true }))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&member, &path)
        .await
        .error(ErrorCode::NotFound);
    let rows: i64 = sqlx::query_scalar("select count(*) from thread_reads where account_id = $1")
        .bind(member.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    // A thread of a workspace that the caller is no member of.
    let elsewhere: junto_protocol::structure::Workspace = server
        .post(
            &owner,
            "/api/v1/workspaces",
            &json!({ "name": "Elsewhere" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let foreign = server.channel(&owner, elsewhere.id, "foreign").await;
    let foreign_root = send(&server, &owner, &foreign, "root").await;
    let foreign_path = format!("{}/thread/follow", message(foreign_root.id));
    server
        .put(&member, &foreign_path, &json!({ "following": true }))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&member, &foreign_path)
        .await
        .error(ErrorCode::NotFound);
    // The choice is a setting of the account: an archived channel takes it.
    let open = server.channel(&owner, workspace.id, "open").await;
    let open_root = send(&server, &owner, &open, "root").await;
    reply(&server, &owner, open_root.id, "reply").await;
    server
        .put_empty(&owner, &format!("/api/v1/channels/{}/archive", open.id))
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(follow(&server, &member, open_root.id, true).await.following);
    unfollow_choice(&server, &member, open_root.id).await;
    // A reply is no root of a thread.
    server
        .put(
            &owner,
            &format!("{}/thread/follow", message(answer.id)),
            &json!({ "following": true }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .put(&owner, &path, &json!({ "following": "yes" }))
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn a_change_of_the_follow_setting_is_told_to_the_account_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &owner, root.id, "reply").await;
    let own = server.snapshot(&reader).await.cursor;
    let other = server.snapshot(&owner).await.cursor;

    follow(&server, &reader, root.id, true).await;
    // The same choice again changes nothing.
    follow(&server, &reader, root.id, true).await;
    unfollow_choice(&server, &reader, root.id).await;
    let told = events(&server, &reader, own, Level::Background).await;
    assert_eq!(
        told,
        [
            Event::ThreadFollowChanged {
                root_id: root.id,
                following: true,
                follow_setting: Some(true)
            },
            Event::ThreadFollowChanged {
                root_id: root.id,
                following: false,
                follow_setting: None
            }
        ]
    );
    assert!(
        events(&server, &owner, other, Level::Full)
            .await
            .iter()
            .all(|event| !matches!(event, Event::ThreadFollowChanged { .. }))
    );
}

/// The changes of the follow state that an account was told after a cursor.
async fn follow_changes(
    server: &TestServer,
    member: &Member,
    after: junto_protocol::events::Cursor,
) -> Vec<Event> {
    events(server, member, after, Level::Background)
        .await
        .into_iter()
        .filter(|event| matches!(event, Event::ThreadFollowChanged { .. }))
        .collect()
}

#[tokio::test]
async fn an_account_that_a_reply_makes_follow_a_thread_is_told() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let named = server.join(&owner, "Named").await;
    let refuser = server.join(&owner, "Refuser").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    follow(&server, &refuser, root.id, false).await;
    let cursors = [
        server.snapshot(&owner).await.cursor,
        server.snapshot(&writer).await.cursor,
        server.snapshot(&named).await.cursor,
        server.snapshot(&refuser).await.cursor,
    ];
    let rule = Event::ThreadFollowChanged {
        root_id: root.id,
        following: true,
        follow_setting: None,
    };

    // The first reply makes its author and the author of the root follow by the rule. A
    // second reply changes nothing.
    reply(&server, &writer, root.id, "first").await;
    reply(&server, &writer, root.id, "second").await;
    assert_eq!(
        follow_changes(&server, &writer, cursors[1]).await,
        std::slice::from_ref(&rule)
    );
    assert_eq!(
        follow_changes(&server, &owner, cursors[0]).await,
        std::slice::from_ref(&rule)
    );
    assert!(thread_state(&server, &writer, root.id).await.following);

    // An account that a reply names follows by the rule. An account that chose not to
    // follow is named and does not follow: it is told nothing.
    named_reply(&server, &writer, root.id, &named).await;
    named_reply(&server, &writer, root.id, &refuser).await;
    assert_eq!(
        follow_changes(&server, &named, cursors[2]).await,
        std::slice::from_ref(&rule)
    );
    assert_eq!(follow_changes(&server, &refuser, cursors[3]).await, []);
    assert!(!thread_state(&server, &refuser, root.id).await.following);

    // An edit that names another account makes it follow, and it is told.
    let later = server.join(&owner, "Later").await;
    let after = server.snapshot(&later).await.cursor;
    let edited = reply(&server, &writer, root.id, "for nobody").await;
    server
        .put(
            &writer,
            &message(edited.id),
            &json!({"document":{"blocks":[{"kind":"paragraph","content":[
                {"kind":"text","text":"for"},{"kind":"account","account_id":later.account_id}]}]}}),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    assert_eq!(follow_changes(&server, &later, after).await, [rule]);
    assert!(thread_state(&server, &later, root.id).await.following);
}

#[tokio::test]
async fn a_follow_by_the_rule_is_not_told_to_an_account_that_does_not_see_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let outsider = server.join(&owner, "Outsider").await;
    let former = server.join(&owner, "Former").await;
    let channel = channel(&server, &owner, "text").await;
    // The root author writes while the channel is open to every member.
    let root = send(&server, &former, &channel, "root").await;
    // Then the channel is for one role only, which the writer holds.
    let insiders = server
        .grant_workspace(&owner, &writer, channel.workspace_id, &[])
        .await;
    server
        .make_private(&owner, channel.workspace_id, channel.id, insiders)
        .await;
    let cursors = [
        server.snapshot(&writer).await.cursor,
        server.snapshot(&outsider).await.cursor,
        server.snapshot(&former).await.cursor,
    ];

    // The reply names a member without `view_channel`, and it is the first reply of a root
    // whose author lost the channel. Both follow by the rule; only the writer is told.
    named_reply(&server, &writer, root.id, &outsider).await;
    assert_eq!(
        follow_changes(&server, &writer, cursors[0]).await,
        [Event::ThreadFollowChanged {
            root_id: root.id,
            following: true,
            follow_setting: None,
        }]
    );
    assert_eq!(follow_changes(&server, &outsider, cursors[1]).await, []);
    assert_eq!(follow_changes(&server, &former, cursors[2]).await, []);
}

fn default_path(workspace_id: WorkspaceId) -> String {
    format!("/api/v1/workspaces/{workspace_id}/notification-default")
}

async fn default_of(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
) -> Option<NotificationLevel> {
    server
        .get(member, &default_path(workspace_id))
        .await
        .ok::<NotificationDefault>(StatusCode::OK)
        .level
}

#[tokio::test]
async fn a_default_that_is_not_set_is_null_in_the_route_and_in_the_snapshot() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let stated =
        |snapshot: junto_protocol::snapshot::Snapshot| snapshot.workspaces[0].notification_default;

    let raw: Value = server
        .get(&member, &default_path(workspace.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(raw, json!({ "level": null }));
    assert_eq!(stated(server.snapshot(&member).await), None);

    for level in [NotificationLevel::Mentions, NotificationLevel::All] {
        server
            .put(
                &owner,
                &default_path(workspace.id),
                &json!({ "level": level }),
            )
            .await
            .ok::<NotificationDefault>(StatusCode::OK);
        assert_eq!(
            default_of(&server, &member, workspace.id).await,
            Some(level)
        );
        assert_eq!(stated(server.snapshot(&member).await), Some(level));
    }
    // The route that sets a level does not remove it.
    for body in [json!({ "level": null }), json!({})] {
        server
            .put(&owner, &default_path(workspace.id), &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    assert_eq!(
        default_of(&server, &member, workspace.id).await,
        Some(NotificationLevel::All)
    );

    server
        .delete(&owner, &default_path(workspace.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(default_of(&server, &member, workspace.id).await, None);
    assert_eq!(stated(server.snapshot(&member).await), None);
    // Without a default, an ordinary message notifies nobody.
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "ordinary").await;
    assert!(notices(&server, &member).await.is_empty());
}

#[tokio::test]
async fn removing_the_default_needs_the_administrator_permission() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let elsewhere: junto_protocol::structure::Workspace = server
        .post(
            &owner,
            "/api/v1/workspaces",
            &json!({ "name": "Elsewhere" }),
        )
        .await
        .ok(StatusCode::CREATED);
    for id in [workspace.id, elsewhere.id] {
        server
            .put(&owner, &default_path(id), &json!({ "level": "all" }))
            .await
            .ok::<NotificationDefault>(StatusCode::OK);
    }

    server
        .delete(&member, &default_path(workspace.id))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&member, &default_path(elsewhere.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&member, &default_path(WorkspaceId::generate()))
        .await
        .error(ErrorCode::NotFound);
    // The identifier of a channel is no workspace, also for an administrator.
    let channel = channel(&server, &owner, "text").await;
    let of_channel = default_path(WorkspaceId::from_uuid(channel.id.as_uuid()));
    for caller in [&owner, &member] {
        server
            .get(caller, &of_channel)
            .await
            .error(ErrorCode::NotFound);
        server
            .put(caller, &of_channel, &json!({ "level": "all" }))
            .await
            .error(ErrorCode::NotFound);
        server
            .delete(caller, &of_channel)
            .await
            .error(ErrorCode::NotFound);
    }
    let nil = default_path(WorkspaceId::from_uuid(NIL));
    server.get(&owner, &nil).await.error(ErrorCode::NotFound);
    server.delete(&owner, &nil).await.error(ErrorCode::NotFound);
    for id in [workspace.id, elsewhere.id] {
        assert_eq!(
            default_of(&server, &owner, id).await,
            Some(NotificationLevel::All)
        );
    }
}

#[tokio::test]
async fn a_change_of_the_default_is_an_event_with_the_level_for_the_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let elsewhere: junto_protocol::structure::Workspace = server
        .post(
            &owner,
            "/api/v1/workspaces",
            &json!({ "name": "Elsewhere" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let inside = server.join(&owner, "Inside").await;
    let outside = server.join_into(&owner, "Outside", &[elsewhere.id]).await;
    let own = server.snapshot(&inside).await.cursor;
    let other = server.snapshot(&outside).await.cursor;

    server
        .put(
            &owner,
            &default_path(workspace.id),
            &json!({ "level": "all" }),
        )
        .await
        .ok::<NotificationDefault>(StatusCode::OK);
    // The same level again is no change.
    server
        .put(
            &owner,
            &default_path(workspace.id),
            &json!({ "level": "all" }),
        )
        .await
        .ok::<NotificationDefault>(StatusCode::OK);
    server
        .delete(&owner, &default_path(workspace.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &default_path(workspace.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    let told = events(&server, &inside, own, Level::Background).await;
    assert_eq!(
        told,
        [
            Event::NotificationDefaultChanged {
                workspace_id: workspace.id,
                level: Some(NotificationLevel::All)
            },
            Event::NotificationDefaultChanged {
                workspace_id: workspace.id,
                level: None
            }
        ]
    );
    assert!(
        events(&server, &outside, other, Level::Full)
            .await
            .is_empty()
    );
    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_entries where action like 'notification_default%' order by id",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(
        actions,
        [
            "notification_default_changed",
            "notification_default_removed"
        ]
    );
}

fn quiet(start_minute: u16, end_minute: u16, time_zone: &str) -> Value {
    json!({ "daily_quiet": {
        "start_minute": start_minute, "end_minute": end_minute, "time_zone": time_zone } })
}

#[tokio::test]
async fn a_quiet_time_names_a_time_zone_that_the_server_knows() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let path = format!("/api/v1/notification-settings/{NIL}");
    for zone in ["Europe/Lisbon", "America/Sao_Paulo", "Asia/Kolkata", "UTC"] {
        server
            .put(&owner, &path, &quiet(1320, 420, zone))
            .await
            .ok::<NotificationSettings>(StatusCode::OK);
    }
    let stored: NotificationSettings = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(stored.daily_quiet.unwrap().time_zone, "UTC");

    // A text that is no name of a zone, also one that PostgreSQL reads as a rule of its
    // own, and a name that is too long.
    let long = "A".repeat(65);
    for zone in [
        "Nowhere/City",
        "",
        "XYZ3",
        "<+05>-05",
        "+03:00",
        long.as_str(),
    ] {
        server
            .put(&owner, &path, &quiet(1320, 420, zone))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    for body in [
        quiet(600, 600, "UTC"),
        quiet(1440, 1, "UTC"),
        quiet(1, 1440, "UTC"),
        json!({ "daily_quiet": { "start_minute": 1, "end_minute": 2 } }),
        json!({ "daily_quiet": {
            "start_minute": 1, "end_minute": 2, "utc_offset_minutes": 0 } }),
        json!({ "daily_quiet": {
            "start_minute": 1, "end_minute": 2, "time_zone": "UTC", "utc_offset_minutes": 0 } }),
    ] {
        server
            .put(&owner, &path, &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let kept: NotificationSettings = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(kept.daily_quiet.unwrap().time_zone, "UTC");
    // A quiet time is a setting of the account, not of a workspace.
    let workspace = server.default_workspace(&owner).await;
    server
        .put(
            &owner,
            &format!("/api/v1/notification-settings/{}", workspace.id),
            &quiet(1320, 420, "UTC"),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

fn at(text: &str) -> OffsetDateTime {
    OffsetDateTime::parse(text, &Rfc3339).unwrap()
}

async fn is_quiet(server: &TestServer, member: &Member, at: OffsetDateTime) -> bool {
    sqlx::query_scalar("select notification_quiet($1, $2)")
        .bind(member.account_id)
        .bind(at)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// The quiet time holds while the clock of the zone is inside the window. New York changes
/// its clock on 2030-03-10 (02:00 becomes 03:00) and on 2030-11-03 (02:00 becomes 01:00).
#[tokio::test]
async fn the_quiet_time_follows_the_clock_of_its_zone_over_daylight_saving_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let zone = "America/New_York";

    // 22:00 to 07:00: the same hours of the clock in winter (UTC-5) and in summer (UTC-4).
    settings(&server, &owner, NIL, quiet(1320, 420, zone)).await;
    for (at, expected) in [
        (at("2030-01-15T02:59:00Z"), false),
        (at("2030-01-15T03:00:00Z"), true),
        (at("2030-01-15T11:59:00Z"), true),
        (at("2030-01-15T12:00:00Z"), false),
        (at("2030-07-15T01:59:00Z"), false),
        (at("2030-07-15T02:00:00Z"), true),
        (at("2030-07-15T10:59:00Z"), true),
        (at("2030-07-15T11:00:00Z"), false),
    ] {
        assert_eq!(is_quiet(&server, &owner, at).await, expected, "{at}");
    }

    // 02:15 to 02:45 does not exist on the day on which the clock skips that hour.
    settings(&server, &owner, NIL, quiet(135, 165, zone)).await;
    for (at, expected) in [
        (at("2030-03-09T07:20:00Z"), true),
        (at("2030-03-10T06:59:00Z"), false),
        (at("2030-03-10T07:00:00Z"), false),
        (at("2030-03-10T07:20:00Z"), false),
        (at("2030-03-11T06:20:00Z"), true),
    ] {
        assert_eq!(is_quiet(&server, &owner, at).await, expected, "{at}");
    }

    // 01:15 to 01:45 happens twice on the day on which the clock repeats that hour.
    settings(&server, &owner, NIL, quiet(75, 105, zone)).await;
    for (at, expected) in [
        (at("2030-11-03T05:10:00Z"), false),
        (at("2030-11-03T05:20:00Z"), true),
        (at("2030-11-03T05:50:00Z"), false),
        (at("2030-11-03T06:20:00Z"), true),
        (at("2030-11-03T06:50:00Z"), false),
        (at("2030-11-03T07:20:00Z"), false),
    ] {
        assert_eq!(is_quiet(&server, &owner, at).await, expected, "{at}");
    }

    // The quiet time that the route stores is the one that holds a ring back.
    let scene = crate::calls_support::Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let india = time::UtcOffset::from_hms(5, 30, 0).unwrap();
    let local = server.clock.now().to_offset(india);
    let minute = u16::from(local.hour()) * 60 + u16::from(local.minute());
    settings(
        server,
        ben,
        NIL,
        quiet((minute + 1439) % 1440, (minute + 2) % 1440, "Asia/Kolkata"),
    )
    .await;
    crate::calls_review2::ring(server, ann, ben)
        .await
        .ok::<junto_protocol::call::CallJoin>(StatusCode::OK);
    assert!(server.snapshot(ben).await.call_invitations.is_empty());
    assert_eq!(server.snapshot(ann).await.sent_call_invitations.len(), 1);
}

#[tokio::test]
async fn a_stored_zone_that_the_database_does_not_know_is_no_quiet_time() {
    let scene = crate::calls_support::Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let store = |settings: Value| async move {
        sqlx::query(
            "insert into notification_settings (account_id, scope_id, settings)
             values ($1, $2, $3)
             on conflict (account_id, scope_id) do update set settings = excluded.settings",
        )
        .bind(ben.account_id)
        .bind(NIL)
        .bind(sqlx::types::Json(settings))
        .execute(&server.db)
        .await
        .unwrap();
    };
    let whole_day = json!({ "start_minute": 0, "end_minute": 1439, "time_zone": "Nowhere/City" });

    store(json!({ "do_not_disturb": false, "daily_quiet": whole_day })).await;
    assert!(!is_quiet(server, ben, server.clock.now()).await);
    // The account is rung, and its settings are still read. The delivery of the ring
    // asks for the quiet time inside its read-only transaction, and it does not fail.
    let cursor = server.snapshot(ben).await.cursor;
    crate::calls_review2::ring(server, ann, ben)
        .await
        .ok::<junto_protocol::call::CallJoin>(StatusCode::OK);
    assert_eq!(server.snapshot(ben).await.call_invitations.len(), 1);
    let told = events(server, ben, cursor, Level::Background).await;
    assert!(
        told.iter()
            .any(|event| matches!(event, Event::CallInvited { .. })),
        "{told:?}"
    );
    let read: NotificationSettings = server
        .get(ben, &format!("/api/v1/notification-settings/{NIL}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(read.daily_quiet.unwrap().time_zone, "Nowhere/City");

    // Do-not-disturb by hand does not depend on the zone.
    store(json!({ "do_not_disturb": true, "daily_quiet": whole_day })).await;
    assert!(is_quiet(server, ben, server.clock.now()).await);
    // A stored quiet time without its minutes is no quiet time, and the answer is no null.
    store(json!({ "daily_quiet": { "time_zone": "UTC" } })).await;
    assert!(!is_quiet(server, ben, server.clock.now()).await);
    store(json!({ "daily_quiet": null })).await;
    assert!(!is_quiet(server, ben, server.clock.now()).await);
}

/// The statement of the migration that converts a stored distance from UTC.
fn conversion() -> &'static str {
    let migration = include_str!("../../migrations/0015_client_requests.sql");
    let begin = migration.find("-- stored offsets: begin").unwrap();
    let end = migration.find("-- stored offsets: end").unwrap();
    &migration[begin..end]
}

#[tokio::test]
async fn stored_offsets_become_windows_of_utc_with_the_same_instants() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let cases: [(i32, i32, i32); 7] = [
        (1320, 420, -180),
        (60, 120, 330),
        (0, 1, 345),
        (1439, 0, 840),
        (600, 300, -720),
        (480, 1020, 0),
        (30, 90, 60),
    ];
    let mut members = Vec::new();
    for (index, (start, end, offset)) in cases.into_iter().enumerate() {
        let member = server.join(&owner, &format!("Member {index}")).await;
        sqlx::query(
            "insert into notification_settings (account_id, scope_id, settings) values ($1, $2, $3)",
        )
        .bind(member.account_id)
        .bind(NIL)
        .bind(sqlx::types::Json(json!({
            "level": "all", "do_not_disturb": false,
            "daily_quiet": {
                "start_minute": start, "end_minute": end, "utc_offset_minutes": offset } })))
        .execute(&server.db)
        .await
        .unwrap();
        members.push(member);
    }
    // A row without a daily quiet time and a row that names a zone stay as they are.
    settings(&server, &owner, NIL, quiet(1, 2, "Europe/Lisbon")).await;
    let plain = server.join(&owner, "Plain").await;
    settings(&server, &plain, NIL, json!({ "level": "nothing" })).await;

    sqlx::raw_sql(conversion())
        .execute(&server.db)
        .await
        .unwrap();

    let day = at("2030-06-01T00:00:00Z");
    for (member, (start, end, offset)) in members.iter().zip(cases) {
        let stored: NotificationSettings = server
            .get(member, &format!("/api/v1/notification-settings/{NIL}"))
            .await
            .ok(StatusCode::OK);
        assert_eq!(stored.level, Some(NotificationLevel::All));
        assert_eq!(stored.daily_quiet.as_ref().unwrap().time_zone, "UTC");
        let quiet_minutes: Vec<(i32, bool)> = sqlx::query_as(
            "select minute, notification_quiet($1, $2 + make_interval(mins => minute))
             from generate_series(0, 1439) minute order by minute",
        )
        .bind(member.account_id)
        .bind(day)
        .fetch_all(&server.db)
        .await
        .unwrap();
        assert_eq!(quiet_minutes.len(), 1440);
        for (minute_of_day, now_quiet) in quiet_minutes {
            let local = (minute_of_day + offset).rem_euclid(1440);
            let before = if start < end {
                local >= start && local < end
            } else {
                local >= start || local < end
            };
            assert_eq!(
                now_quiet, before,
                "{start} to {end} at {offset}, minute {minute_of_day}"
            );
        }
    }
    let kept: NotificationSettings = server
        .get(&owner, &format!("/api/v1/notification-settings/{NIL}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(kept.daily_quiet.unwrap().time_zone, "Europe/Lisbon");
    let plain_kept: NotificationSettings = server
        .get(&plain, &format!("/api/v1/notification-settings/{NIL}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(plain_kept.daily_quiet, None);
    assert_eq!(plain_kept.level, Some(NotificationLevel::Nothing));
}

#[tokio::test]
async fn mark_all_and_a_late_notice_agree() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "already committed").await;
    let before = server.snapshot(&reader).await.cursor;
    server.gate().arm(0);
    let route = format!("/api/v1/channels/{}/messages", channel.id);
    let body = text("late commit at the same time");
    let held = server.post(&owner, &route, &body);
    tokio::pin!(held);
    crate::support::drive_until(&mut held, server.gate().arrived()).await;
    let marked = mark_all(&server, &reader, json!({})).await;
    assert_eq!(marked.marked, 1);
    server.gate().release();
    let late: Message = held.await.ok(StatusCode::CREATED);
    let listed = notices(&server, &reader).await;
    let late_notice = listed.iter().find(|n| n.message_id == late.id).unwrap();
    assert!(late_notice.created_at <= marked.until);
    assert_eq!(late_notice.read_at, Some(marked.read_at));
    let received = events(&server, &reader, before, Level::Full).await;
    let event = received
        .iter()
        .find(|e| matches!(e, Event::NotificationsRead { .. }))
        .unwrap();
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type":"notifications_read", "until": serde_json::to_value(&marked).unwrap()["until"], "read_at": serde_json::to_value(&marked).unwrap()["read_at"]})
    );
    assert_eq!(notices(&server, &reader).await, listed);
}

#[tokio::test]
async fn an_empty_mark_applies_to_late_notices_but_not_newer_ones() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let marked = mark_all(&server, &reader, json!({})).await;
    assert_eq!(marked.marked, 0);
    let late = send(&server, &owner, &channel, "same timestamp").await;
    server.clock.advance(time::Duration::seconds(1));
    let newer = send(&server, &owner, &channel, "later timestamp").await;
    let listed = notices(&server, &reader).await;
    assert_eq!(
        listed
            .iter()
            .find(|notice| notice.message_id == late.id)
            .unwrap()
            .read_at,
        Some(marked.read_at)
    );
    assert_eq!(
        listed
            .iter()
            .find(|notice| notice.message_id == newer.id)
            .unwrap()
            .read_at,
        None
    );
}

#[tokio::test]
async fn notifications_read_keeps_the_version_one_fields() {
    #[derive(serde::Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum PreviousEvent {
        NotificationsRead {
            #[serde(with = "time::serde::rfc3339")]
            until: OffsetDateTime,
            #[serde(with = "time::serde::rfc3339")]
            read_at: OffsetDateTime,
        },
    }
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = reader_of_everything(&server, &owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "notice").await;
    let cursor = server.snapshot(&reader).await.cursor;
    let marked = mark_all(&server, &reader, json!({})).await;
    let frames = events(&server, &reader, cursor, Level::Full).await;
    let frame = frames
        .iter()
        .find(|event| matches!(event, Event::NotificationsRead { .. }))
        .unwrap();
    let PreviousEvent::NotificationsRead { until, read_at } =
        serde_json::from_value(serde_json::to_value(frame).unwrap()).unwrap();
    assert_eq!((until, read_at), (marked.until, marked.read_at));
}

#[tokio::test]
async fn the_read_boundary_is_an_indexed_account_lookup() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    mark_all(&server, &owner, json!({})).await;
    sqlx::query("with people as (insert into accounts(id,name,status,created_at) select gen_random_uuid(),'filler','active',$1 from generate_series(1,10000) returning id) insert into notification_read_boundaries(account_id,read_until,read_at) select id,$1,$1 from people")
        .bind(server.clock.now()).execute(&mut server.test_data().await).await.unwrap();
    analyze(&server.db, &["notification_read_boundaries"]).await;
    let plan: Value = sqlx::query_scalar(explained(
        "select read_at from notification_read_boundaries where account_id=$1 and read_until >= $2",
    ))
    .bind(owner.account_id)
    .bind(server.clock.now())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(
        index_names(&plan)
            .iter()
            .any(|name| name == "notification_read_boundaries_pkey"),
        "{plan}"
    );
    assert!(visited_rows(&plan[0]["Plan"]) < 10, "{plan}");
}
