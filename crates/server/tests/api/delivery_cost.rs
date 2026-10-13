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
                log::publish(
                    &mut tx,
                    server.state.now(),
                    &Event::MessageChanged {
                        place: root.place,
                        message_id: root.id,
                        position: root.position,
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
            position: start.position as i64,
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
            assert_eq!(deliveries.len(), 100);
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
        let bound = if mixed { 19 } else { 14 };
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

#[tokio::test]
async fn suppressed_events_do_not_wake_or_fill_delivery_batches() {
    use crate::plan_support::{analyze, explained, index_names, visited_rows};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let start = server.snapshot(&owner).await.cursor;
    let app = junto_server::state::AppState::new(
        server.database_with(|_| {}).await,
        server.state.config.clone(),
        server.state.clock.clone(),
        server.state.server_id,
        None,
        None,
    );
    let registration = app.streams.open(owner.account_id, owner.device_id).unwrap();
    registration.link().set_level(Some(Level::Full));
    sqlx::query("insert into events(id,recorded_at,class,kind,audience,payload) select gen_random_uuid(),$1,'call_invite','call_invitation_resolved','{\"to\":\"suppressed\"}'::jsonb,jsonb_build_object('type','call_invitation_resolved','invitation_id',gen_random_uuid(),'outcome','expired') from generate_series(1,5000)")
        .bind(server.clock.now()).execute(&server.db).await.unwrap();
    let head = server.snapshot(&owner).await.cursor;
    assert_eq!(
        junto_server::events::watcher::advance(&app, Some(start.position as i64))
            .await
            .unwrap(),
        head.position as i64
    );
    let woke = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        registration.link().news(),
    )
    .await
    .is_ok();
    let subscription = Subscription {
        level: Level::Full,
        position: start.position as i64,
        epoch: start.epoch,
    };
    let batch = next_batch(&app, &crate::stream_review::opened(&owner), subscription, 5)
        .await
        .unwrap();
    let Batch::Events {
        deliveries,
        more,
        position,
        ..
    } = batch
    else {
        panic!("unexpected {batch:?}")
    };
    assert_eq!(
        (woke, more, position, deliveries.len()),
        (false, false, head.position as i64, 0)
    );
    analyze(&server.db, &["events"]).await;
    let plan: serde_json::Value=sqlx::query_scalar(explained("select position from events where position>$1 and audience->>'to' is distinct from 'suppressed' order by position limit 5"))
        .bind(start.position as i64).fetch_one(&server.db).await.unwrap();
    assert!(
        index_names(&plan)
            .iter()
            .any(|name| name == "events_deliverable"),
        "{plan}"
    );
    assert!(visited_rows(&plan[0]["Plan"]) < 10, "{plan}");
}
