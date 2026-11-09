use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{Profile, Startup, Status};
use crate::app_identity::APP_ID;

pub(super) struct Autostart {
    path: PathBuf,
    program: PathBuf,
    profile: Profile,
    inherited: Vec<PathBuf>,
    desktops: Vec<String>,
}

fn this_program() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    Ok(crate::releases::linux::running_appimage(&executable).unwrap_or(executable))
}

fn autostart_directory() -> io::Result<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::other("the configuration directory is not known"))?;
    Ok(config.join("autostart"))
}

/// The entry of a profile at login: `chat.junto.Junto.desktop` for the normal profile and
/// `chat.junto.Junto.profile.<key>.desktop` for another one (ADR 0754).
pub(super) fn running(profile: Profile) -> io::Result<Autostart> {
    let file = format!("{}.desktop", profile.entry_name("."));
    Ok(Autostart {
        path: autostart_directory()?.join(&file),
        program: this_program()?,
        inherited: std::env::split_paths(
            &std::env::var_os("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into()),
        )
        .filter(|path| path.is_absolute())
        .map(|path| path.join("autostart").join(&file))
        .collect(),
        profile,
        desktops: std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}

fn program_text(program: &Path) -> io::Result<&str> {
    program
        .to_str()
        .filter(|text| !text.chars().any(char::is_control))
        .ok_or_else(|| io::Error::other("the program path cannot be used in a desktop entry"))
}

fn value(text: &str) -> String {
    text.replace('\\', "\\\\").replace(' ', "\\s")
}

fn unescaped_value(text: &str) -> Option<String> {
    let mut result = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        result.push(if character == '\\' {
            match characters.next()? {
                '\\' => '\\',
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                _ => return None,
            }
        } else {
            character
        });
    }
    Some(result)
}

fn executable(path: &Path) -> bool {
    path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
}

fn find_program(program: &str) -> bool {
    if program.contains('/') {
        return executable(Path::new(program));
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| executable(&path.join(program)))
    })
}

fn quoted_argument(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        if matches!(character, '\\' | '"' | '$' | '`') {
            quoted.push('\\');
        }
        if character == '%' {
            quoted.push('%');
        }
        quoted.push(character);
    }
    quoted.push('"');
    value(&quoted)
}

/// The script of the entry of a profile other than the normal one: the program and the data
/// directory are its arguments.
const PROFILE_SCRIPT: &str = r#"exec "$0" --no-activate --data-dir "$1""#;

fn command(program: &Path, profile: &Profile) -> io::Result<String> {
    // GIO checks the Exec binary before expanding %%: pass the path as an argument.
    Ok(match profile {
        Profile::Normal => format!(
            "/bin/sh -c {} {}",
            quoted_argument(r#"exec "$0" --no-activate"#),
            quoted_argument(program_text(program)?)
        ),
        Profile::Other { directory, .. } => format!(
            "{} {}",
            profile_prefix(program)?,
            quoted_argument(program_text(directory)?)
        ),
    })
}

/// The start of the command of every profile entry of a program, before its data directory.
fn profile_prefix(program: &Path) -> io::Result<String> {
    Ok(format!(
        "/bin/sh -c {} {}",
        quoted_argument(PROFILE_SCRIPT),
        quoted_argument(program_text(program)?)
    ))
}

/// Removes the entries at login of this program: of the normal profile, as the settings do,
/// and every entry of another profile whose command starts this program. The entries of
/// another installation are left.
pub(super) fn remove_every_entry_of_this_program() -> io::Result<()> {
    let program = this_program()?;
    running(Profile::Normal)?.remove()?;
    remove_profile_entries(&autostart_directory()?, &program)
}

fn remove_profile_entries(directory: &Path, program: &Path) -> io::Result<()> {
    let prefix = format!("{} ", profile_prefix(program)?);
    let files = match fs::read_dir(directory) {
        Ok(files) => files,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for file in files {
        let path = file?.path();
        let named = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(&format!("{APP_ID}.profile.")) && name.ends_with(".desktop")
            });
        if !named {
            continue;
        }
        if let Some(entry) = Autostart::read_at(&path)?
            && field(&entry, "Exec").is_some_and(|exec| exec.starts_with(&prefix))
        {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let mut entry = false;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            entry = line == "[Desktop Entry]";
        }
        if entry
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == name
        {
            found = Some(value.trim());
        }
    }
    found
}

impl Autostart {
    pub(super) fn remove(&self) -> io::Result<()> {
        if let Some(entry) = Self::read_at(&self.path)? {
            if !self.owns(&entry)? {
                return Ok(());
            }
            fs::remove_file(&self.path)?;
        }
        if self.status()?.requested() {
            self.set(false)?;
        }
        Ok(())
    }

