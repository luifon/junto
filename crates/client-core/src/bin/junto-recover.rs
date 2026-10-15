//! The recovery program of an installed package. It restores the previous release from the
//! record beside the application with the keys and the certificate pin compiled into it, and
//! has no other command (ADR 0726).

use std::path::Path;
use std::process::ExitCode;

use junto_client_core::releases::PlatformInstaller;

const USAGE: &str = "usage: junto-recover rollback /absolute/path/to/the/application";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [command, application] = arguments.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if command != "rollback" || !Path::new(application).is_absolute() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    match PlatformInstaller::at(Path::new(application)).and_then(|installer| installer.rollback()) {
        Ok(version) => {
            println!("restored version {version}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
