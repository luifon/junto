mod cli;
mod events;
mod pattern;
mod publish;
mod session;
mod subscribe;
mod tone;

use clap::Parser;

/// The exit code of a participant whose test process has ended.
const STOPPED_WITH_PROCESS: i32 = 3;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    stop_with_process()?;
    let cli = cli::Cli::parse();
    let passed = match cli.command {
        cli::Command::Publish(args) => publish::run(*args).await.map(|_| true)?,
        cli::Command::Subscribe(args) => subscribe::run(args).await?,
    };
    std::process::exit(if passed { 0 } else { 1 });
}

/// With `JUNTO_STOP_WITH_PROCESS`, the number of the process of the test that started the
/// participant, the participant ends when that process ends, also when it is killed and
/// runs no `Drop` of its own (ADR 0915). A thread reads the number of the parent every
/// 100 ms: when a process ends, the system gives its children another parent, which cannot
/// have the number of the ended one. On other systems than Unix the setting is not read.
fn stop_with_process() -> anyhow::Result<()> {
    let Some(value) = std::env::var_os("JUNTO_STOP_WITH_PROCESS") else {
        return Ok(());
    };
    let starter: u32 = value
        .to_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("JUNTO_STOP_WITH_PROCESS is not a process number"))?;
    #[cfg(unix)]
    {
        let ended = move || std::os::unix::process::parent_id() != starter;
        if ended() {
            std::process::exit(STOPPED_WITH_PROCESS);
        }
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if ended() {
                std::process::exit(STOPPED_WITH_PROCESS);
            }
        });
    }
    #[cfg(not(unix))]
    let _ = (starter, STOPPED_WITH_PROCESS);
    Ok(())
}
