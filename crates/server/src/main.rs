use std::process::ExitCode;
use std::sync::Arc;

use junto_protocol::ids::AccountId;
use junto_server::clock::{Clock, SystemClock};
use junto_server::config::{Config, LogFormat};
use junto_server::database::Database;
use junto_server::{maintenance, owner};
use sqlx::postgres::PgConnectOptions;
use tokio::net::TcpListener;
use tokio::signal;

const DATABASE_CONNECTIONS: u32 = 16;
const HOST_COMMAND_CONNECTIONS: u32 = 1;
/// The exit code of `check-upgrade` while a recording runs.
const RECORDING_RUNS: u8 = 3;
const USAGE: &str = "usage:
  junto-server                             serve the API
  junto-server assign-owner <account-id>   make an active account the server owner
  junto-server issue-owner-code            print a new one-time owner code
  junto-server new-event-epoch             make every client load a snapshot, after a restore
  junto-server check-upgrade               tell whether a recording runs; an upgrade waits for it
  junto-server backup-begin                hold file deletions back and print a backup identifier
  junto-server backup-files <backup-id>    after the copy of the database: list the files to copy
  junto-server backup-end <backup-id>      state that the backup has copied its files
  junto-server restored                    after a restore: new event epoch, no export, no backup
  junto-server schema-version              print the newest migration of this binary
  junto-server unnamed-files [--remove]    list, or remove, the directories that no row names
  junto-server health                      exit code 0 while the server of this host is healthy

Every form except health and schema-version reads JUNTO_DATABASE_URL; the commands work on
the database directly. health reads JUNTO_LISTEN_ADDRESS and asks the health route.
schema-version reads no setting.";

enum Command {
    Serve,
    AssignOwner(AccountId),
    IssueOwnerCode,
    NewEventEpoch,
    CheckUpgrade,
    BackupBegin,
    BackupFiles(uuid::Uuid),
    BackupEnd(uuid::Uuid),
    Restored,
    SchemaVersion,
    UnnamedFiles { remove: bool },
    Health,
}