    fn read_at(path: &Path) -> io::Result<Option<String>> {
        match fs::symlink_metadata(path) {
            Ok(file) if !file.is_file() || file.file_type().is_symlink() => {
                return Err(io::Error::other(
                    "the autostart entry is not a regular file",
                ));
            }
            Ok(file) if file.len() > 64 * 1024 => {
                return Err(io::Error::other("the autostart entry is too large"));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
            _ => {}
        }
        fs::read_to_string(path).map(Some)
    }

    fn read(&self) -> io::Result<Option<String>> {
        for path in std::iter::once(&self.path).chain(&self.inherited) {
            if let Some(entry) = Self::read_at(path)? {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    fn owns(&self, entry: &str) -> io::Result<bool> {
        let Some(exec) = field(entry, "Exec") else {
            return Ok(false);
        };
        // The entry of another profile is this program with this directory, as written.
        if matches!(self.profile, Profile::Other { .. }) {
            return Ok(exec == command(&self.program, &self.profile)?);
        }
        let path = program_text(&self.program)?;
        let quoted = quoted_argument(path);
        let direct = exec == quoted
            || exec
                .strip_prefix(&quoted)
                .is_some_and(|rest| rest.starts_with(' '));
        let plain = !path
            .chars()
            .any(|character| character.is_whitespace() || "\"'\\$`%;&|<>()".contains(character))
            && (exec == path
                || exec
                    .strip_prefix(path)
                    .is_some_and(|rest| rest.starts_with(' ')));
        let alias = (exec == "junto" || exec.starts_with("junto "))
            && std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths)
                    .filter_map(|path| path.join("junto").canonicalize().ok())
                    .any(|candidate| {
                        self.program
                            .canonicalize()
                            .is_ok_and(|program| candidate == program)
                    })
            });
        Ok(exec == command(&self.program, &self.profile)? || direct || plain || alias)
    }
}

impl Startup for Autostart {
    fn status(&self) -> io::Result<Status> {
        let Some(entry) = self.read()? else {
            return Ok(Status::Off);
        };
        let in_list = |list: &str| {
            list.split(';')
                .any(|name| self.desktops.iter().any(|desktop| desktop == name))
        };
        let try_exec = match field(&entry, "TryExec") {
            None | Some("") => true,
            Some(path) => unescaped_value(path).is_some_and(|path| find_program(&path)),
        };
        let enabled = executable(&self.program)
            && try_exec
            && field(&entry, "Type") == Some("Application")
            && field(&entry, "DBusActivatable") != Some("true")
            && self.owns(&entry)?
            && field(&entry, "OnlyShowIn").is_none_or(in_list)
            && field(&entry, "NotShowIn").is_none_or(|list| !in_list(list))
            && field(&entry, "Hidden") != Some("true")
            && field(&entry, "X-GNOME-Autostart-enabled") != Some("false");
        Ok(if enabled { Status::On } else { Status::Off })
    }

    fn set(&self, enabled: bool) -> io::Result<Status> {
        let previous = self.read()?.unwrap_or_default();
        let command = command(&self.program, &self.profile)?;
        if !enabled && !self.owns(&previous)? {
            return Ok(Status::Off);
        }
        // The entry of another profile is the file of this program alone, and no system entry
        // has its name: it is removed, not disabled.
        if !enabled && matches!(self.profile, Profile::Other { .. }) {
            if Self::read_at(&self.path)?.is_some() {
                fs::remove_file(&self.path)?;
            }
            return self.status();
        }
        let own = [
            "Type",
            "Name",
            "Exec",
            "TryExec",
            "Terminal",
            "Hidden",
            "X-GNOME-Autostart-enabled",
            "OnlyShowIn",
            "NotShowIn",
            "DBusActivatable",
        ];
        let mut entry = false;
        let mut kept = String::new();
        let mut other = String::new();
        for line in previous.lines() {
            if line.trim().starts_with('[') {
                entry = line.trim() == "[Desktop Entry]";
            }
            let owned = entry
                && line
                    .split_once('=')
                    .is_some_and(|(key, _)| own.contains(&key.trim()));
            if line.trim() == "[Desktop Entry]" || owned {
                continue;
            }
            let target = if entry { &mut kept } else { &mut other };
            target.push_str(line);
            target.push('\n');
        }
        let content = format!(
            "[Desktop Entry]\nType=Application\nName={}\nExec={command}\nTryExec={}\nTerminal=false\nHidden={}\nX-GNOME-Autostart-enabled={}\n{kept}{other}",
            value(&self.profile.shown_name()),
            value(program_text(&self.program)?),
            !enabled,
            enabled
        );
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("no autostart directory"))?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".junto-{}.tmp",
            getrandom::u64().map_err(|_| io::Error::other("no random bytes"))?
        ));
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        let _ = fs::remove_file(temporary);
        result?;
        self.status()
    }
}

#[cfg(test)]
mod tests;
