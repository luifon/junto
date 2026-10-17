//! A window of a separate program for the tests of the Windows capture (ADR 0706). The
//! capture never lists a window of the program that captures, so the window that a test
//! captures belongs to this program.
//!
//! `junto-share-fixture <title> [--start <program> <argument>...]` opens a window with that
//! title whose inner area shows four quarters of known colours, above the other windows and
//! without taking the focus, and writes its lines to its standard output (`window::run`). It
//! reads the commands of the window from its standard input; the end of the input closes the
//! window. With `--start` it then starts the program as its own child, with the variable
//! `JUNTO_FIXTURE_WINDOW` set to the number of its window, and writes `ran <exit code>` when
//! that program has ended.
//!
//! `junto-share-fixture --report <file>` writes the identifier of the process that created
//! it and the path of that process's program into the file, as `<identifier> <path>`, and
//! ends.

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod report;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod window;

#[cfg(target_os = "windows")]
fn main() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, file] = arguments.as_slice()
        && flag == "--report"
    {
        std::process::exit(match std::fs::write(file, report::creator()) {
            Ok(()) => 0,
            Err(_) => 1,
        });
    }
    let title = arguments
        .first()
        .cloned()
        .unwrap_or_else(|| "Junto share fixture".to_owned());
    let start: Option<Vec<String>> = match arguments.get(1).map(String::as_str) {
        Some("--start") => Some(arguments[2..].to_vec()),
        _ => None,
    };
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let say = {
        let out = out.clone();
        move |line: String| {
            let mut out = out
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    };
    // The input thread of the window reads the commands.
    let commands = std::iter::from_fn(|| {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    });
    let started = say.clone();
    let code = window::run(&title, commands, move |line: String| {
        if let (Some(start), Some(number)) = (
            &start,
            line.strip_prefix("ready ")
                .and_then(|rest| rest.split_whitespace().next()),
        ) {
            let (start, number, say) = (start.clone(), number.to_owned(), started.clone());
            std::thread::spawn(move || {
                let code = std::process::Command::new(&start[0])
                    .args(&start[1..])
                    .env("JUNTO_FIXTURE_WINDOW", number)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .status()
                    .ok()
                    .and_then(|status| status.code())
                    .unwrap_or(-1);
                say(format!("ran {code}"));
            });
        }
        say(line);
    });
    std::process::exit(code);
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the share fixture is a window of Windows");
    std::process::exit(2);
}
