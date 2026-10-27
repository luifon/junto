//! The setup program of Windows and the uninstall program of the installed folder
//! (ADR 0728). With an archive at its end it installs Junto for the account and starts it;
//! without one, as `uninstall.exe` of the folder, it removes Junto. It opens no console.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("junto-installer runs on Windows");
    ExitCode::from(2)
}

#[cfg(windows)]
fn main() -> ExitCode {
    windows_main::run()
}

#[cfg(windows)]
mod windows_main {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::Duration;

    use junto_client_core::releases::setup::{self, Places};
    use junto_client_core::releases::setup_windows::{
        Answer, Buttons, WindowsSetup, ask, remove_after_end, runs, start, wait_for_end,
    };

    const TITLE: &str = "Junto";
    const USAGE: &str =
        "usage: Junto-<version>-windows-x86_64-setup.exe [--quiet] [--no-start] [--allow-older]
       uninstall.exe [--quiet] [--remove-data]";

    struct Options {
        quiet: bool,
        no_start: bool,
        allow_older: bool,
        remove_data: bool,
        /// The uninstall program runs as a copy outside the folder: the process number of the
        /// program that started it, and the folder.
        copy_of: Option<(u32, PathBuf)>,
    }

    fn options() -> Result<Options, String> {
        let mut options = Options {
            quiet: false,
            no_start: false,
            allow_older: false,
            remove_data: false,
            copy_of: None,
        };
        let mut arguments = std::env::args_os().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--quiet") => options.quiet = true,
                Some("--no-start") => options.no_start = true,
                Some("--allow-older") => options.allow_older = true,
                Some("--remove-data") => options.remove_data = true,
                Some("--from-copy") => {
                    let process = arguments
                        .next()
                        .and_then(|value| value.to_str().and_then(|text| text.parse().ok()))
                        .ok_or("--from-copy needs a process number")?;
                    let folder = arguments.next().ok_or("--from-copy needs a folder")?;
                    options.copy_of = Some((process, PathBuf::from(folder)));
                }
                _ => return Err(USAGE.to_owned()),
            }
        }
        Ok(options)
    }

    fn account() -> Result<(PathBuf, PathBuf), String> {
        let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
        let roaming = std::env::var_os("APPDATA").ok_or("APPDATA is not set")?;
        Ok((PathBuf::from(local), PathBuf::from(roaming)))
    }

    /// Tells the person, or writes to standard error in a quiet run.
    fn tell(options: &Options, text: &str, warning: bool) {
        if options.quiet {
            eprintln!("{text}");
        } else {
            ask(TITLE, text, Buttons::Ok, warning);
        }
    }

    pub fn run() -> ExitCode {
        let options = match options() {
            Ok(options) => options,
            Err(problem) => {
                ask(TITLE, &problem, Buttons::Ok, true);
                return ExitCode::from(2);
            }
        };
        let program = match std::env::current_exe() {
            Ok(program) => program,
            Err(error) => {
                tell(
                    &options,
                    &format!("Junto cannot find this program: {error}"),
                    true,
                );
                return ExitCode::FAILURE;
            }
        };
        let result = if let Some((process, folder)) = options.copy_of.clone() {
            remove_as_copy(&options, process, &folder, &program)
        } else if setup::holds_payload(&program) {
            install(&options, &program)
        } else {
            uninstall(&options, &program)
        };
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(problem) => {
                tell(&options, &problem, true);
                ExitCode::FAILURE
            }
        }
    }

    /// Asks the person to quit the Junto of this folder while it runs.
    fn wait_for_quit(options: &Options, program: &Path, doing: &str) -> Result<(), String> {
        while runs(program) {
            if options.quiet {
                return Err(format!("Junto is running; quit it before {doing}."));
            }
            let text =
                format!("Junto is running. Quit Junto, then choose Retry to continue {doing}.");
            if ask(TITLE, &text, Buttons::RetryCancel, true) != Answer::Yes {
                return Err(format!("Junto was not changed: {doing} was cancelled."));
            }
        }
        Ok(())
    }

    fn install(options: &Options, setup_program: &Path) -> Result<(), String> {
        let archive = setup::payload(setup_program).map_err(|error| {
            format!("This setup program is damaged. Download it again. ({error})")
        })?;
        let (local, roaming) = account()?;
        let places = Places::of_account(&local, &roaming);
        wait_for_quit(options, &places.program(), "the installation")?;
        let installed = setup::install(&archive, &places, options.allow_older, &WindowsSetup)
            .map_err(|error| format!("Junto was not installed: {error}."))?;
        println!("installed version {}", installed.version);
        if !options.no_start {
            start(&places.program(), &[])
                .map_err(|error| format!("Junto was installed but did not start: {error}."))?;
        }
        Ok(())
    }

    fn uninstall(options: &Options, program: &Path) -> Result<(), String> {
        let folder = program
            .parent()
            .ok_or("the folder of this program is not known")?
            .to_owned();
        let (local, roaming) = account()?;
        let places = Places::of_folder(&folder, &local, &roaming);
        let mut remove_data = options.remove_data;
        if !options.quiet {
            let text = "Remove Junto from this account? This removes the program, its Start menu entry and its entry in the list of installed programs.";
            if ask(TITLE, text, Buttons::OkCancel, false) != Answer::Yes {
                return Ok(());
            }
            let text = format!(
                "Also remove your Junto data from {}? It holds your servers, the keys of this device and your settings. Without it, Junto starts as a new device. Choose No to keep it.",
                places.data.display()
            );
            remove_data = ask(TITLE, &text, Buttons::YesNo, true) == Answer::Yes;
        }
        wait_for_quit(options, &places.program(), "the removal")?;
        // The folder holds this program, which Windows does not let a running process
        // delete: a copy outside the folder removes it after this process ended.
        let copy = std::env::temp_dir().join(format!("junto-uninstall-{}.exe", std::process::id()));
        std::fs::copy(program, &copy)
            .map_err(|error| format!("Junto was not removed: {error}."))?;
        let process = std::process::id().to_string();
        let mut arguments: Vec<&OsStr> = vec![
            OsStr::new("--from-copy"),
            OsStr::new(&process),
            folder.as_os_str(),
        ];
        if remove_data {
            arguments.push(OsStr::new("--remove-data"));
        }
        if options.quiet {
            arguments.push(OsStr::new("--quiet"));
        }
        start(&copy, &arguments).map_err(|error| format!("Junto was not removed: {error}."))
    }

    fn remove_as_copy(
        options: &Options,
        process: u32,
        folder: &Path,
        copy: &Path,
    ) -> Result<(), String> {
        let (local, roaming) = account()?;
        let places = Places::of_folder(folder, &local, &roaming);
        // The uninstall program of the folder ends after it started this copy; until then
        // its file cannot be removed.
        wait_for_end(process, Duration::from_secs(60));
        let last = setup::uninstall(&places, options.remove_data, &WindowsSetup);
        // The copy is removed after it ended.
        let _ = remove_after_end(copy, std::process::id());
        match last {
            Ok(()) => {
                if !options.quiet {
                    ask(TITLE, "Junto was removed.", Buttons::Ok, false);
                }
                Ok(())
            }
            Err(error) => Err(format!(
                "Junto was not removed completely: {error}. Remove the folder {} by hand.",
                folder.display()
            )),
        }
    }
}
