//! The binary `junto`: reads its arguments, opens the data directory and starts the window.

use std::path::PathBuf;
use std::process::ExitCode;

use junto_client::app::{self, WindowStart};
use junto_client::assets::Assets;
use junto_client::logs::LogFile;
use junto_client_core::paths::{DATA_DIR_VARIABLE, data_dir_of_this_user};
use junto_client_core::{Client, ClientOptions};

/// The level of the log on standard error and in the log file: `error`, `warn`, `info`,
/// `debug` or `trace`.
const LOG_LEVEL_VARIABLE: &str = "JUNTO_LOG_LEVEL";

const USAGE: &str = "\
junto: the Junto desktop application

usage: junto [options]

options:
  --data-dir <directory>   the directory of the server book, the profile and the device
                           keys of this installation. Two instances with two directories
                           run as two people on one machine. Default: the variable
                           JUNTO_DATA_DIR, then the directory of the user.
  --scale <percent>        the interface scale of this start, from 90 to 150
  --no-activate            open the window without bringing it to the front
  --check-startup          exit after rendering the first window frame
  --rollback-update        restore the verified previous archive, then exit
  --version                print the version
  --help                   print this text

The variable JUNTO_LOG_LEVEL sets the level of the log on standard error (default: warn)
and in the file logs/junto.log of the data directory (default: info). A panic is written
to a report file in the directory panics of the data directory.";

struct Arguments {
    data_dir: Option<PathBuf>,
    start: WindowStart,
    check_startup: bool,
}

enum Parsed {
    Run(Arguments),
    Print(String),
    Rollback,
    Update(String),
}

/// `test_configuration`: whether this build is a test configuration, the only one that
/// accepts `--update-from`.
fn parse(
    mut values: impl Iterator<Item = String>,
    test_configuration: bool,
) -> Result<Parsed, String> {
    let mut arguments = Arguments {
        data_dir: None,
        start: WindowStart::default(),
        check_startup: false,
    };
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--data-dir" => {
                let directory = values.next().ok_or("--data-dir needs a directory")?;
                arguments.data_dir = Some(PathBuf::from(directory));
            }
            "--scale" => {
                let percent = values
                    .next()
                    .and_then(|value| value.trim_end_matches('%').parse::<u32>().ok())
                    .ok_or("--scale needs a number of percent")?;
                arguments.start.scale_percent = Some(percent);
            }
            "--no-activate" => arguments.start.activate = false,
            "--check-startup" => {
                arguments.check_startup = true;
                arguments.start.activate = false;
            }
            "--rollback-update" => return Ok(Parsed::Rollback),
            // The package checks update an installed test build from a local source, through
            // the path of the update action of the window.
            "--update-from" if test_configuration => {
                let source = values.next().ok_or("--update-from needs an address")?;
                return Ok(Parsed::Update(source));
            }
            "--update-from" => {
                return Err("--update-from is accepted only by a test configuration".into());
            }
            "--version" => {
                return Ok(Parsed::Print(junto_client_core::releases::build_label()));
            }
            "--help" | "-h" => return Ok(Parsed::Print(USAGE.to_owned())),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Parsed::Run(arguments))
}

fn data_dir(chosen: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(directory) = chosen {
        return if directory.is_absolute() {
            Ok(directory)
        } else {
            std::env::current_dir()
                .map(|current| current.join(directory))
                .map_err(|error| format!("the current directory cannot be read: {error}"))
        };
    }
    if let Some(directory) = std::env::var_os(DATA_DIR_VARIABLE).filter(|value| !value.is_empty()) {
        let directory = PathBuf::from(directory);
        return if directory.is_absolute() {
            Ok(directory)
        } else {
            Err(format!("{DATA_DIR_VARIABLE} must be an absolute path"))
        };
    }
    data_dir_of_this_user().map_err(|error| error.to_string())
}

