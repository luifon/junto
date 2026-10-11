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

Every form reads JUNTO_DATABASE_URL. The four commands work on the database directly.";

enum Command {
    Serve,
    AssignOwner(AccountId),
    IssueOwnerCode,
    NewEventEpoch,
    CheckUpgrade,
}

fn command(arguments: &[String]) -> Result<Command, String> {
    match arguments {
        [] => Ok(Command::Serve),
        [name] if name == "serve" => Ok(Command::Serve),
        [name] if name == "issue-owner-code" => Ok(Command::IssueOwnerCode),
        [name] if name == "new-event-epoch" => Ok(Command::NewEventEpoch),
        [name] if name == "check-upgrade" => Ok(Command::CheckUpgrade),
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

async fn serve(config: Config) -> Result<(), Failure> {
    let database = connect(&config, DATABASE_CONNECTIONS).await?;
    let listen_address = config.listen_address;
    let prepared = junto_server::prepare(database, config, Arc::new(SystemClock)).await?;

    if let Some(code) = &prepared.owner_code {
        println!(
            "This server has no owner. The first client that presents this one-time code \
             becomes the server owner. A restart replaces the code.\n\n    {}\n",
            code.expose()
        );
    }

    let state = prepared.state;
    if state.config.public_origins.is_local_only() {
        tracing::warn!(
            "JUNTO_PUBLIC_ORIGINS is not set: devices can log in through a loopback address only"
        );
    }
    let listener = TcpListener::bind(listen_address).await?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    let maintenance = tokio::spawn(maintenance::run(state.clone()));
    let served = junto_server::serve(listener, state.clone(), shutdown_signal()).await;
    maintenance.abort();
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

async fn issue_owner_code(config: Config) -> Result<(), Failure> {
    let database = connect(&config, HOST_COMMAND_CONNECTIONS).await?;
    let lifetime = config.limits.owner_code_lifetime;
    let issued = owner::issue_code(&database, lifetime, SystemClock.now()).await?;
    match issued.expires_at {
        Some(_) => println!(
            "The client that presents this one-time code within {} minutes gets access to the \
             account of the server owner. Every session of that account ends.\n\n    {}\n",
            lifetime.whole_minutes(),
            issued.code.expose()
        ),
        None => println!(
            "This server has no owner. The first client that presents this one-time code \
             becomes the server owner.\n\n    {}\n",
            issued.code.expose()
        ),
    }
    Ok(())
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

async fn shutdown_signal() {
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

    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received");
}
