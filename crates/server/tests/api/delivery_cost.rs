use crate::messages_support::*;
use crate::support::TestServer;
use junto_protocol::events::{Event, Level};
use junto_server::events::{
    delivery::{Batch, Subscription, next_batch},
    log,
};

#[test]
fn general_stream_authorization_window_matches_production() {
    assert_eq!(
        crate::support::test_config()
            .limits
            .stream
            .authorization_validity,
        time::Duration::seconds(1)
    );
}

#[tokio::test]
async fn delivery_batch_statement_bound() {
    let server = TestServer::start_measured_with(|config| {
        config.limits.stream.batch = 100;
        config.limits.stream.poll_interval = time::Duration::hours(1);
    })
    .await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    // The changes of one batch are spread over several channels, so that a statement per
    // channel would show in the counts.
    let mut roots = vec![root.clone()];
    for words in ["second", "third", "fourth"] {
        let other = crate::messages_support::channel(&server, &owner, "text").await;
        roots.push(send(&server, &owner, &other, words).await);
    }
    for mixed in [false, true] {
        let start = server.snapshot(&reader).await.cursor;
        let mut tx = server.db.begin().await.unwrap();
        if mixed {
            sqlx::query("insert into notifications(id,account_id,message_id,kind,created_at) values($1,$2,$3,'thread',$4)")
                .bind(uuid::Uuid::new_v4()).bind(reader.account_id).bind(root.id).bind(server.state.now())
                .execute(&mut *tx).await.unwrap();
        }
        for n in 0..100 {
            if mixed && n % 3 == 1 {
                junto_server::messages::notifications::publish(
                    &mut tx,
                    server.state.now(),
                    root.id,
                    &[(reader.account_id, true)],
                    true,
                )
                .await
                .unwrap();
            } else if mixed && n % 3 == 2 {
                log::tell_account(
                    &mut tx,
                    server.state.now(),
                    reader.account_id,
                    &Event::ThreadRead {
                        root_id: root.id,
                        position: root.position,
                    },
                )
                .await
                .unwrap();
            } else {
                let changed = &roots[n % roots.len()];
                log::publish(
                    &mut tx,
                    server.state.now(),
                    &Event::MessageChanged {
                        place: changed.place,
                        message_id: changed.id,
                        position: changed.position,
                        deleted: false,
                    },
                )
                .await
                .unwrap();
            }
        }
        tx.commit().await.unwrap();
        let subscription = Subscription {
            level: Level::Full,
            position: junto_server::events::log::change_of(start.position),
            epoch: start.epoch,
        };
        let who = crate::stream_review::opened(&reader);
        for _ in 0..3 {
            next_batch(&server.state, &who, subscription, 100)
                .await
                .unwrap();
        }
        let mut observer = server.db.acquire().await.unwrap();
        let role: i64 =
            sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
                .fetch_one(&mut *observer)
                .await
                .unwrap();
        let jit_before: i64 = sqlx::query_scalar("select coalesce(sum(jit_functions),0)::bigint from pg_stat_statements where dbid=(select oid from pg_database where datname=current_database()) and query like 'with channels_selected%'")
            .fetch_one(&mut *observer).await.unwrap();
        let before = crate::messages_queries::statements(&mut observer, role).await;
        let mut times = Vec::new();
        for _ in 0..20 {
            let started = std::time::Instant::now();
            let batch = next_batch(&server.state, &who, subscription, 100)
                .await
                .unwrap();
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            let Batch::Events {
                deliveries,
                skipped,
                ..
            } = batch
            else {
                panic!("expected events")
            };
            assert!(!skipped);
            assert_eq!(
                deliveries
                    .iter()
                    .map(|delivery| delivery.events)
                    .sum::<usize>(),
                100
            );
        }
        let counts = crate::messages_queries::since(
            crate::messages_queries::statements(&mut observer, role).await,
            before,
        );
        let jit_after: i64 = sqlx::query_scalar("select coalesce(sum(jit_functions),0)::bigint from pg_stat_statements where dbid=(select oid from pg_database where datname=current_database()) and query like 'with channels_selected%'")
            .fetch_one(&mut *observer).await.unwrap();
        assert_eq!(
            jit_after, jit_before,
            "bounded standing reads must not compile a large JIT plan per batch"
        );
        let statements: i64 = counts
            .iter()
            .filter(|(sql, _)| !sql.contains("pg_stat_statements"))
            .map(|(_, calls)| calls)
            .sum();
        // The rules of the reader as a person (ADR 0304) are read once per batch: the rule
        // of the server, the rule of the workspace of the batch and the rules of all its
        // channels in one statement, with the events spread over four channels. The bounds
        // are the measured counts, 16 statements per batch of changes and 21 per mixed
        // batch, three of them these reads.
        for person_rules in [
            "select allow,deny from server_person_overrides",
            "select allow,deny from workspace_person_overrides",
            "select channel_id,allow,deny from effective_channel_person_overrides",
        ] {
            let calls: i64 = counts
                .iter()
                .filter(|(sql, _)| sql.starts_with(person_rules))
                .map(|(_, calls)| calls)
                .sum();
            assert_eq!(calls, 20, "{person_rules}: {counts:#?}");
        }
        let bound = if mixed { 21 } else { 16 };
        assert!(
            statements <= bound * 20,
            "batch statement bound: {counts:#?}"
        );
        times.sort_by(f64::total_cmp);
        eprintln!(
            "DELIVERY mixed={mixed} median_ms={:.3} min_ms={:.3} max_ms={:.3} statements_per_batch={}",
            times[10],
            times[0],
            times[19],
            statements / 20
        );
    }
    server.require_no_statement_eviction(evictions).await;
}
