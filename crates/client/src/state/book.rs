//! The server book beyond its rows (ADR 0780): the order of the servers, the arrangement of
//! the person (ADR 0775), and what the window tells about a newer build of the client
//! (ADR 0704).
//!
//! The update check belongs to the client core. This module derives from its state what
//! the window shows, and holds the check that the person asked for. Nothing is downloaded
//! and nothing is installed.

use gpui::Context;
use junto_client_core::Client;
use junto_client_core::arrangement::{self as layout, Arrangement, Change, GroupId, Item, Shelf};
use junto_client_core::book::BookError;
use junto_client_core::flow::FlowError;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};
use junto_client_core::protocol::message::MessagePlace;
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

/// A step of a server, a group or a workspace in the order of the server book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

impl Step {
    fn up(self) -> bool {
        self == Step::Up
    }
}

/// What a drag in the server book carries (ADR 0775).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dragged {
    Server(ServerId),
    Group(GroupId),
    Workspace(ServerId, WorkspaceId),
}

/// What a drag in the server book is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    /// The head of a server.
    Server(ServerId),
    /// The head of a group.
    Group(GroupId),
    Workspace(ServerId, WorkspaceId),
    /// The free space after the last part of the server book.
    End,
}

/// Where on its target a drag is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Before,
    Into,
    After,
}

/// A drop of a drag in the server book, while it is over its target: what is dropped where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookDrop {
    pub target: DropTarget,
    pub zone: Zone,
}

/// True when the target takes what is dragged: a server or a group anywhere in the list of
/// servers, a workspace only among the workspaces of its own server.
pub fn accepts(dragged: Dragged, target: DropTarget) -> bool {
    match (dragged, target) {
        (Dragged::Server(_) | Dragged::Group(_), DropTarget::Workspace(..)) => false,
        (Dragged::Server(_) | Dragged::Group(_), _) => true,
        (Dragged::Workspace(server, _), DropTarget::Workspace(other, _)) => server == other,
        (Dragged::Workspace(..), _) => false,
    }
}

/// True when the target can take a dragged server into it: the head of another server makes
/// a group, the head of a group takes the server in.
pub fn takes_into(dragged: Dragged, target: DropTarget) -> bool {
    matches!(
        (dragged, target),
        (
            Dragged::Server(_),
            DropTarget::Server(_) | DropTarget::Group(_)
        )
    )
}

/// The zone of a drag at the height `at` from the top of a target of height `height`: the
/// upper and the lower third with the middle third for a target that takes a server into
/// it, the upper and the lower half otherwise.
pub fn zone(at: f32, height: f32, into: bool) -> Zone {
    let share = if height > 0.0 { at / height } else { 0.5 };
    if into {
        if share < 1.0 / 3.0 {
            Zone::Before
        } else if share > 2.0 / 3.0 {
            Zone::After
        } else {
            Zone::Into
        }
    } else if share < 0.5 {
        Zone::Before
    } else {
        Zone::After
    }
}

/// The change that a drop makes, from the top-level parts of the server book. A drop on
/// itself, and a drop that a target does not take, make none.
pub fn dropped(dragged: Dragged, drop: BookDrop, shelves: &[Shelf]) -> Option<Change> {
    use layout::{Spot, WorkspaceSpot};
    if !accepts(dragged, drop.target) {
        return None;
    }
    let side = |item: Item| match drop.zone {
        Zone::Before => Spot::Before(item),
        Zone::Into | Zone::After => Spot::After(item),
    };
    match (dragged, drop.target) {
        (Dragged::Server(server), DropTarget::Server(target)) => {
            if server == target {
                return None;
            }
            Some(match drop.zone {
                Zone::Into => Change::GroupWith { server, target },
                _ => Change::MoveServer {
                    server,
                    to: side(Item::Server(target)),
                },
            })
        }
        (Dragged::Server(server), DropTarget::Group(group)) => {
            let to = match drop.zone {
                Zone::Before => Spot::Before(Item::Group(group)),
                Zone::Into => Spot::InGroup(group),
                Zone::After => {
                    // Under the head of an expanded group: the first place of the group.
                    let first = shelves.iter().find_map(|shelf| match shelf {
                        Shelf::Group {
                            group: held,
                            servers,
                        } if held.id == group => Some((held.collapsed, servers.first().copied())),
                        _ => None,
                    })?;
                    match first {
                        (false, Some(first)) if first != server => {
                            Spot::Before(Item::Server(first))
                        }
                        (false, Some(_)) => return None,
                        _ => Spot::After(Item::Group(group)),
                    }
                }
            };
            Some(Change::MoveServer { server, to })
        }
        (Dragged::Server(server), DropTarget::End) => Some(Change::MoveServer {
            server,
            to: Spot::End,
        }),
        (Dragged::Group(group), DropTarget::Server(target)) => Some(Change::MoveGroup {
            group,
            to: side(Item::Server(target)),
        }),
        (Dragged::Group(group), DropTarget::Group(target)) => {
            (group != target).then(|| Change::MoveGroup {
                group,
                to: side(Item::Group(target)),
            })
        }
        (Dragged::Group(group), DropTarget::End) => Some(Change::MoveGroup {
            group,
            to: Spot::End,
        }),
        (Dragged::Workspace(server, workspace), DropTarget::Workspace(_, target)) => {
            (workspace != target).then(|| Change::MoveWorkspace {
                server,
                workspace,
                to: match drop.zone {
                    Zone::Before => WorkspaceSpot::Before(target),
                    Zone::Into | Zone::After => WorkspaceSpot::After(target),
                },
                // The window fills the list of the server when it makes the change.
                shown: Vec::new(),
            })
        }
        _ => None,
    }
}

