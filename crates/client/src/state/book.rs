//! The server book beyond its rows (ADR 0780): the order of the servers, and what the
//! window tells about a newer build of the client (ADR 0704).
//!
//! The update check belongs to the client core. This module derives from its state what
//! the window shows, and holds the check that the person asked for. Nothing is downloaded
//! and nothing is installed.

use gpui::Context;
use junto_client_core::book::BookError;
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::updates::{UpdateCheck, UpdateState, Version};
use junto_client_core::version::ServerVersion;

use super::AppState;
use crate::text;

/// What the update check of the client core holds, and the check that the person asked
/// for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatesKnown {
    /// The version of the build that runs.
    pub build: Version,
    /// False when no source of releases is configured: nothing is ever asked.
    pub has_source: bool,
    pub state: UpdateState,
    /// The version of the newest release that the source of releases names, also when it is
    /// not newer than this build.
    pub newest: Option<Version>,
    /// A check that the person asked for is on its way.
    pub checking: bool,
}

impl Default for UpdatesKnown {
    fn default() -> Self {
        Self {
            build: Version::THIS_BUILD,
            has_source: false,
            state: UpdateState::NotChecked,
            newest: None,
            checking: false,
        }
    }
}

/// A newer build that the source of releases offers. The channel column shows its notice
/// while the window knows of it, and the settings and the state of a server that needs a
/// newer client name its version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Available {
    pub version: Version,
}

/// The newer build that the window knows of: the release of the state "available" of the
/// update check.
pub fn available(known: &UpdatesKnown) -> Option<Available> {
    match &known.state {
        UpdateState::Available(release) => Some(Available {
            version: release.version,
        }),
        UpdateState::NotChecked
        | UpdateState::Current
        | UpdateState::Failed
        | UpdateState::Expired
        | UpdateState::UntrustedKeys => None,
    }
}

/// The version of the newest release when it comes after the version that a server states.
/// The server and the client of one release have one version, so the newest release of the
/// client is the newest release of the server (ADR 0780). Nothing without a source.
pub fn newer_server_release(known: &UpdatesKnown, server: &ServerVersion) -> Option<Version> {
    let newest = known.newest?;
    server.is_older_than(newest).then_some(newest)
}

/// Where the update check stands, as the settings state it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckShown {
    /// This build has no source of releases.
    NotConfigured,
    /// No check has ended yet.
    NotChecked,
    Current,
    Available {
        version: Version,
        /// The address of the page of the release. It is text of the source.
        page: Option<String>,
    },
    /// No check has succeeded.
    Failed,
    Expired,
    UntrustedKeys,
}

/// The section "Version" of the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionView {
    pub build: Version,
    pub check: CheckShown,
    /// A source exists: the person can ask for a check.
    pub can_check: bool,
    pub checking: bool,
}

pub fn version_view(known: &UpdatesKnown) -> VersionView {
    let check = match (&known.state, known.has_source) {
        (UpdateState::Available(release), _) => CheckShown::Available {
            version: release.version,
            page: release.page.as_ref().map(|page| page.as_str().to_owned()),
        },
        (_, false) => CheckShown::NotConfigured,
        (UpdateState::NotChecked, true) => CheckShown::NotChecked,
        (UpdateState::Current, true) => CheckShown::Current,
        (UpdateState::Failed, true) => CheckShown::Failed,
        (UpdateState::Expired, true) => CheckShown::Expired,
        (UpdateState::UntrustedKeys, true) => CheckShown::UntrustedKeys,
    };
    VersionView {
        build: known.build,
        check,
        can_check: known.has_source,
        checking: known.has_source && known.checking,
    }
}

/// A step of a server in the order of the server book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

/// The place that a step gives the server at `index` of a book of `count` servers, as
/// the client core counts it: the index in the list after the move. Absent at the end
/// that the step leads to.
pub fn place_after(index: usize, count: usize, step: Step) -> Option<usize> {
    match step {
        Step::Up => index.checked_sub(1),
        Step::Down => Some(index + 1).filter(|place| *place < count),
    }
}

/// What the application holds of the server book beside the client core.
#[derive(Debug, Default)]
pub(super) struct BookState {
    checking: bool,
    /// Why the last move of a server was not written, with the server.
    move_failed: Option<(ServerId, String)>,
    /// The state of the update check that is drawn in place of the one of the client.
    #[cfg(feature = "test-support")]
    fixture: Option<UpdatesKnown>,
}

