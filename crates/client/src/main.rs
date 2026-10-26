//! The binary `junto`: reads its arguments, opens the data directory and starts the window.
//! On Windows it is a program of the Windows subsystem, so Windows opens no console beside the
//! window (ADR 0755).

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
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
to a report file in the directory panics of the data directory. On Windows standard error
of a running window is kept only when it is redirected, for example: junto.exe 2> err.txt";

struct Arguments {
    data_dir: Option<PathBuf>,
    start: WindowStart,
    check_startup: bool,
    /// The release source of the update check and of the update action, in place of the
    /// compiled one. Only a test configuration accepts it.
    update_source: Option<String>,
    /// Check for an update, install it with the update action of the window, then exit.
    check_update: bool,
}

enum Parsed {
    Run(Arguments),
    Print(String),
    Rollback,
}

/// `test_configuration`: whether this build is a test configuration, the only one that
/// accepts `--update-source` and `--check-update`.
fn parse(
    mut values: impl Iterator<Item = String>,
    test_configuration: bool,
) -> Result<Parsed, String> {
    let mut arguments = Arguments {
        data_dir: None,
        start: WindowStart::default(),
        check_startup: false,
        update_source: None,
        check_update: false,
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
            // The package checks show the update notice of the window and install from it.
            "--update-source" if test_configuration => {
                let source = values.next().ok_or("--update-source needs an address")?;
                arguments.update_source = Some(source);
            }
            "--check-update" if test_configuration => {
                arguments.check_update = true;
                arguments.start.activate = false;
            }
            "--update-source" | "--check-update" => {
                return Err(format!("{flag} is accepted only by a test configuration"));
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

/// The update source of this start: the compiled one, or the one of `--update-source`.
fn update_source_of(
    base: Option<&str>,
) -> Result<Option<junto_client_core::releases::HttpSource>, String> {
    use junto_client_core::releases::{HttpSource, Target, TrustedKeys, install::Package};
    match base {
        None => HttpSource::compiled().map_err(|error| error.to_string()),
        Some(base) => {
            let keys = TrustedKeys::compiled().map_err(|error| error.to_string())?;
            HttpSource::new(
                base,
                keys,
                junto_client_core::updates::Version::THIS_BUILD,
                Target::this_build(Package::running()),
            )
            .map(Some)
            .map_err(|error| error.to_string())
        }
    }
}

/// `--check-update`: the window checks its source, shows the notice, and installs the newer
/// version with its update action; the start then ends. The lines that it prints are what
/// the package checks read. It fails when no newer version is found in 300 seconds.
fn check_update_through_the_window(
    state: &gpui::Entity<junto_client::state::AppState>,
    reached: std::rc::Rc<std::cell::Cell<bool>>,
    cx: &mut gpui::App,
) {
    use junto_client::state::update_install::VersionAction;
    let told = std::rc::Rc::new(std::cell::Cell::new(false));
    let installing = std::rc::Rc::new(std::cell::Cell::new(false));
    let step = std::rc::Rc::new(
        move |state: gpui::Entity<junto_client::state::AppState>, cx: &mut gpui::App| {
            if told.get() {
                return;
            }
            let read = state.read(cx);
            let Some(update) = read.available_update() else {
                return;
            };
            if !installing.get() {
                installing.set(true);
                println!("Junto update notice: {}", update.version);
                let line = read.version_line();
                println!("Junto version line: {} {}", line.version, line.sentence);
                state.update(cx, |state, cx| {
                    state.act_on_version_line(cx);
                });
                return;
            }
            if read.update_installing() {
                return;
            }
            told.set(true);
            let line = read.version_line();
            if read.update_installed() && line.action == Some(VersionAction::Restart) {
                println!("installed version {}", update.version);
                println!("Junto restart offered: {}", line.sentence);
                reached.set(true);
            } else {
                eprintln!(
                    "the update was not installed: {}",
                    read.update_install_status().unwrap_or("no result")
                );
            }
            cx.quit();
        },
    );
    // The check of the start of the event streams finds the newer version: one request.
    // The state can hold its answer before the observer exists, so the step runs once now.
    let observed = step.clone();
    cx.observe(state, move |state, cx| observed(state, cx)).detach();
    let first = state.clone();
    cx.defer(move |cx| step(first, cx));
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(300))
            .await;
        eprintln!("no update was installed in 300 seconds");
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
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

/// The file of a directory of the account that states why the last start
/// failed before it had a data directory (ADR 0757).
const START_FAILURE_FILE: &str = "junto-start-failure.txt";

/// Tells why the start failed: on standard error, in the log, and, when nothing reads
/// standard error (a Windows program started from Explorer), in a message box of the system
/// that names the file that keeps the reason (ADR 0757).
fn start_failed(problem: &str, kept_in: Option<&Path>) {
    let read = junto_client_core::console::attach_to_parent();
    eprintln!("junto cannot start: {problem}");
    tracing::error!("junto cannot start: {problem}");
    if !read {
        junto_client_core::console::show_error(
            junto_client::text::APP_NAME,
            &start_failure_message(problem, kept_in),
        );
    }
}

fn start_failure_message(problem: &str, kept_in: Option<&Path>) -> String {
    let mut text = format!("Junto cannot start.\n\n{problem}");
    if let Some(path) = kept_in {
        text.push_str(&format!("\n\nThe reason is also in {}.", path.display()));
    }
    text
}

/// Writes why the start failed to [`START_FAILURE_FILE`], replacing an earlier one, and
/// returns its path.
// The start writes it before any window (ADR 0730).
#[allow(clippy::disallowed_methods)]
fn write_start_failure(problem: &str) -> Option<PathBuf> {
    // The temporary directory of the account on macOS and Windows; on Linux `/tmp` is shared
    // by every account, so the runtime directory of the account, or its cache directory.
    #[cfg(target_os = "linux")]
    let directory = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "linux"))]
    let directory = std::env::temp_dir();
    let path = directory.join(START_FAILURE_FILE);
    let time = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let text = format!(
        "{time} {}: junto cannot start: {problem}\n",
        junto_client_core::releases::build_label()
    );
    // A new file, which follows no link that is at its name.
    let _ = std::fs::remove_file(&path);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()));
    match written {
        Ok(()) => Some(path),
        Err(error) => {
            eprintln!("{} cannot be written: {error}", path.display());
            None
        }
    }
}