/// What the application holds of the server book beside the client core.
#[derive(Debug, Default)]
pub(super) struct BookState {
    checking: bool,
    /// Why the last move of a server was not written, with the server.
    move_failed: Option<(ServerId, String)>,
    /// Why the last other change of the arrangement was not written: a drag, a change of a
    /// group or of a workspace. The server book states it.
    arrange_failed: Option<String>,
    /// The state of the update check that is drawn in place of the one of the client.
    #[cfg(feature = "test-support")]
    fixture: Option<UpdatesKnown>,
    /// The arrangement that is drawn in place of the one of the client.
    #[cfg(feature = "test-support")]
    arrangement_fixture: Option<Arrangement>,
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
        BookError::NotSaved { .. } | BookError::Arrange(layout::ArrangeError::NoServer(_)) => {
            text::book::MOVE_SERVER_GONE.to_owned()
        }
        BookError::Arrange(layout::ArrangeError::NoGroup(_)) => text::book::GROUP_GONE.to_owned(),
        BookError::Arrange(layout::ArrangeError::NoWorkspace(_)) => {
            text::book::WORKSPACE_GONE.to_owned()
        }
        BookError::Arrange(layout::ArrangeError::TooManyGroups) => {
            text::book::too_many_groups(layout::MOST_GROUPS)
        }
        other => text::flow_error(&FlowError::Storage(other.to_string())),
    }
}

/// The top-level parts of the server book as the client holds them now.
fn shelves_of(client: &Client) -> Vec<Shelf> {
    let order: Vec<ServerId> = client.servers().iter().map(|server| server.id()).collect();
    client.arrangement().shelves(&order)
}