impl BookState {
    fn known(&self, updates: &UpdateCheck) -> UpdatesKnown {
        #[cfg(feature = "test-support")]
        if let Some(fixture) = &self.fixture {
            return fixture.clone();
        }
        UpdatesKnown {
            build: updates.build(),
            has_source: updates.has_source(),
            state: updates.state(),
            newest: updates.newest(),
            checking: self.checking,
        }
    }
}

fn move_failure(error: &BookError) -> String {
    match error {
        BookError::NotSaved { .. } => text::book::MOVE_SERVER_GONE.to_owned(),
        other => text::flow_error(&FlowError::Storage(other.to_string())),
    }
}

impl AppState {
    /// Takes the state of the update check into what the view is derived from. It runs
    /// with every change of the client: a change of the update check is one.
    pub(super) fn follow_updates(&mut self) {
        self.known.updates = self.book.known(self.client.updates());
    }

    /// The section "Version" of the settings.
    pub fn version_view(&self) -> VersionView {
        version_view(&self.known.updates)
    }

    /// The newer build that the window knows of.
    pub fn available_update(&self) -> Option<Available> {
        available(&self.known.updates)
    }

    /// Asks the source of releases once. Nothing is asked without a source, and while a
    /// check that the person asked for is on its way. The client core makes one request
    /// at a time.
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) {
        let updates = self.client.updates();
        if !updates.has_source() || self.book.checking {
            return;
        }
        self.book.checking = true;
        let checking = updates.check();
        cx.spawn(async move |this, cx| {
            checking.await;
            let _ = this.update(cx, |this, cx| {
                this.book.checking = false;
                this.follow_updates();
                cx.notify();
            });
        })
        .detach();
        self.follow_updates();
        cx.notify();
    }

    /// Moves a server one place up or down in the server book. The file is written on a
    /// background thread after the writes that were asked for before, and the place is
    /// counted when the write runs: two steps that are asked for in a row move the server
    /// two places. The client core tells its change when the book is written. A failure
    /// is told in [`AppState::move_failure`].
    pub fn move_server(&mut self, server_id: ServerId, step: Step, cx: &mut Context<Self>) {
        self.book.move_failed = None;
        let client = self.client.clone();
        Self::queue_write(
            cx,
            move || {
                let servers = client.servers();
                let Some(index) = servers.iter().position(|server| server.id() == server_id) else {
                    return Err(BookError::NotSaved { server_id });
                };
                match place_after(index, servers.len(), step) {
                    Some(place) => client.move_server(server_id, place),
                    // The server is at that end already.
                    None => Ok(()),
                }
            },
            move |this, moved, cx| {
                if let Err(error) = moved {
                    tracing::warn!(%error, "the order of the server book was not saved");
                    this.book.move_failed = Some((server_id, move_failure(&error)));
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Why the last move of this server was not written.
    pub fn move_failure(&self, server_id: ServerId) -> Option<&str> {
        self.book
            .move_failed
            .as_ref()
            .filter(|(failed, _)| *failed == server_id)
            .map(|(_, sentence)| sentence.as_str())
    }

    /// Puts away what was told about the last move of a server.
    pub fn dismiss_move_failure(&mut self, cx: &mut Context<Self>) {
        if self.book.move_failed.take().is_some() {
            cx.notify();
        }
    }

    /// Draws this state of the update check in place of the one of the client. The screen
    /// tool uses it: the application has no source of releases.
    #[cfg(feature = "test-support")]
    pub fn show_updates_fixture(&mut self, updates: UpdatesKnown, cx: &mut Context<Self>) {
        self.book.fixture = Some(updates);
        self.changed(cx);
    }

    /// Draws this failure of a move of a server. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_move_failure_fixture(
        &mut self,
        server_id: ServerId,
        sentence: &str,
        cx: &mut Context<Self>,
    ) {
        self.book.move_failed = Some((server_id, sentence.to_owned()));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::updates::Release;

    use super::*;

    const BUILD: Version = Version::new(0, 3, 1);

    fn known(state: UpdateState) -> UpdatesKnown {
        UpdatesKnown {
            build: BUILD,
            has_source: true,
            state,
            newest: None,
            checking: false,
        }
    }

    fn release(version: Version, page: Option<&str>) -> UpdateState {
        UpdateState::Available(Release::new(version, page))
    }

    #[test]
    fn a_newer_release_of_a_server_is_the_newest_release_after_its_version() {
        let server = |text: &str| ServerVersion::parse(text).unwrap();
        let without = UpdatesKnown {
            has_source: false,
            ..known(UpdateState::NotChecked)
        };
        assert_eq!(newer_server_release(&without, &server("0.1.0")), None);
        let current = UpdatesKnown {
            newest: Some(BUILD),
            ..known(UpdateState::Current)
        };
        for (stated, newer) in [
            ("0.3.0", Some(BUILD)),
            ("0.3.1-test.2", Some(BUILD)),
            ("0.3.1", None),
            ("0.3.1+build.4", None),
            ("0.3.2", None),
        ] {
            let found = newer_server_release(&current, &server(stated));
            assert_eq!(found, newer, "{stated}");
        }
    }

    #[test]
    fn a_newer_release_is_known_while_the_check_says_available() {
        let newer = Version::new(0, 4, 0);
        for state in [
            UpdateState::NotChecked,
            UpdateState::Current,
            UpdateState::Failed,
        ] {
            assert_eq!(available(&known(state.clone())), None, "{state:?}");
        }
        let mut held = known(release(newer, None));
        assert_eq!(available(&held), Some(Available { version: newer }));
        // A check that the person asked for changes nothing of what is known.
        held.checking = true;
        assert_eq!(available(&held), Some(Available { version: newer }));
        // A still newer release takes the place of the one before.
        let newest = Version::new(0, 4, 1);
        held.state = release(newest, Some("https://junto.example.test/releases/0.4.1"));
        assert_eq!(available(&held), Some(Available { version: newest }));
        // The check found no newer release: nothing is known any more.
        held.state = UpdateState::Current;
        assert_eq!(available(&held), None);
    }

    #[test]
    fn the_settings_state_the_build_and_each_state_of_the_check() {
        let without = UpdatesKnown {
            has_source: false,
            checking: true,
            ..known(UpdateState::NotChecked)
        };
        let view = version_view(&without);
        assert_eq!(view.build, BUILD);
        assert_eq!(view.check, CheckShown::NotConfigured);
        // Without a source nothing can be asked, and nothing is on its way.
        assert!(!view.can_check && !view.checking);

        for (state, shown) in [
            (UpdateState::NotChecked, CheckShown::NotChecked),
            (UpdateState::Current, CheckShown::Current),
            (UpdateState::Failed, CheckShown::Failed),
        ] {
            let view = version_view(&known(state));
            assert_eq!(view.check, shown);
            assert!(view.can_check && !view.checking);
        }

        let newer = Version::new(0, 4, 0);
        let page = "https://junto.example.test/releases/0.4.0";
        let view = version_view(&known(release(newer, Some(page))));
        assert_eq!(
            view.check,
            CheckShown::Available {
                version: newer,
                page: Some(page.to_owned())
            }
        );
        // A page that is no https address is not shown.
        let view = version_view(&known(release(newer, Some("http://junto.example.test/"))));
        assert_eq!(
            view.check,
            CheckShown::Available {
                version: newer,
                page: None
            }
        );
        let checking = UpdatesKnown {
            checking: true,
            ..known(UpdateState::Current)
        };
        assert!(version_view(&checking).checking);
    }

    #[test]
    fn a_step_gives_the_next_place_and_none_at_the_end() {
        assert_eq!(place_after(0, 3, Step::Up), None);
        assert_eq!(place_after(1, 3, Step::Up), Some(0));
        assert_eq!(place_after(2, 3, Step::Up), Some(1));
        assert_eq!(place_after(0, 3, Step::Down), Some(1));
        assert_eq!(place_after(1, 3, Step::Down), Some(2));
        assert_eq!(place_after(2, 3, Step::Down), None);
        assert_eq!(place_after(0, 1, Step::Up), None);
        assert_eq!(place_after(0, 1, Step::Down), None);
        assert_eq!(place_after(0, 0, Step::Down), None);
    }

    #[test]
    fn a_move_that_failed_has_a_sentence() {
        let gone = move_failure(&BookError::NotSaved {
            server_id: ServerId::generate(),
        });
        assert_eq!(gone, text::book::MOVE_SERVER_GONE);
        let storage = junto_client_core::storage::StorageError::NotPrivate {
            path: std::path::PathBuf::from("servers.json"),
        };
        let failed = move_failure(&BookError::Storage(storage));
        assert!(failed.starts_with("A file of the data directory could not be used"));
    }
}