fn command(arguments: &[String]) -> Result<Command, String> {
    match arguments {
        [] => Ok(Command::Serve),
        [name] if name == "serve" => Ok(Command::Serve),
        [name] if name == "issue-owner-code" => Ok(Command::IssueOwnerCode),
        [name] if name == "new-event-epoch" => Ok(Command::NewEventEpoch),
        [name] if name == "check-upgrade" => Ok(Command::CheckUpgrade),
        [name] if name == "backup-begin" => Ok(Command::BackupBegin),
        [name] if name == "restored" => Ok(Command::Restored),
        [name] if name == "schema-version" => Ok(Command::SchemaVersion),
        [name] if name == "unnamed-files" => Ok(Command::UnnamedFiles { remove: false }),
        [name, flag] if name == "unnamed-files" && flag == "--remove" => {
            Ok(Command::UnnamedFiles { remove: true })
        }
        [name] if name == "health" => Ok(Command::Health),
        [name, backup_id] if name == "backup-end" || name == "backup-files" => backup_id
            .parse()
            .map(|id| {
                if name == "backup-end" {
                    Command::BackupEnd(id)
                } else {
                    Command::BackupFiles(id)
                }
            })
            .map_err(|_| format!("{backup_id:?} is not the identifier of a backup")),
        [name, account_id] if name == "assign-owner" => account_id
            .parse()
            .map(Command::AssignOwner)
            .map_err(|_| format!("{account_id:?} is not an account identifier")),
        _ => Err("unknown arguments".to_owned()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = match command(&arguments) {
        Ok(command) => command,
        Err(problem) => {
            eprintln!("{problem}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Health => return health().await,
        Command::SchemaVersion => {
            println!("schema {}", junto_server::bootstrap::schema_version());
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("configuration error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = match command {
        Command::Serve => {
            init_logging(config.log_format, config.log_level);
            serve(config).await.inspect_err(|error| {
                tracing::error!(%error, "the server stopped with an error");
            })
        }
        Command::AssignOwner(account_id) => assign_owner(config, account_id).await,
        Command::IssueOwnerCode => issue_owner_code(config).await,
        Command::NewEventEpoch => new_event_epoch(config).await,
        Command::CheckUpgrade => match check_upgrade(config).await {
            Ok(true) => return ExitCode::from(RECORDING_RUNS),
            Ok(false) => Ok(()),
            Err(error) => Err(error),
        },
        Command::BackupBegin => backup_begin(config).await,
        Command::BackupFiles(id) => backup_files(config, id).await,
        Command::BackupEnd(id) => backup_end(config, id).await,
        Command::Restored => restored(config).await,
        Command::UnnamedFiles { remove } => unnamed_files(config, remove).await,
        Command::Health | Command::SchemaVersion => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

type Failure = Box<dyn std::error::Error>;

async fn connect(config: &Config, connections: u32) -> Result<Database, Failure> {
    let options: PgConnectOptions = config.database_url.parse()?;
    let limits = &config.limits;
    Ok(Database::connect(
        options,
        connections,
        &limits.database,
        &limits.stream.pool(),
    )
    .await?)
}

/// The time that the health command waits for the answer of the server.
const HEALTH_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(4);

/// Asks the health route of the server that listens on this host (ADR 0012): the exit code
/// is 0 for the status 200 and 1 otherwise. It is the health check of a container, so it
/// needs no setting except the listen address and no other program.
async fn health() -> ExitCode {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listen = std::env::var(Config::LISTEN_ADDRESS)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1:8080".to_owned());
    let Ok(mut address) = listen.trim().parse::<std::net::SocketAddr>() else {
        eprintln!("{} is not an address: {listen:?}", Config::LISTEN_ADDRESS);
        return ExitCode::FAILURE;
    };
    // A server that listens on every address is asked on the loopback address.
    if address.ip().is_unspecified() {
        address.set_ip(match address {
            std::net::SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            std::net::SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        });
    }
    let asked = async {
        let mut stream = tokio::net::TcpStream::connect(address).await?;
        let request =
            format!("GET /api/v1/health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        let mut answer = Vec::new();
        stream.take(1024).read_to_end(&mut answer).await?;
        Ok::<_, std::io::Error>(answer)
    };
    match tokio::time::timeout(HEALTH_TIME_LIMIT, asked).await {
        Ok(Ok(answer)) => {
            let status = String::from_utf8_lossy(&answer);
            let line = status.lines().next().unwrap_or_default();
            let mut parts = line.split(' ');
            if parts
                .next()
                .is_some_and(|version| version.starts_with("HTTP/1."))
                && parts.next() == Some("200")
            {
                println!("healthy");
                ExitCode::SUCCESS
            } else {
                eprintln!("the server at {address} answered: {line}");
                ExitCode::FAILURE
            }
        }
        Ok(Err(error)) => {
            eprintln!("the server at {address} could not be asked: {error}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("the server at {address} did not answer in time");
            ExitCode::FAILURE
        }
    }
}

async fn serve(config: Config) -> Result<(), Failure> {
    let options: PgConnectOptions = config.database_url.parse()?;
    use junto_server::bootstrap::MigrationCopied;
    match junto_server::bootstrap::copy_before_migration(&options, config.migration_copy).await? {
        MigrationCopied::Nothing => {}
        MigrationCopied::Initial => tracing::info!(
            "continuing the first installation without a migration copy: no installed server state exists"
        ),
        MigrationCopied::Made(copy) => tracing::info!(
            database = %copy,
            "this version changes the database; a copy of the database was made before"
        ),
        MigrationCopied::Unfinished(copy) => tracing::warn!(
            database = copy.as_deref().unwrap_or("none"),
            "an upgrade before this start did not apply every migration: no copy of the \
             database was made; the database named here is the copy from before that upgrade"
        ),
    }
    let database = connect(&config, DATABASE_CONNECTIONS).await?;
    let listen_address = config.listen_address;
    let stop_with_process = config.stop_with_process;
    let prepared = junto_server::prepare(database, config, Arc::new(SystemClock)).await?;

    if let Some(code) = &prepared.owner_code {
        print_code_notice(&format!(
            "This server has no owner. The first client that presents this one-time code \
             becomes the server owner. A restart replaces the code.\n\n    {}\n\n",
            code.expose()
        ))?;
    }

    let state = prepared.state;
    if state.config.public_origins.is_local_only() {
        tracing::warn!(
            "JUNTO_PUBLIC_ORIGINS is not set: devices can log in through a loopback address only"
        );
    }
    let listener = TcpListener::bind(listen_address).await?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    let background = [
        tokio::spawn(maintenance::run(state.clone())),
        tokio::spawn(junto_server::purge::run(state.clone())),
        tokio::spawn(junto_server::exports::run(state.clone())),
    ];
    let shutdown = shutdown_signal(stop_with_process);
    let served = junto_server::serve(listener, state.clone(), shutdown).await;
    for task in background {
        task.abort();
    }
    state.database.close().await;
    tracing::info!("stopped");
    Ok(served?)
}

async fn assign_owner(config: Config, account_id: AccountId) -> Result<(), Failure> {
    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    owner::assign(&database, account_id, SystemClock.now()).await?;
    println!("The account {account_id} is now the server owner.");
    Ok(())
}

async fn new_event_epoch(config: Config) -> Result<(), Failure> {
    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let epoch = owner::renew_event_epoch(&database, SystemClock.now()).await?;
    println!(
        "The event stream has the new epoch {epoch}. Every client loads a snapshot when it \
         subscribes the next time."
    );
    Ok(())
}

/// Prints whether a recording is starting or runs. The server does not migrate its database
/// during a recording, and the recorder is not replaced during one (ADR 0005). Returns true
/// while one runs.
async fn check_upgrade(config: Config) -> Result<bool, Failure> {
    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let running = junto_server::bootstrap::recordings_that_run(&database).await?;
    if running == 0 {
        println!("No recording runs. An upgrade can start.");
    } else {
        println!(
            "{running} recording(s) are starting or run. End them before an upgrade of the \
             server or of the recorder."
        );
    }
    Ok(running > 0)
}

/// Starts a backup and prints its identifier and the time until which the server holds
/// deletions back.
async fn backup_begin(config: Config) -> Result<(), Failure> {
    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let begun = junto_server::backup::begin(&database, SystemClock.now()).await?;
    let until = begun
        .expires_at
        .format(&time::format_description::well_known::Rfc3339)?;
    println!("backup {} {until}", begun.id);
    Ok(())
}

/// Prints one line for each directory that a backup copies or leaves out. It runs after
/// the copy of the database.
async fn backup_files(config: Config, id: uuid::Uuid) -> Result<(), Failure> {
    use std::io::Write;

    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let roots = roots(&config);
    let mut output = std::io::stdout().lock();
    let mut failed = None;
    let now = SystemClock.now();
    let listed = junto_server::backup::list(&database, now, id, &roots, |line| {
        if failed.is_none()
            && let Err(error) = writeln!(output, "{line}")
        {
            failed = Some(error);
        }
    })
    .await?;
    if let Some(error) = failed {
        return Err(error.into());
    }
    output.flush()?;
    match listed {
        junto_server::backup::Listed::Done => Ok(()),
        junto_server::backup::Listed::NoBackup => {
            Err(format!("no backup has the identifier {id}; it has ended or has expired").into())
        }
        junto_server::backup::Listed::NoDirectory(setting) => Err(format!(
            "the command does not see the directory of {setting}; run it with the settings \
             and the directories of the server"
        )
        .into()),
    }
}

/// The directories of the files of the server, as its settings name them.
fn roots(config: &Config) -> junto_server::backup::Roots {
    junto_server::backup::Roots {
        attachments: config.attachments_directory.clone(),
        recordings: config
            .recorder
            .as_ref()
            .map(|recorder| recorder.directory.clone()),
        exports: config.exports_directory.clone(),
    }
}

/// Ends a backup that runs. A backup that has expired, and the identifier of no backup,
/// are failures: the server did not hold deletions back until this command.
async fn backup_end(config: Config, id: uuid::Uuid) -> Result<(), Failure> {
    use junto_server::backup::Ended;

    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    match junto_server::backup::end_running(&database, SystemClock.now(), id).await? {
        Ended::Done => {
            println!("The backup {id} has ended. The server deletes files again.");
            Ok(())
        }
        Ended::Expired(expired_at) => {
            let expired_at = expired_at.format(&time::format_description::well_known::Rfc3339)?;
            Err(format!(
                "the backup {id} expired at {expired_at}: the server may have deleted files \
                 before the backup copied them. Make the backup again"
            )
            .into())
        }
        Ended::Unknown => {
            Err(format!("no backup has the identifier {id}; it has ended or has expired").into())
        }
    }
}

async fn restored(config: Config) -> Result<(), Failure> {
    use junto_server::backup::Restored;

    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    match junto_server::backup::restored(&database, SystemClock.now()).await? {
        Restored::Done { exports_removed } => {
            println!(
                "The event stream has a new epoch, no backup holds deletions back, and \
                 {exports_removed} export(s) were removed. Start the server."
            );
            Ok(())
        }
        Restored::OtherSchema(other) => Err(format!(
            "{other}. Nothing was changed. Run the command with the version of the server \
             that wrote the backup"
        )
        .into()),
    }
}

/// Prints one line for each directory of an attachment, of a recording and of an export
/// that no row names, and with `remove` deletes the ones that are old enough.
async fn unnamed_files(config: Config, remove: bool) -> Result<(), Failure> {
    use std::io::Write;

    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let roots = roots(&config);
    let mut output = std::io::stdout().lock();
    let mut failed = None;
    let now = SystemClock.now();
    let done = junto_server::backup::unnamed(&database, now, &roots, remove, |line| {
        if failed.is_none()
            && let Err(error) = writeln!(output, "{line}")
        {
            failed = Some(error);
        }
    })
    .await;
    if let Some(error) = failed {
        return Err(error.into());
    }
    output.flush()?;
    Ok(done?)
}

async fn issue_owner_code(config: Config) -> Result<(), Failure> {
    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let lifetime = config.limits.owner_code_lifetime;
    let issued = owner::issue_code(&database, lifetime, SystemClock.now()).await?;
    let notice = match issued.expires_at {
        Some(_) => format!(
            "The client that presents this one-time code within {} minutes gets access to the \
             account of the server owner. Every session of that account ends.\n\n    {}\n\n",
            lifetime.whole_minutes(),
            issued.code.expose()
        ),
        None => format!(
            "This server has no owner. The first client that presents this one-time code \
             becomes the server owner.\n\n    {}\n\n",
            issued.code.expose()
        ),
    };
    Ok(print_code_notice(&notice)?)
}

/// Writes a notice with a one-time code to standard output in one write, so that a reader
/// of standard output gets it in one read. `println!` writes a text with several lines in
/// parts. A log that holds both streams can still have a record of standard error between
/// the lines of one write; a program reads the code from standard output alone (ADR 0008).
fn print_code_notice(notice: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut output = std::io::stdout().lock();
    output.write_all(notice.as_bytes())?;
    output.flush()
}

fn init_logging(format: LogFormat, level: tracing::Level) {
    let builder = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(level);
    match format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Text => builder.init(),
    }
}

async fn shutdown_signal(stop_with_process: Option<u32>) {
    let interrupt = async {
        if signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    // The process that started the server (ADR 0915).
    let starter_ended = async {
        match stop_with_process {
            Some(starter) => {
                junto_server::starter::ended(starter).await;
                tracing::info!(
                    process = starter,
                    "the process that started the server has ended"
                );
            }
            None => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
        () = starter_ended => {}
    }
    tracing::info!("shutdown signal received");
}
