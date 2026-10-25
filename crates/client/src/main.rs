//! The binary `junto`: reads its arguments, opens the data directory and starts the window.
//! On Windows it is a program of the Windows subsystem, so Windows opens no console beside the
//! window (ADR 0755).

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use junto_client::app::{self, WindowStart};
use junto_client::assets::Assets;
use junto_client_core::paths::{DATA_DIR_VARIABLE, data_dir_of_this_user};
use junto_client_core::{Client, ClientOptions};

/// The level of the log on standard error: `error`, `warn`, `info`, `debug` or `trace`.
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

The variable JUNTO_LOG_LEVEL sets the level of the log on standard error (default: warn).";

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

fn main() -> ExitCode {
    let parsed = parse(
        std::env::args().skip(1),
        junto_client_core::releases::TEST_CONFIGURATION,
    );
    // A command that writes a text for the person and opens no window.
    if !matches!(parsed, Ok(Parsed::Run(_))) {
        junto_client_core::console::attach_to_parent();
    }
    let arguments = match parsed {
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
    let level = std::env::var(LOG_LEVEL_VARIABLE)
        .ok()
        .and_then(|level| level.parse::<tracing::Level>().ok())
        .unwrap_or(tracing::Level::WARN);
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(level)
        .init();
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
            // (ADR 0710). The person reads why and decides.
            junto_client_core::console::attach_to_parent();
            eprintln!("junto cannot start: {problem}");
            return ExitCode::FAILURE;
        }
    };
    let start = arguments.start;
    let check_startup = arguments.check_startup;
    let started = std::rc::Rc::new(std::cell::Cell::new(false));
    let reached = started.clone();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            if let Err(error) = app::init(cx) {
                eprintln!("junto cannot start: {error:#}");
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
