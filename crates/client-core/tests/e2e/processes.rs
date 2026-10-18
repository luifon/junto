//! A server that a test starts stops when the process of that test ends, also when that
//! process is killed (ADR 0915).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;

use crate::support::{TestServer, admin_url};

/// Set in the process that the test below starts and kills.
const HOLDER: &str = "JUNTO_TEST_SERVER_HOLDER";
const SERVER_LINE: &str = "the server ";

/// The process that the test below kills: it starts a server, prints the number of its
/// process and the name of its database, and waits. Without the variable it does nothing.
#[tokio::test]
#[ignore = "a part of a_server_that_a_test_started_stops_when_the_process_of_the_test_is_killed"]
async fn holds_a_server_until_its_process_is_killed() {
    if std::env::var_os(HOLDER).is_none() {
        return;
    }
    let server = TestServer::start("held").await;
    let database = url::Url::parse(&server.database_url).unwrap();
    println!(
        "{SERVER_LINE}{} {}",
        server.process_id().unwrap(),
        database.path().trim_start_matches('/')
    );
    std::future::pending::<()>().await;
}

/// The sessions of the server on its own database end only when the server has stopped.
async fn sessions_on(database: &str) -> i64 {
    let admin = PgConnectOptions::from_str(&admin_url()).unwrap();
    let mut connection = admin.connect().await.unwrap();
    sqlx::query_scalar("select count(*) from pg_stat_activity where datname = $1")
        .bind(database)
        .fetch_one(&mut connection)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_server_that_a_test_started_stops_when_the_process_of_the_test_is_killed() {
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "processes::holds_a_server_until_its_process_is_killed",
            "--ignored",
            "--nocapture",
        ])
        .env(HOLDER, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let line = BufReader::new(holder.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line.strip_prefix(SERVER_LINE).map(str::to_owned))
        .expect("the holder names the process and the database of its server");
    let (process, database) = line.split_once(' ').unwrap();
    assert!(sessions_on(database).await > 0);

    // The process ends without running any code of its own: SIGKILL on Unix,
    // TerminateProcess on Windows.
    holder.kill().unwrap();
    holder.wait().unwrap();
    let killed = Instant::now();
    while sessions_on(database).await > 0 {
        if killed.elapsed() > Duration::from_secs(30) {
            end(process);
            panic!("the server still ran 30 s after the process of its test was killed");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Ends the server that the test above leaves running when it fails.
#[cfg(unix)]
fn end(process: &str) {
    if let Some(process) = rustix::process::Pid::from_raw(process.parse().unwrap()) {
        let _ = rustix::process::kill_process(process, rustix::process::Signal::KILL);
    }
}

/// Ends the server that the test above leaves running when it fails.
#[cfg(windows)]
fn end(process: &str) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", process])
        .status();
}