fn update_from(source: &str) -> ExitCode {
    use junto_client_core::releases::{self, install::Previous};
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(releases::Error::from)
        .and_then(|runtime| runtime.block_on(releases::update_running_from(Some(source))));
    match result {
        Ok(installed) => {
            println!("installed version {}", installed.version);
            match installed.previous {
                Previous::Kept(version) => println!("version {version} is kept for a rollback"),
                Previous::Missing(reason) => println!("no previous version is kept: {reason}"),
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// The log on standard error and, once [`LogFile::open`] is called, in the log file of the
/// data directory (ADR 0757). Without `JUNTO_LOG_LEVEL` the file takes `info` of the crates
/// of Junto and `warn` of the other crates, such as the call library and gpui.
fn start_log() -> LogFile {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    use tracing_subscriber::{Layer as _, fmt};
    let chosen = std::env::var(LOG_LEVEL_VARIABLE)
        .ok()
        .and_then(|level| level.parse::<tracing::Level>().ok());
    let log_file = LogFile::new();
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(LevelFilter::from_level(
                    chosen.unwrap_or(tracing::Level::WARN),
                )),
        )
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(log_file.clone())
                .with_filter(match chosen {
                    Some(level) => Targets::new().with_default(level),
                    None => Targets::new()
                        .with_default(LevelFilter::WARN)
                        .with_target("junto", LevelFilter::INFO),
                }),
        )
        .init();
    log_file
}

/// The file of the temporary directory of the account that states why the last start
/// failed before it had a data directory (ADR 0757).
const START_FAILURE_FILE: &str = "junto-start-failure.txt";

/// Writes why the start failed to [`START_FAILURE_FILE`], replacing an earlier one.
// The start writes it before any window (ADR 0730).
#[allow(clippy::disallowed_methods)]
fn write_start_failure(problem: &str) {
    let path = std::env::temp_dir().join(START_FAILURE_FILE);
    let time = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let text = format!(
        "{time} {}: junto cannot start: {problem}\n",
        junto_client_core::releases::build_label()
    );
    if let Err(error) = std::fs::write(&path, text) {
        eprintln!("{} cannot be written: {error}", path.display());
    }
}

fn main() -> ExitCode {
    junto_client::panics::install();
    let arguments = match parse(
        std::env::args().skip(1),
        junto_client_core::releases::TEST_CONFIGURATION,
    ) {
        Ok(Parsed::Run(arguments)) => arguments,
        Ok(Parsed::Rollback) => {
            let result = junto_client_core::releases::PlatformInstaller::running()
                .and_then(|installer| installer.rollback());
            return match result {
                Ok(version) => {
                    println!("restored version {version}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(Parsed::Update(source)) => return update_from(&source),
        Ok(Parsed::Print(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(problem) => {
            eprintln!("{problem}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let log_file = start_log();
    #[cfg(target_os = "linux")]
    junto_client::renderer::choose_before_the_first_window();
    let client = data_dir(arguments.data_dir).and_then(|directory| {
        let mut options = ClientOptions::new(directory);
        options.update_source = junto_client_core::releases::HttpSource::compiled()
            .map_err(|error| error.to_string())?
            .map(|source| {
                std::sync::Arc::new(source)
                    as std::sync::Arc<dyn junto_client_core::updates::UpdateSource>
            });
        Client::open(options).map_err(|error| error.to_string())
    });
    let client = match client {
        Ok(client) => client,
        Err(problem) => {
            // The files of the data directory are not changed by a start that fails
            // (ADR 0710). The person reads why and decides; a program without a console
            // (Windows) has the file of the temporary directory (ADR 0757).
            eprintln!("junto cannot start: {problem}");
            tracing::error!("junto cannot start: {problem}");
            write_start_failure(&problem);
            return ExitCode::FAILURE;
        }
    };
    // The client made the data directory. The log of the directory is written by one
    // instance only: the one that holds `logs/lock` (ADR 0757).
    junto_client::panics::record_into(client.data_dir().root());
    if let Err(error) = log_file.open(client.data_dir().root()) {
        tracing::warn!(%error, "the log file was not opened");
    }
    let start = arguments.start;
    let check_startup = arguments.check_startup;
    let started = std::rc::Rc::new(std::cell::Cell::new(false));
    let reached = started.clone();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            if let Err(error) = app::init(cx) {
                eprintln!("junto cannot start: {error:#}");
                tracing::error!("junto cannot start: {error:#}");
                cx.quit();
                return;
            }
            let state = app::state(client, start, cx);
            match app::open_main_window(state, start, cx) {
                Ok(window) if check_startup => {
                    let _ = window.update(cx, |_, window, _| {
                        // Frame callbacks run before drawing. The second callback proves
                        // that the preceding frame completed layout and presentation.
                        window.on_next_frame(move |window, _| {
                            window.on_next_frame(move |_, cx| {
                                reached.set(true);
                                println!("Junto first frame rendered");
                                cx.quit();
                            });
                        });
                    });
                }
                Ok(_) => reached.set(true),
                Err(error) => {
                    eprintln!("junto cannot open its window: {error:#}");
                    tracing::error!("junto cannot open its window: {error:#}");
                    cx.quit();
                }
            }
        });
    if started.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Result<Parsed, String> {
        parse(
            arguments.iter().map(|argument| (*argument).to_owned()),
            false,
        )
    }

    #[test]
    fn only_a_test_configuration_updates_from_a_named_source() {
        let arguments = || ["--update-from", "http://127.0.0.1:1/"].map(str::to_owned);
        assert!(matches!(
            parse(arguments().into_iter(), true),
            Ok(Parsed::Update(source)) if source == "http://127.0.0.1:1/"
        ));
        assert!(matches!(
            parse(arguments().into_iter(), false),
            Err(problem) if problem.contains("test configuration")
        ));
        assert!(parse(["--update-from".to_owned()].into_iter(), true).is_err());
    }

    #[test]
    fn the_arguments_name_the_data_directory_the_scale_and_the_activation() {
        let Ok(Parsed::Run(arguments)) = parsed(&[]) else {
            panic!("no argument starts the application");
        };
        assert_eq!(arguments.data_dir, None);
        assert_eq!(arguments.start, WindowStart::default());
        assert!(arguments.start.activate);

        let Ok(Parsed::Run(arguments)) = parsed(&[
            "--data-dir",
            "/tmp/first",
            "--scale",
            "125%",
            "--no-activate",
        ]) else {
            panic!("the arguments are valid");
        };
        assert_eq!(arguments.data_dir, Some(PathBuf::from("/tmp/first")));
        assert_eq!(arguments.start.scale_percent, Some(125));
        assert!(!arguments.start.activate);
    }

    #[test]
    fn an_argument_that_is_not_known_or_not_complete_is_refused() {
        assert!(parsed(&["--scale"]).is_err());
        assert!(parsed(&["--scale", "large"]).is_err());
        assert!(parsed(&["--data-dir"]).is_err());
        assert!(parsed(&["--code", "ABCD"]).is_err());
        assert!(matches!(parsed(&["--help"]), Ok(Parsed::Print(text)) if text == USAGE));
        assert!(matches!(parsed(&["--version"]), Ok(Parsed::Print(_))));
    }

    #[test]
    fn a_relative_data_directory_is_taken_from_the_current_directory() {
        let chosen = data_dir(Some(PathBuf::from("first"))).unwrap();
        assert!(chosen.is_absolute() && chosen.ends_with("first"));
        assert_eq!(
            data_dir(Some(PathBuf::from("/tmp/first"))).unwrap(),
            PathBuf::from("/tmp/first")
        );
    }
}
