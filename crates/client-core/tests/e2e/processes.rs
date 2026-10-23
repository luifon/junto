//! A server that a test starts stops when the process of that test ends, also when that
//! process is killed (ADR 0915).

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;

use crate::recording_server::RecordingServer;
use crate::support::{TestServer, admin_url, delete_database};

/// Set in the process that the test below starts and kills.
const HOLDER: &str = "JUNTO_TEST_SERVER_HOLDER";
const SERVER_LINE: &str = "the server ";

/// The process that the test below kills: it starts a server and the processes of
/// [`bystanders`], prints the number of the server process, the name of its database and the
/// numbers of the bystanders, and waits. Without the variable it does nothing.
#[tokio::test]
#[ignore = "a part of a_server_that_a_test_started_stops_when_the_process_of_the_test_is_killed"]
async fn holds_a_server_until_its_process_is_killed() {
    if std::env::var_os(HOLDER).is_none() {
        return;
    }
    let server = TestServer::start("held").await;
    let database = url::Url::parse(server.database_url()).unwrap();
    let bystanders = bystanders();
    let numbers: Vec<String> = bystanders
        .iter()
        .map(|bystander| bystander.id().to_string())
        .collect();
    println!(
        "{SERVER_LINE}{} {} {}",
        server.process_id().unwrap(),
        database.path().trim_start_matches('/'),
        numbers.join(",")
    );
    std::future::pending::<()>().await;
}

/// Processes that keep running after this one ends, each with a copy of one pipe that this
/// process holds. On macOS the standard library creates a pipe and makes it private to the
/// process in a second call, so a process that another thread starts between the two calls
/// inherits it; these processes hold every such pipe at once.
#[cfg(unix)]
fn bystanders() -> Vec<Child> {
    use std::os::unix::fs::FileTypeExt;
    let mut started = Vec::new();
    for entry in std::fs::read_dir("/dev/fd").unwrap().map_while(Result::ok) {
        let path = entry.path();
        let number = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok());
        if number.is_none_or(|number| number < 3)
            || !std::fs::metadata(&path).is_ok_and(|metadata| metadata.file_type().is_fifo())
        {
            continue;
        }
        let Ok(copy) = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .or_else(|_| std::fs::File::open(&path))
        else {
            continue;
        };
        let bystander = Command::new("sleep")
            .arg("600")
            .stdin(copy)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        started.push(bystander);
    }
    assert!(
        !started.is_empty(),
        "this process holds pipes of its server"
    );
    started
}

/// On Windows the standard library creates the end of a pipe that a child inherits while no
/// other child is started, so no other process holds it.
#[cfg(windows)]
fn bystanders() -> Vec<Child> {
    Vec::new()
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
    let mut parts = line.splitn(3, ' ');
    let (server, database) = (parts.next().unwrap(), parts.next().unwrap());
    let bystanders: Vec<&str> = parts
        .next()
        .unwrap_or_default()
        .split(',')
        .filter(|number| !number.is_empty())
        .collect();
    assert!(sessions_on(database).await > 0);

    // The process ends without running any code of its own: SIGKILL on Unix,
    // TerminateProcess on Windows.
    holder.kill().unwrap();
    holder.wait().unwrap();
    let killed = Instant::now();
    let mut stopped = false;
    while killed.elapsed() < Duration::from_secs(30) {
        if sessions_on(database).await == 0 {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for process in bystanders
        .iter()
        .chain(if stopped { None } else { Some(&server) })
    {
        end(process);
    }
    // The killed process ran no code that deletes its database.
    delete_database(database.to_owned(), Duration::from_secs(30)).unwrap();
    assert!(
        stopped,
        "the server still ran 30 s after the process of its test was killed"
    );
}

/// The setting of statement statistics that a session on the database of `url` gets, and
/// whether that database exists.
async fn recorded_and_kept(url: &str) -> (Option<String>, bool) {
    let name = url::Url::parse(url)
        .unwrap()
        .path()
        .trim_start_matches('/')
        .to_owned();
    let admin = PgConnectOptions::from_str(&admin_url()).unwrap();
    let mut connection = admin.connect().await.unwrap();
    let kept: bool =
        sqlx::query_scalar("select exists (select 1 from pg_database where datname = $1)")
            .bind(&name)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    if !kept {
        return (None, false);
    }
    let mut own = PgConnectOptions::from_str(url)
        .unwrap()
        .connect()
        .await
        .unwrap();
    let setting = sqlx::query_scalar("select current_setting('pg_stat_statements.track', true)")
        .fetch_one(&mut own)
        .await
        .unwrap();
    (setting, true)
}

/// The database of each kind of test server records no statement statistics, which the
/// tests of the server read on the same database server, and is deleted with its server.
#[tokio::test]
async fn the_database_of_a_test_server_records_no_statistics_and_ends_with_the_server() {
    let server = TestServer::start("dropped").await;
    let url = server.database_url().to_owned();
    assert_eq!(
        recorded_and_kept(&url).await,
        (Some("none".to_owned()), true)
    );
    drop(server);
    assert_eq!(recorded_and_kept(&url).await, (None, false));

    let server = RecordingServer::start("recorded").await;
    let url = server.database_url().to_owned();
    assert_eq!(
        recorded_and_kept(&url).await,
        (Some("none".to_owned()), true)
    );
    drop(server);
    assert_eq!(recorded_and_kept(&url).await, (None, false));
}

/// Ends a process that the test above leaves running.
#[cfg(unix)]
fn end(process: &str) {
    if let Some(process) = rustix::process::Pid::from_raw(process.parse().unwrap()) {
        let _ = rustix::process::kill_process(process, rustix::process::Signal::KILL);
    }
}

/// Ends a process that the test above leaves running.
#[cfg(windows)]
fn end(process: &str) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", process])
        .status();
}
