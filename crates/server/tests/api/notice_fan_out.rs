//! The cost of the trigger that queues the recipients of new notices for the cap of 500
//! notices per account (ADR 0409), when one message gives a notice to every member of a
//! large workspace. A measurement: it prints a table and asserts no time.

use junto_protocol::discovery::NotificationDefault;
use reqwest::StatusCode;
use serde_json::json;

use crate::messages_support::{channel, send};
use crate::support::TestServer;

const MESSAGES: u32 = 5;

/// The trigger of the product runs once for each notice. The other form, measured for
/// comparison, runs once for each statement and reads the inserted notices from a
/// transition table.
const PER_STATEMENT: &str = "
    drop trigger queue_notification_trim on notifications;
    create function queue_notification_trim_of_statement() returns trigger language plpgsql as $$
    begin
        insert into notification_trim_accounts select distinct account_id from inserted
            on conflict do nothing;
        return null;
    end;
    $$;
    create trigger queue_notification_trim after insert on notifications
        referencing new table as inserted
        for each statement execute function queue_notification_trim_of_statement();";

struct Measured {
    request_ms: f64,
    insert_ms: f64,
    trigger_ms: f64,
}

async fn measure(members: i64, per_statement: bool) -> Measured {
    let first = TestServer::start_measured().await;
    let owner = first.owner().await;
    let room = channel(&first, &owner, "text").await;
    let mut setup = vec![
        "do $$ begin execute format('alter database %I set track_functions = ''pl''', \
         current_database()); end $$"
            .to_owned(),
    ];
    if per_statement {
        setup.push(PER_STATEMENT.to_owned());
    }
    for statement in setup {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&first.db)
            .await
            .unwrap();
    }
    sqlx::query(
        "with people as (
             insert into accounts (id, name, status, created_at)
             select gen_random_uuid(), 'member ' || n, 'active', $2
             from generate_series(1, $3) n returning id)
         insert into workspace_members (workspace_id, account_id, joined_at)
         select $1, id, $2 from people",
    )
    .bind(room.workspace_id)
    .bind(first.state.now())
    .bind(members)
    .execute(&first.db)
    .await
    .unwrap();
    first
        .put(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/notification-default",
                room.workspace_id
            ),
            &json!({ "level": "all" }),
        )
        .await
        .ok::<NotificationDefault>(StatusCode::OK);
    // A process whose sessions start after the settings above.
    let server = first.restart().await;
    sqlx::raw_sql("select pg_stat_statements_reset(); select pg_stat_reset();")
        .execute(&server.db)
        .await
        .unwrap();

    let started = std::time::Instant::now();
    for number in 0..MESSAGES {
        send(&server, &owner, &room, &format!("to everyone {number}")).await;
    }
    let request_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(MESSAGES);
    sqlx::query("select pg_stat_force_next_flush()")
        .execute(&server.db)
        .await
        .unwrap();
    let notices: i64 = sqlx::query_scalar("select count(*) from notifications")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(notices, members * i64::from(MESSAGES));
    let (calls, insert_total): (i64, f64) = sqlx::query_as(
        "select coalesce(sum(calls), 0)::bigint, coalesce(sum(total_exec_time), 0)::float8
         from pg_stat_statements
         where query like 'insert into notifications(id,account_id,message_id,kind%'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(calls, i64::from(MESSAGES));
    let trigger_total: f64 = sqlx::query_scalar(
        "select coalesce(sum(total_time), 0)::float8 from pg_stat_user_functions
         where funcname in ('queue_notification_trim', 'queue_notification_trim_of_statement')",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    Measured {
        request_ms,
        insert_ms: insert_total / f64::from(MESSAGES),
        trigger_ms: trigger_total / f64::from(MESSAGES),
    }
}

#[tokio::test]
#[ignore = "a measurement for ADR 0409; run it alone with --nocapture"]
async fn measure_the_queue_of_the_notice_cap_at_large_fan_out() {
    println!("members | form | request ms | notice insert ms | trigger ms | trigger share");
    for members in [100, 1_000, 5_000, 10_000] {
        for per_statement in [false, true] {
            let measured = measure(members, per_statement).await;
            println!(
                "{members} | {} | {:.1} | {:.1} | {:.1} | {:.0}%",
                if per_statement { "statement" } else { "row" },
                measured.request_ms,
                measured.insert_ms,
                measured.trigger_ms,
                100.0 * measured.trigger_ms / measured.insert_ms
            );
        }
    }
}
