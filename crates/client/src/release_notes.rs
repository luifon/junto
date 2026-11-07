//! The release notes of the versions of the client: the section "What is new" of each
//! `docs/releases/<version>.md`, which the build compiles in. The GitHub release shows the
//! whole file, with the files and the steps of a first install (ADR 0727).

use std::sync::Arc;

use junto_client_core::updates::Version;

/// The notes of each version. Only the start of the program gives the notes of the build
/// ([`ReleaseNotes::built`]); every other start, a test or the screens tool, has none unless
/// it gives some.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReleaseNotes(Arc<[(Version, &'static str)]>);

const BUILT: &[(u32, u32, u32, &str)] = include!(concat!(env!("OUT_DIR"), "/release-notes.rs"));

/// The heading of the section of a notes file that the client shows.
pub const SECTION: &str = "## What is new";

/// The text of the section "What is new" of a notes file, up to the next heading of its
/// level or above.
pub fn what_is_new(text: &str) -> Option<&str> {
    let (mut offset, mut start, mut end) = (0, None, text.len());
    for line in text.split_inclusive('\n') {
        if start.is_none() {
            if line.trim_end() == SECTION {
                start = Some(offset + line.len());
            }
        } else if line.starts_with("# ") || line.starts_with("## ") {
            end = offset;
            break;
        }
        offset += line.len();
    }
    let section = text[start?..end].trim();
    (!section.is_empty()).then_some(section)
}

impl ReleaseNotes {
    /// The section "What is new" of the notes of every version that the build carries.
    pub fn built() -> Self {
        Self::new(BUILT.iter().filter_map(|&(major, minor, patch, text)| {
            what_is_new(text).map(|section| (Version::new(major, minor, patch), section))
        }))
    }

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
    fn the_client_shows_only_the_section_what_is_new() {
        let text = "# Junto 0.2.0\n\nIntro.\n\n## What is new\n\n- Favourites\n\n### Calls\n\n\
                    - Sound off\n\n## Packages\n\n- junto.dmg\n";
        assert_eq!(
            what_is_new(text),
            Some("- Favourites\n\n### Calls\n\n- Sound off")
        );
        assert_eq!(
            what_is_new("# Junto 0.1.0\n\n## Contents\n\n- Chat\n"),
            None
        );
        assert_eq!(what_is_new("## What is new\n\n## Packages\n"), None);
        assert_eq!(what_is_new("## What is new\n- Last\n"), Some("- Last"));
    }

    #[test]
    fn only_the_start_of_the_program_carries_the_notes_of_the_build() {
        assert_eq!(
            crate::app::WindowStart::default().release_notes,
            ReleaseNotes::default()
        );
        assert!(ReleaseNotes::default().of(Version::new(0, 1, 0)).is_none());
        // The notes of 0.1.0 have no section "What is new": the build shows none of them.
        assert!(ReleaseNotes::built().of(Version::new(0, 1, 0)).is_none());
    }
}
