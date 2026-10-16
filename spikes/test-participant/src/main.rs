mod cli;
mod events;
mod pattern;
mod publish;
mod session;
mod subscribe;
mod tone;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = cli::Cli::parse();
    let passed = match cli.command {
        cli::Command::Publish(args) => publish::run(*args).await.map(|_| true)?,
        cli::Command::Subscribe(args) => subscribe::run(args).await?,
    };
    std::process::exit(if passed { 0 } else { 1 });
}
