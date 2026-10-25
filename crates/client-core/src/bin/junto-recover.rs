//! The recovery program of an installed package. It restores the previous release from the
//! record beside the application with the keys and the certificate pin compiled into it
//! (ADR 0726). On Windows it also removes the registration of the notifications of the
//! account, the one entry that the application writes outside its folder and its data
//! directory (ADR 0755).

use std::path::Path;
use std::process::ExitCode;

use junto_client_core::releases::PlatformInstaller;

#[cfg(not(windows))]
const USAGE: &str = "usage: junto-recover rollback /absolute/path/to/the/application";
#[cfg(windows)]
const USAGE: &str = "usage: junto-recover rollback C:\\absolute\\path\\to\\the\\application
       junto-recover unregister";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [command, application] if command == "rollback" && Path::new(application).is_absolute() => {
            rollback(Path::new(application))
        }
        #[cfg(windows)]
        [command] if command == "unregister" => unregister(),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn rollback(application: &Path) -> ExitCode {
    match PlatformInstaller::at(application).and_then(|installer| installer.rollback()) {
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

#[cfg(windows)]
fn unregister() -> ExitCode {
    use junto_client_core::app_identity;
    match app_identity::unregister(app_identity::APP_ID) {
        Ok(true) => {
            println!("removed the registration of the notifications of Junto");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("no registration of the notifications of Junto exists");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("the registration of the notifications was not removed: {error}");
            ExitCode::FAILURE
        }
    }
}