/// The workspaces of a server as the server book lists them: in the order of the person.
fn listed_workspaces(client: &Client, server_id: ServerId) -> Vec<WorkspaceId> {
    let mut listed: Vec<WorkspaceId> = client
        .server(server_id)
        .and_then(|server| server.live().view().model)
        .map(|model| {
            model
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace.id)
                .collect()
        })
        .unwrap_or_default();
    client
        .arrangement()
        .sort_workspaces(server_id, &mut listed, |id| *id);
    listed
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

    /// Moves a server one place up or down in the server book: among the servers of its
    /// group, or among the top-level parts over a whole group (ADR 0775). The file is
    /// written on a background thread after the writes that were asked for before, and the
    /// place is counted when the write runs: two steps that are asked for in a row move the
    /// server two places. The client core tells its change when the book is written. A
    /// failure is told in [`AppState::move_failure`].
    pub fn move_server(&mut self, server_id: ServerId, step: Step, cx: &mut Context<Self>) {
        self.book.move_failed = None;
        let client = self.client.clone();
        Self::queue_write(
            cx,
            move || {
                if client.server(server_id).is_none() {
                    return Err(BookError::NotSaved { server_id });
                }
                let shelves = shelves_of(&client);
                match layout::step(&shelves, Item::Server(server_id), step.up()) {
                    Some(to) => client.arrange(Change::MoveServer {
                        server: server_id,
                        to,
                    }),
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

    /// Makes one change of the arrangement of the server book (ADR 0775): a drop of a drag,
    /// or an item of the menu of a server, a group or a workspace. The write runs on the
    /// queue of writes (ADR 0730); a failure is told in the server book.
    pub fn arrange(&mut self, change: Change, cx: &mut Context<Self>) {
        self.arrange_with(move |_| Some(change), cx);
    }

    /// Makes the change that `change` finds when the write runs, from the client as it is
    /// then. Nothing happens without one.
    fn arrange_with(
        &mut self,
        change: impl FnOnce(&Client) -> Option<Change> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.book.arrange_failed = None;
        let client = self.client.clone();
        Self::queue_write(
            cx,
            move || match change(&client) {
                Some(change) => client.arrange(change),
                None => Ok(()),
            },
            move |this, done, cx| {
                if let Err(error) = done {
                    tracing::warn!(%error, "the arrangement of the server book was not saved");
                    this.book.arrange_failed = Some(move_failure(&error));
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Moves a group one place up or down among the top-level parts of the server book.
    pub fn move_group(&mut self, group: GroupId, step: Step, cx: &mut Context<Self>) {
        self.arrange_with(
            move |client| {
                let shelves = shelves_of(client);
                layout::step(&shelves, Item::Group(group), step.up())
                    .map(|to| Change::MoveGroup { group, to })
            },
            cx,
        );
    }

    /// Moves a workspace one place up or down among the workspaces of its server.
    pub fn move_workspace(
        &mut self,
        server_id: ServerId,
        workspace: WorkspaceId,
        step: Step,
        cx: &mut Context<Self>,
    ) {
        self.arrange_with(
            move |client| {
                let shown = listed_workspaces(client, server_id);
                layout::step_workspace(&shown, workspace, step.up()).map(|to| {
                    Change::MoveWorkspace {
                        server: server_id,
                        workspace,
                        to,
                        shown,
                    }
                })
            },
            cx,
        );
    }

    /// Moves a workspace next to another one of its server: the drop of a drag.
    pub fn drop_workspace(
        &mut self,
        server_id: ServerId,
        workspace: WorkspaceId,
        to: layout::WorkspaceSpot,
        cx: &mut Context<Self>,
    ) {
        self.arrange_with(
            move |client| {
                Some(Change::MoveWorkspace {
                    server: server_id,
                    workspace,
                    to,
                    shown: listed_workspaces(client, server_id),
                })
            },
            cx,
        );
    }

    /// Why the last change of the arrangement other than a move of a server in its menu was
    /// not written.
    pub fn arrange_failure(&self) -> Option<&str> {
        self.book.arrange_failed.as_deref()
    }

    /// Puts away what was told about the last change of the arrangement.
    pub fn dismiss_arrange_failure(&mut self, cx: &mut Context<Self>) {
        if self.book.arrange_failed.take().is_some() {
            cx.notify();
        }
    }

    /// Takes the arrangement of the client core into what the view is derived from. It runs
    /// with every change of the client: a change of the arrangement is one.
    pub(super) fn follow_arrangement(&mut self) {
        #[cfg(feature = "test-support")]
        if let Some(fixture) = &self.book.arrangement_fixture {
            self.known.arrangement = fixture.clone();
            return;
        }
        self.known.arrangement = self.client.arrangement();
    }

    /// Draws this arrangement in place of the one of the client. The screen tool uses it:
    /// its servers are not in a server book.
    #[cfg(feature = "test-support")]
    pub fn show_arrangement_fixture(&mut self, arrangement: Arrangement, cx: &mut Context<Self>) {
        self.book.arrangement_fixture = Some(arrangement);
        self.changed(cx);
    }

    /// The arrangement as the window holds it.
    pub fn arrangement(&self) -> &Arrangement {
        &self.known.arrangement
    }

    /// Moves the read position of every place of a server that holds unread messages to its
    /// latest message: of one workspace, or of every workspace and the direct conversations
    /// of the server.
    pub fn mark_server_read(&mut self, server_id: ServerId, workspace: Option<WorkspaceId>) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let places: Vec<(PlaceId, u64)> = model
            .message_states
            .iter()
            .filter(|state| state.latest > state.last_read || state.unread > 0)
            .filter(|state| match (&state.place, workspace) {
                (_, None) => true,
                (MessagePlace::Channel { workspace_id, .. }, Some(only)) => *workspace_id == only,
                (MessagePlace::Direct { .. }, Some(_)) => false,
            })
            .map(|state| (PlaceId::of(&state.place), state.latest))
            .collect();
        for (place, latest) in places {
            self.mark_read(server_id, place, latest);
        }
    }

    /// Marks every server of a group as read.
    pub fn mark_group_read(&mut self, group: GroupId) {
        let members: Vec<ServerId> = self
            .client
            .servers()
            .iter()
            .map(|server| server.id())
            .filter(|server| self.known.arrangement.group_of(*server) == Some(group))
            .collect();
        for server in members {
            self.mark_server_read(server, None);
        }
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
    fn a_drag_has_a_zone_of_a_third_or_of_a_half_of_its_target() {
        assert_eq!(zone(2.0, 30.0, true), Zone::Before);
        assert_eq!(zone(15.0, 30.0, true), Zone::Into);
        assert_eq!(zone(28.0, 30.0, true), Zone::After);
        assert_eq!(zone(14.0, 30.0, false), Zone::Before);
        assert_eq!(zone(16.0, 30.0, false), Zone::After);
        assert_eq!(zone(5.0, 0.0, true), Zone::Into);
    }

    #[test]
    fn a_drop_makes_the_change_of_its_zone_and_target() {
        use junto_client_core::arrangement::{GroupColour, ServerGroup, Spot, WorkspaceSpot};
        let ids: Vec<ServerId> = (0..4).map(|_| ServerId::generate()).collect();
        let group = |collapsed: bool| ServerGroup {
            id: GroupId::from_uuid(uuid::Uuid::from_u128(7)),
            name: "G".to_owned(),
            colour: GroupColour::Ochre,
            collapsed,
        };
        let shelves = |collapsed: bool| {
            vec![
                Shelf::Server(ids[0]),
                Shelf::Group {
                    group: group(collapsed),
                    servers: vec![ids[1], ids[2]],
                },
                Shelf::Server(ids[3]),
            ]
        };
        let g = group(false).id;
        let at = |target, zone| BookDrop { target, zone };
        let open = shelves(false);
        let server = Dragged::Server(ids[3]);
        assert_eq!(
            dropped(server, at(DropTarget::Server(ids[0]), Zone::Into), &open),
            Some(Change::GroupWith {
                server: ids[3],
                target: ids[0]
            })
        );
        assert_eq!(
            dropped(server, at(DropTarget::Server(ids[0]), Zone::Before), &open),
            Some(Change::MoveServer {
                server: ids[3],
                to: Spot::Before(Item::Server(ids[0]))
            })
        );
        assert_eq!(
            dropped(server, at(DropTarget::Server(ids[3]), Zone::Into), &open),
            None
        );
        assert_eq!(
            dropped(server, at(DropTarget::Group(g), Zone::Into), &open),
            Some(Change::MoveServer {
                server: ids[3],
                to: Spot::InGroup(g)
            })
        );
        // Under the head of an expanded group: its first place; of a collapsed one: after it.
        assert_eq!(
            dropped(server, at(DropTarget::Group(g), Zone::After), &open),
            Some(Change::MoveServer {
                server: ids[3],
                to: Spot::Before(Item::Server(ids[1]))
            })
        );
        assert_eq!(
            dropped(
                server,
                at(DropTarget::Group(g), Zone::After),
                &shelves(true)
            ),
            Some(Change::MoveServer {
                server: ids[3],
                to: Spot::After(Item::Group(g))
            })
        );
        assert_eq!(
            dropped(server, at(DropTarget::End, Zone::After), &open),
            Some(Change::MoveServer {
                server: ids[3],
                to: Spot::End
            })
        );
        // A group goes next to a server or a group, never into one.
        let dragged_group = Dragged::Group(g);
        assert_eq!(
            dropped(
                dragged_group,
                at(DropTarget::Server(ids[3]), Zone::Into),
                &open
            ),
            Some(Change::MoveGroup {
                group: g,
                to: Spot::After(Item::Server(ids[3]))
            })
        );
        assert_eq!(
            dropped(dragged_group, at(DropTarget::Group(g), Zone::Before), &open),
            None
        );
        assert!(!takes_into(dragged_group, DropTarget::Server(ids[0])));
        // A workspace goes only among the workspaces of its server.
        let (w0, w1) = (WorkspaceId::generate(), WorkspaceId::generate());
        let workspace = Dragged::Workspace(ids[0], w0);
        assert!(!accepts(workspace, DropTarget::Workspace(ids[1], w1)));
        assert!(!accepts(workspace, DropTarget::Server(ids[0])));
        assert!(!accepts(server, DropTarget::Workspace(ids[0], w1)));
        assert_eq!(
            dropped(
                workspace,
                at(DropTarget::Workspace(ids[0], w1), Zone::After),
                &open
            ),
            Some(Change::MoveWorkspace {
                server: ids[0],
                workspace: w0,
                to: WorkspaceSpot::After(w1),
                shown: Vec::new()
            })
        );
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
