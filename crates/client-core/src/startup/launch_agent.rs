//! The entry at login of a profile other than the normal one on macOS (ADR 0730): a launch
//! agent of the account, `~/Library/LaunchAgents/chat.junto.Junto.profile.<key>.plist`,
//! that starts the program of the installed bundle with the data directory of the profile.
//! The login service of the system (`SMAppService.mainAppService`) starts only the
//! application itself, without arguments, so it starts the normal profile alone.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::{Profile, Startup, Status};

pub(super) struct Agent {
    /// The file of the launch agent.
    pub path: PathBuf,
    pub program: PathBuf,
    pub profile: Profile,
}

/// Escapes text for an XML property list.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Agent {
    /// The property list of the agent: its label, the program with the arguments of the
    /// profile, a start at the login of the graphical session, and no restart.
    fn content(&self) -> io::Result<String> {
        let program = self
            .program
            .to_str()
            .filter(|program| !program.chars().any(char::is_control))
            .ok_or_else(|| io::Error::other("the program path cannot be used at login"))?;
        let arguments: String = std::iter::once(program.to_owned())
            .chain(self.profile.arguments())
            .map(|argument| format!("\t\t<string>{}</string>\n", escaped(&argument)))
            .collect();
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n{arguments}\t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>LimitLoadToSessionType</key>\n\
             \t<string>Aqua</string>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n",
            escaped(&self.profile.entry_name(".")),
        ))
    }

    fn read(&self) -> io::Result<Option<String>> {
        match fs::symlink_metadata(&self.path) {
            Ok(file) if !file.is_file() || file.file_type().is_symlink() => {
                Err(io::Error::other("the launch agent is not a regular file"))
            }
            Ok(file) if file.len() > 64 * 1024 => {
                Err(io::Error::other("the launch agent is too large"))
            }
            Ok(_) => fs::read_to_string(&self.path).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Startup for Agent {
    /// On while the file is the agent that this program writes for this profile. A person
    /// who turns the agent off in the Login Items of System Settings leaves the file; the
    /// state of that switch is not read (ADR 0730).
    fn status(&self) -> io::Result<Status> {
        Ok(match self.read()? {
            Some(content) if content == self.content()? => Status::On,
            _ => Status::Off,
        })
    }

    fn set(&self, enabled: bool) -> io::Result<Status> {
        if !enabled {
            if self.status()? == Status::On {
                fs::remove_file(&self.path)?;
            }
            return self.status();
        }
        let content = self.content()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("no launch agent directory"))?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".junto-{}.tmp",
            getrandom::u64().map_err(|_| io::Error::other("no random bytes"))?
        ));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        let _ = fs::remove_file(&temporary);
        result?;
        self.status()
    }
}

/// The directory of the launch agents of the account.
pub(super) fn directory() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| Path::new(&home).join("Library/LaunchAgents"))
        .ok_or_else(|| io::Error::other("the home directory is not known"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(root: &Path, name: &str) -> Agent {
        let directory = root.join("profiles").join(name);
        fs::create_dir_all(&directory).unwrap();
        let profile = Profile::of(&directory, None).unwrap();
        Agent {
            path: root.join(format!("LaunchAgents/{}.plist", profile.entry_name("."))),
            program: PathBuf::from("/Applications/Junto.app/Contents/MacOS/junto"),
            profile,
        }
    }

    /// The agent of a profile names it and starts the program with its directory; turning
    /// it off removes its file, and the agent of another profile is left.
    #[test]
    fn each_profile_has_its_own_launch_agent_with_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let work = agent(root.path(), "Work & <test>");
        let other = agent(root.path(), "other");
        assert_eq!(work.status().unwrap(), Status::Off);
        assert_eq!(work.set(true).unwrap(), Status::On);
        assert_eq!(other.status().unwrap(), Status::Off);
        let content = fs::read_to_string(&work.path).unwrap();
        let Profile::Other { key, directory, .. } = &work.profile else {
            panic!("{:?}", work.profile);
        };
        assert!(content.contains(&format!("<string>chat.junto.Junto.profile.{key}</string>")));
        assert!(content.contains("<string>--data-dir</string>"));
        assert!(content.contains(&format!(
            "<string>{}</string>",
            escaped(&directory.to_string_lossy())
        )));
        assert!(content.contains("Work &amp; &lt;test&gt;"));
        assert!(!content.contains("Work & <test>"));
        other.set(true).unwrap();
        assert_eq!(work.set(false).unwrap(), Status::Off);
        assert!(!work.path.exists());
        assert_eq!(other.status().unwrap(), Status::On);
        // A file that this program did not write for the profile is not taken as its agent,
        // and is not removed.
        fs::write(&work.path, "<plist/>").unwrap();
        assert_eq!(work.status().unwrap(), Status::Off);
        assert_eq!(work.set(false).unwrap(), Status::Off);
        assert!(work.path.exists());
        assert_eq!(work.set(true).unwrap(), Status::On);
        let entries: Vec<_> = fs::read_dir(root.path().join("LaunchAgents"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 2, "{entries:?}");
    }
}
