use super::{Startup, Status};
use crate::app_identity::APP_ID;
use std::io;
use std::path::{Path, PathBuf};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

pub(super) struct Login {
    program: PathBuf,
}

pub(super) fn running() -> io::Result<Login> {
    Ok(Login {
        program: std::env::current_exe()?,
    })
}

fn command(program: &Path) -> io::Result<String> {
    let path = program
        .to_str()
        .filter(|path| !path.contains('"') && !path.chars().any(char::is_control))
        .ok_or_else(|| io::Error::other("the program path cannot be used at login"))?;
    Ok(format!("\"{path}\" --no-activate"))
}

fn matches_program(registered: Option<&str>, program: &Path) -> bool {
    let Some(path) = registered
        .and_then(|entry| entry.strip_suffix(" --no-activate"))
        .and_then(|entry| entry.strip_prefix('"'))
        .and_then(|entry| entry.strip_suffix('"'))
    else {
        return false;
    };
    match (Path::new(path).canonicalize(), program.canonicalize()) {
        (Ok(registered), Ok(program)) => registered == program,
        _ => program
            .to_str()
            .is_some_and(|program| path.eq_ignore_ascii_case(program)),
    }
}

fn registered() -> io::Result<Option<String>> {
    match windows_registry::CURRENT_USER.open(RUN) {
        Ok(key) => match key.get_string(APP_ID) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code().0 == NOT_FOUND => Ok(None),
            Err(error) => Err(io::Error::other(error.to_string())),
        },
        Err(error) if error.code().0 == NOT_FOUND => Ok(None),
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

impl Startup for Login {
    fn status(&self) -> io::Result<Status> {
        Ok(
            if matches_program(registered()?.as_deref(), &self.program) {
                Status::On
            } else {
                Status::Off
            },
        )
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        if enabled {
            let command = command(&self.program)?;
            windows_registry::CURRENT_USER
                .create(RUN)
                .and_then(|key| key.set_string(APP_ID, &command))
                .map_err(|error| io::Error::other(error.to_string()))?;
        } else if matches_program(registered()?.as_deref(), &self.program) {
            windows_registry::CURRENT_USER
                .options()
                .write()
                .open(RUN)
                .and_then(|key| key.remove_value(APP_ID))
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        self.status()
    }
}

pub(super) fn remove_for(program: &Path) -> io::Result<()> {
    Login {
        program: program.to_owned(),
    }
    .set(false)
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_run_command_quotes_the_program_and_rejects_an_argument_injection() {
        assert_eq!(
            command(Path::new(r"C:\Program Files\Junto\junto.exe")).unwrap(),
            r#""C:\Program Files\Junto\junto.exe" --no-activate"#
        );
        assert!(command(Path::new("C:\\bad\" path\\junto.exe")).is_err());
        assert!(matches_program(
            Some(r#""c:\example\junto.exe" --no-activate"#),
            Path::new(r"C:\Example\junto.exe")
        ));
        assert!(!matches_program(
            Some(r#""C:\Other\junto.exe" --no-activate"#),
            Path::new(r"C:\Example\junto.exe")
        ));
    }
}
