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
const USAGE: &str = "usage:
  junto-server                             serve the API
  junto-server assign-owner <account-id>   make an active account the server owner
  junto-server issue-owner-code            print a new one-time owner code

Every form reads JUNTO_DATABASE_URL. The two commands work on the database directly.";

enum Command {
    Serve,
    AssignOwner(AccountId),
    IssueOwnerCode,
}

fn command(arguments: &[String]) -> Result<Command, String> {
    match arguments {
        [] => Ok(Command::Serve),
        [name] if name == "serve" => Ok(Command::Serve),
        [name] if name == "issue-owner-code" => Ok(Command::IssueOwnerCode),
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
    Ok(Database::connect(options, connections, &config.limits.database).await?)
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
