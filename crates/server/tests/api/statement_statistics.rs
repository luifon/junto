//! Which test databases record statement statistics.
use crate::support::TestServer;

const OWN_ENTRIES: &str = "select count(*) from pg_stat_statements
     where dbid = (select oid from pg_database where datname = current_database())";

#[tokio::test]
async fn only_a_measured_test_database_records_statements() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    server.snapshot(&owner).await;
    sqlx::query("create extension pg_stat_statements")
        .execute(&server.db)
        .await
        .unwrap();
    let entries: i64 = sqlx::query_scalar(OWN_ENTRIES)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(entries, 0, "a test database records no statement");

    let measured = TestServer::start_measured().await;
    let evictions = measured.statement_evictions().await;
    let owner = measured.owner().await;
    measured.snapshot(&owner).await;
    let entries: i64 = sqlx::query_scalar(OWN_ENTRIES)
        .fetch_one(&measured.db)
        .await
        .unwrap();
    assert!(entries > 0, "a measured database records its statements");
    measured.require_no_statement_eviction(evictions).await;
}

/// Prints how many entries the statement statistics of the database server hold. Run it
/// after the suite, in the same call of `tools/with-test-postgres.sh`.
#[tokio::test]
#[ignore = "a measurement of a whole run, not a check"]
async fn entries_of_the_run() {
    let server = TestServer::start().await;
    sqlx::query("create extension pg_stat_statements")
        .execute(&server.db)
        .await
        .unwrap();
    let (entries, evictions): (i64, i64) = sqlx::query_as(
        "select (select count(*) from pg_stat_statements),
                (select dealloc::bigint from pg_stat_statements_info)",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    println!("statement statistics: {entries} entries, {evictions} evictions");
}
