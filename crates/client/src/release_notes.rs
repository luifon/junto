//! The release notes of the versions of the client, compiled into the build from
//! `docs/releases/<version>.md`, the texts of the GitHub releases (ADR 0727).

use std::sync::Arc;

use junto_client_core::updates::Version;

/// The notes of each version: the build carries them, and a test gives others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes(Arc<[(Version, &'static str)]>);

const BUILT: &[(u32, u32, u32, &str)] = include!(concat!(env!("OUT_DIR"), "/release-notes.rs"));

impl Default for ReleaseNotes {
    fn default() -> Self {
        Self::new(
            BUILT
                .iter()
                .map(|&(major, minor, patch, text)| (Version::new(major, minor, patch), text)),
        )
    }
}

impl ReleaseNotes {
    pub fn new(notes: impl IntoIterator<Item = (Version, &'static str)>) -> Self {
        let mut notes: Vec<(Version, &'static str)> = notes.into_iter().collect();
        notes.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
        Self(notes.into())
    }

    /// The notes of the versions after `last` up to `build`, the newest first: what a
    /// person who ran `last` before has not seen.
    pub fn after(&self, last: Version, build: Version) -> Vec<(Version, &'static str)> {
        self.0
            .iter()
            .filter(|(version, _)| last < *version && *version <= build)
            .copied()
            .collect()
    }

    pub fn of(&self, build: Version) -> Option<(Version, &'static str)> {
        self.0
            .iter()
            .find(|(version, _)| *version == build)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_after_a_version_are_those_up_to_the_build_newest_first() {
        let version = |minor| Version::new(0, minor, 0);
        let notes = ReleaseNotes::new([
            (version(1), "one"),
            (version(3), "three"),
            (version(2), "two"),
            (version(4), "four"),
        ]);
        assert_eq!(
            notes.after(version(1), version(3)),
            [(version(3), "three"), (version(2), "two")]
        );
        assert!(notes.after(version(3), version(3)).is_empty());
        assert!(notes.after(version(4), version(2)).is_empty());
        assert_eq!(notes.of(version(2)), Some((version(2), "two")));
        assert_eq!(notes.of(version(5)), None);
    }

    #[test]
    fn the_build_carries_the_notes_of_every_release() {
        let built = ReleaseNotes::default();
        let (version, text) = built
            .of(Version::new(0, 1, 0))
            .expect("the notes of 0.1.0 are in the build");
        assert_eq!(version, Version::new(0, 1, 0));
        assert!(text.starts_with("# Junto 0.1.0"));
    }
}
