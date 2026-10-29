//! The Windows URL registration of an installed copy.
use std::io;
use std::path::Path;

fn command(program: &Path) -> io::Result<String> {
    let path = program
        .to_str()
        .filter(|path| !path.contains('"') && !path.chars().any(char::is_control))
        .ok_or_else(|| io::Error::other("the program path cannot be registered"))?;
    Ok(format!("\"{path}\" \"%1\""))
}

fn matches_program(registered: &str, program: &Path) -> bool {
    let Some(path) = registered
        .strip_suffix("\" \"%1\"")
        .and_then(|path| path.strip_prefix('"'))
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

#[cfg(windows)]
mod windows {
    use super::*;
    const KEY: &str = r"Software\Classes\junto";
    const OPEN: &str = r"Software\Classes\junto\shell\open\command";
    const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;
    fn error(error: impl std::fmt::Display) -> io::Error {
        io::Error::other(error.to_string())
    }
    pub fn register(program: &Path) -> io::Result<()> {
        let command = command(program)?;
        let key = windows_registry::CURRENT_USER.create(KEY).map_err(error)?;
        key.set_string("", "URL:Junto").map_err(error)?;
        key.set_string("URL Protocol", "").map_err(error)?;
        windows_registry::CURRENT_USER
            .create(OPEN)
            .and_then(|key| key.set_string("", &command))
            .map_err(error)
    }
    pub fn unregister(program: &Path) -> io::Result<()> {
        let registered = match windows_registry::CURRENT_USER.open(OPEN) {
            Ok(key) => key.get_string("").map_err(error)?,
            Err(failure) if failure.code().0 == NOT_FOUND => return Ok(()),
            Err(failure) => return Err(error(failure)),
        };
        if matches_program(&registered, program) {
            windows_registry::CURRENT_USER
                .remove_tree(KEY)
                .map_err(error)?;
        }
        Ok(())
    }
}
#[cfg(windows)]
pub use windows::{register, unregister};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_scheme_names_one_program_and_a_quoted_url() {
        let program = Path::new(r"C:\Program Files\Junto\junto.exe");
        let registered = command(program).unwrap();
        assert_eq!(registered, r#""C:\Program Files\Junto\junto.exe" "%1""#);
        assert!(matches_program(&registered, program));
        assert!(!matches_program(
            &registered,
            Path::new(r"C:\Other\junto.exe")
        ));
        assert!(!matches_program(
            r#""C:\Other\junto.exe" "%1" --extra"#,
            program
        ));
        assert!(command(Path::new("bad\"path")).is_err());
    }
}