fn main() -> ExitCode {
    junto_client::panics::install();
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
    let chosen_dir = arguments.data_dir.is_some();
    let update_source = arguments.update_source.clone();
    let client = data_dir(arguments.data_dir).and_then(|directory| {
        let mut options = ClientOptions::new(directory);
        options.update_source = update_source_of(update_source.as_deref())?.map(|source| {
            std::sync::Arc::new(source)
                as std::sync::Arc<dyn junto_client_core::updates::UpdateSource>
        });
        Client::open(options).map_err(|error| error.to_string())
    });
    let client = match client {
        Ok(client) => client,
        Err(problem) => {
            // The files of the data directory are not changed by a start that fails
            // (ADR 0710). The person reads why and decides; the file of the temporary
            // directory keeps the reason also without a console (ADR 0757).
            let kept_in = write_start_failure(&problem);
            start_failed(&problem, kept_in.as_deref());
            return ExitCode::FAILURE;
        }
    };
    // The client made the data directory. The log of the directory is written by one
    // instance only: the one that holds `logs/lock` (ADR 0757).
    junto_client::panics::record_into(client.data_dir().root());
    let log_path = log_file
        .open(client.data_dir().root())
        .inspect_err(|error| tracing::warn!(%error, "the log file was not opened"))
        .ok();
    let start = arguments.start;
    let check_startup = arguments.check_startup;
    let check_update = arguments.check_update;
    let update_source = arguments.update_source;
    // A restart after an update starts the same data directory (ADR 0780).
    let restart_arguments: Vec<std::ffi::OsString> = if chosen_dir {
        vec!["--data-dir".into(), client.data_dir().root().as_os_str().to_owned()]
    } else {
        Vec::new()
    };
    let started = std::rc::Rc::new(std::cell::Cell::new(false));
    let reached = started.clone();
    gpui_platform::application()
        .with_assets(Assets)
        .with_restart_arguments(restart_arguments)
        .run(move |cx| {
            if let Err(error) = app::init(cx) {
                start_failed(&format!("{error:#}"), log_path.as_deref());
                log_file.written();
                cx.quit();
                return;
            }
            let state = app::state(client, start, cx);
            if update_source.is_some() {
                state.update(cx, |state, _| state.use_update_source(update_source.clone()));
            }
            if check_update {
                check_update_through_the_window(&state, reached.clone(), cx);
            }
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
                Ok(_) if check_update => {}
                Ok(_) => reached.set(true),
                Err(error) => {
                    start_failed(
                        &format!("its window cannot open: {error:#}"),
                        log_path.as_deref(),
                    );
                    log_file.written();
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
    fn only_a_test_configuration_takes_another_update_source_and_checks_the_update() {
        let arguments = || {
            ["--update-source", "http://127.0.0.1:1/", "--check-update"].map(str::to_owned)
        };
        assert!(matches!(
            parse(arguments().into_iter(), true),
            Ok(Parsed::Run(Arguments { update_source: Some(source), check_update: true, .. }))
                if source == "http://127.0.0.1:1/"
        ));
        assert!(matches!(
            parse(arguments().into_iter(), false),
            Err(problem) if problem.contains("test configuration")
        ));
        assert!(parse(["--check-update".to_owned()].into_iter(), false).is_err());
        assert!(parse(["--update-source".to_owned()].into_iter(), true).is_err());
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
    fn the_message_of_a_failed_start_names_the_reason_and_the_file_that_keeps_it() {
        let file = Path::new("/tmp/junto-start-failure.txt");
        assert_eq!(
            start_failure_message("the data directory is in use", Some(file)),
            format!(
                "Junto cannot start.\n\nthe data directory is in use\n\nThe reason is also in {}.",
                file.display()
            )
        );
        assert_eq!(
            start_failure_message("no file", None),
            "Junto cannot start.\n\nno file"
        );
    }

    #[test]
    fn a_relative_data_directory_is_taken_from_the_current_directory() {
        let chosen = data_dir(Some(PathBuf::from("first"))).unwrap();
        assert!(chosen.is_absolute() && chosen.ends_with("first"));
        // An absolute path of the system: `/tmp/first` has no drive on Windows.
        let absolute = std::env::temp_dir().join("first");
        assert_eq!(data_dir(Some(absolute.clone())).unwrap(), absolute);
    }
}
