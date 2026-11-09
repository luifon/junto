use super::{Profile, Startup, Status};
use crate::app_identity::APP_ID;
use std::io;
use std::path::{Path, PathBuf};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

/// The Run value of one profile: `chat.junto.Junto` for the normal profile and
/// `chat.junto.Junto.profile.<key>` for another one, which starts the program with its data
/// directory (ADR 0730).
pub(super) struct Login {
    program: PathBuf,
    profile: Profile,
}

pub(super) fn running(profile: Profile) -> io::Result<Login> {
    Ok(Login {
        program: std::env::current_exe()?,
        profile,
    })
}

fn usable(path: &Path) -> io::Result<&str> {
    path.to_str()
        .filter(|path| !path.contains('"') && !path.chars().any(char::is_control))
        .ok_or_else(|| io::Error::other("the program path cannot be used at login"))
}

fn command(program: &Path, profile: &Profile) -> io::Result<String> {
    let path = usable(program)?;
    Ok(match profile {
        Profile::Normal => format!("\"{path}\" --no-activate"),
        Profile::Other { directory, .. } => {
            let directory = usable(directory)?;
            format!("\"{path}\" --no-activate --data-dir \"{directory}\"")
        }
    })
}

/// The program and the data directory of a Run value that this application wrote.
fn parse(registered: &str) -> Option<(&str, Option<&str>)> {
    let rest = registered.strip_prefix('"')?;
    let (program, rest) = rest.split_once('"')?;
    let rest = rest.strip_prefix(" --no-activate")?;
    if rest.is_empty() {
        return Some((program, None));
    }
    let directory = rest
        .strip_prefix(" --data-dir \"")?
        .strip_suffix('"')
        .filter(|directory| !directory.contains('"'))?;
    Some((program, Some(directory)))
}

fn same_path(registered: &str, path: &Path) -> bool {
    match (Path::new(registered).canonicalize(), path.canonicalize()) {
        (Ok(registered), Ok(path)) => registered == path,
        _ => path
            .to_str()
            .is_some_and(|path| registered.eq_ignore_ascii_case(path)),
    }
}

fn matches_program(registered: Option<&str>, program: &Path) -> bool {
    registered
        .and_then(parse)
        .is_some_and(|(path, _)| same_path(path, program))
}

fn matches(registered: Option<&str>, program: &Path, profile: &Profile) -> bool {
    let Some((path, directory)) = registered.and_then(parse) else {
        return false;
    };
    let directory_matches = match (profile, directory) {
        (Profile::Normal, None) => true,
        (Profile::Other { directory: own, .. }, Some(directory)) => same_path(directory, own),
        _ => false,
    };
    directory_matches && same_path(path, program)
}

fn registered(name: &str) -> io::Result<Option<String>> {
    match windows_registry::CURRENT_USER.open(RUN) {
        Ok(key) => match key.get_string(name) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code().0 == NOT_FOUND => Ok(None),
            Err(error) => Err(io::Error::other(error.to_string())),
        },
        Err(error) if error.code().0 == NOT_FOUND => Ok(None),
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

fn remove(name: &str) -> io::Result<()> {
    windows_registry::CURRENT_USER
        .options()
        .write()
        .open(RUN)
        .and_then(|key| key.remove_value(name))
        .map_err(|error| io::Error::other(error.to_string()))
}

impl Startup for Login {
    fn status(&self) -> io::Result<Status> {
        let name = self.profile.entry_name(".");
        Ok(
            if matches(registered(&name)?.as_deref(), &self.program, &self.profile) {
                Status::On
            } else {
                Status::Off
            },
        )
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        let name = self.profile.entry_name(".");
        if enabled {
            let command = command(&self.program, &self.profile)?;
            windows_registry::CURRENT_USER
                .create(RUN)
                .and_then(|key| key.set_string(&name, &command))
                .map_err(|error| io::Error::other(error.to_string()))?;
        } else if matches(registered(&name)?.as_deref(), &self.program, &self.profile) {
            remove(&name)?;
        }
        self.status()
    }
}

/// Whether a Run value is one that this application names: of the normal profile or of
/// another one.
fn named_by_this_application(name: &str) -> bool {
    name == APP_ID || name.starts_with(&format!("{APP_ID}.profile."))
}

/// Removes the Run values of a program, of every profile, as the uninstallation of the
/// program does (ADR 0728). The values of another installation are left.
pub(super) fn remove_for(program: &Path) -> io::Result<()> {
    let names: Vec<String> = match windows_registry::CURRENT_USER.open(RUN) {
        Ok(key) => key
            .values()
            .map_err(|error| io::Error::other(error.to_string()))?
            .map(|(name, _)| name)
            .filter(|name| named_by_this_application(name))
            .collect(),
        Err(error) if error.code().0 == NOT_FOUND => return Ok(()),
        Err(error) => return Err(io::Error::other(error.to_string())),
    };
    for name in names {
        if matches_program(registered(&name)?.as_deref(), program) {
            remove(&name)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_run_command_quotes_the_program_and_rejects_an_argument_injection() {
        assert_eq!(
            command(
                Path::new(r"C:\Program Files\Junto\junto.exe"),
                &Profile::Normal
            )
            .unwrap(),
            r#""C:\Program Files\Junto\junto.exe" --no-activate"#
        );
        assert!(command(Path::new("C:\\bad\" path\\junto.exe"), &Profile::Normal).is_err());
        assert!(matches_program(
            Some(r#""c:\example\junto.exe" --no-activate"#),
            Path::new(r"C:\Example\junto.exe")
        ));
        assert!(!matches_program(
            Some(r#""C:\Other\junto.exe" --no-activate"#),
            Path::new(r"C:\Example\junto.exe")
        ));
    }

    /// The Run value of another profile names it and starts the program with its data
    /// directory; it matches only its own profile, and the removal of the program finds it.
    #[test]
    fn the_run_value_of_another_profile_names_it_and_starts_its_directory() {
        let program = Path::new(r"C:\Program Files\Junto\junto.exe");
        let work = Profile::Other {
            key: "0123456789abcdef".to_owned(),
            name: "Work".to_owned(),
            directory: PathBuf::from(r"D:\Profiles\Work"),
        };
        let other = Profile::Other {
            key: "fedcba9876543210".to_owned(),
            name: "Other".to_owned(),
            directory: PathBuf::from(r"D:\Profiles\Other"),
        };
        assert_eq!(
            work.entry_name("."),
            "chat.junto.Junto.profile.0123456789abcdef"
        );
        let written = command(program, &work).unwrap();
        assert_eq!(
            written,
            r#""C:\Program Files\Junto\junto.exe" --no-activate --data-dir "D:\Profiles\Work""#
        );
        assert!(matches(Some(&written), program, &work));
        assert!(!matches(Some(&written), program, &other));
        assert!(!matches(Some(&written), program, &Profile::Normal));
        assert!(!matches(
            Some(&command(program, &Profile::Normal).unwrap()),
            program,
            &work
        ));
        assert!(matches_program(Some(&written), program));
        assert!(named_by_this_application("chat.junto.Junto"));
        assert!(named_by_this_application(
            "chat.junto.Junto.profile.0123456789abcdef"
        ));
        assert!(!named_by_this_application("chat.junto.JuntoOther"));
        assert!(
            command(
                program,
                &Profile::Other {
                    key: "0".to_owned(),
                    name: "bad".to_owned(),
                    directory: PathBuf::from("D:\\a\"b"),
                }
            )
            .is_err()
        );
        assert_eq!(
            parse(r#""C:\a.exe" --no-activate --data-dir "D:\x"extra"#),
            None
        );
    }
}
