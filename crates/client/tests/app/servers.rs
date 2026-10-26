//! Several servers in one window against the real server (ADR 0107, ADR 0780, ADR 0784):
//! a second server that is joined by approval, a notice from a server in the background,
//! the order of the server book, the notice of a newer build, the version of a server in its
//! menu, and the page of the recordings of a workspace.

use std::time::{Duration, Instant};

use gpui::TestAppContext;
use junto_client::state::admin::view::recordings::RecordingsRead;
use junto_client::state::admin::view::{Scope, ServerPage};
use junto_client::state::book::CheckShown;
use junto_client::state::view::{ServerRelease, ServerStatus};
use junto_client::text;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{AccountId, ConversationId};
use junto_client_core::protocol::invite::ApproveJoinRequest;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::updates::{FakeSource, Release, UpdateError, Version};
use junto_client_core::version::ServerVersion;

use crate::harness::{Admin, Window, World, member, named, open, open_with_updates};
use crate::messages::document;
use crate::support::{Fault, FaultProxy, TestServer};

impl Window {
    fn server_names(&mut self) -> Vec<String> {
        self.view()
            .servers
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    /// Joins a further server with an invite code of its administrator. The new server
    /// is shown afterwards.
    #[track_caller]
    fn join_further(&mut self, world: &World, server: &TestServer, admin: &Admin) {
        let count = self.view().servers.len();
        let code = admin.invite(world, 1);
        self.find_server(&server.address());
        self.join_with_code(code.expose());
        self.until("the further server is shown", |window| {
            let view = window.view();
            view.servers.len() == count + 1
                && view.servers[count].selected
                && view.servers[count].status == ServerStatus::Ready
                && view.workspace().is_some()
        });
    }

    /// Checks for a time that `holds` stays true while the window is drawn.
    #[track_caller]
    fn stays(&mut self, what: &str, time: Duration, mut holds: impl FnMut(&mut Self) -> bool) {
        let end = Instant::now() + time;
        while Instant::now() < end {
            self.run_tasks();
            assert!(holds(self), "this did not stay: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Opens the settings at their last section, "Version", as "Details" does. The
    /// settings scroll in a window that is lower than they are.
    #[track_caller]
    fn version_settings(&mut self) {
        self.in_shell(|shell, window, cx| shell.open_version_settings(window, cx));
        assert!(self.shows("settings-dialog") && self.shows("version-section"));
    }

    /// Checks the place and the outer form of a line of the channel column that is
    /// directly above the profile strip, in design pixels at 100 %: 30 high, 8 from each
    /// side of the column and 8 above the strip. The column has a border of 1 at its
    /// right side.
    #[track_caller]
    fn above_the_profile_strip(&mut self, line: &'static str) {
        self.run_tasks();
        let bounds = |window: &mut Self, name: &'static str| {
            window
                .cx
                .debug_bounds(name)
                .unwrap_or_else(|| panic!("{name} is not drawn"))
        };
        let (column, strip) = (
            bounds(self, "channel-column"),
            bounds(self, "profile-strip"),
        );
        let line = bounds(self, line);
        let measures = [
            ("height", line.size.height, 30.0),
            ("below", strip.top() - line.bottom(), 8.0),
            ("left", line.left() - column.left(), 8.0),
            ("right", column.right() - line.right(), 9.0),
        ];
        for (name, measure, expected) in measures {
            let measure = f32::from(measure);
            assert!(
                (measure - expected).abs() < 0.01,
                "{name} is {measure} and not {expected}"
            );
        }
    }

    fn open_server_menu(&mut self) {
        if !self.shows("server-menu") {
            self.click("server-menu-button");
        }
        assert!(self.shows("server-menu"));
    }
}

impl Admin {
    /// Writes to a person in the direct conversation with that person.
    fn write_to(&self, world: &World, account_id: AccountId, text: &str) -> ConversationId {
        let api = self.handle.api();
        let conversation = world.run(api.open_conversation(account_id)).unwrap();
        let message = SendMessage {
            document: document(text),
            reply_to: None,
            attachments: Vec::new(),
        };
        world
            .run(api.send_message(PlaceId::Direct(conversation.id), &message))
            .unwrap();
        conversation.id
    }
}

#[gpui::test]
fn a_member_adds_a_second_server_and_enters_it_when_its_administrator_approves(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let first_admin = world.owner(&first);
    let second_admin = world.owner(&second);
    let (_directory, mut window) = member(cx, &world, &first.address(), &first_admin, "Luca");
    assert_eq!(window.server_names(), ["Acme Studio"]);

    // The address of the second server is typed into the address field of the server
    // book. The dialog shows the server that answered and the notice about its operator.
    window.find_server(&second.address());
    assert!(window.shows("join-dialog"));
    assert!(window.shows("operator-notice"));
    assert!(window.shows("code-field") && window.shows("request-access"));
    assert_eq!(
        window.server_names(),
        ["Acme Studio"],
        "nothing is saved before the person chooses a way in"
    );

    // The person has no code and asks for access.
    window.click("request-access");
    window.until("the request waits", |window| {
        window.shows("cancel-request")
            && window.status()
                == Some(ServerStatus::WaitingForApproval {
                    name: "Luca".to_owned(),
                })
    });
    window.click("close-join");
    assert!(!window.shows("join-dialog"));

    // The server book shows the second server as waiting for approval, and the first
    // server keeps its lists.
    let view = window.view();
    assert_eq!(window.server_names(), ["Acme Studio", "Harbor"]);
    assert!(matches!(
        view.servers[1].status,
        ServerStatus::WaitingForApproval { .. }
    ));
    assert!(view.servers[1].workspaces.is_empty());
    assert!(view.servers[1].selected);
    assert!(window.shows("waiting-for-approval"));
    window.until("the first server stays ready", |window| {
        let view = window.view();
        view.servers[0].status == ServerStatus::Ready && view.servers[0].workspaces.len() == 1
    });

    // The administrator of the second server, a client without a window, approves.
    let requests = world
        .run(second_admin.handle.api().join_requests())
        .unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].name, "Luca");
    world
        .run(
            second_admin
                .handle
                .api()
                .approve_join_request(requests[0].id, &ApproveJoinRequest::default()),
        )
        .unwrap();

    // The window enters without an action of the person: no click and no key from here
    // to the lists of the second server.
    window.until_ready();
    let view = window.view();
    assert_eq!(view.selected.as_ref().unwrap().name, "Harbor");
    assert!(view.servers[1].selected);
    assert_eq!(view.servers[1].status, ServerStatus::Ready);
    assert_eq!(view.servers[1].workspaces.len(), 1);
    assert!(view.servers[1].workspaces[0].selected);
    assert!(view.workspace().is_some());
    assert_eq!(window.channel_names().len(), 2);
    assert!(!window.shows("waiting-for-approval"));
    assert!(window.shows("feed") && window.shows("composer"));
    assert_eq!(window.view().person.name, "Luca");
    // The account exists on the second server.
    assert!(
        world
            .run(second_admin.handle.api().join_requests())
            .unwrap()
            .is_empty()
    );
    assert_eq!(window.view().servers[0].status, ServerStatus::Ready);
}

#[gpui::test]
fn a_server_in_the_background_shows_its_unread_signs_and_a_server_that_is_not_kept_connected_shows_none(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let first_admin = world.owner(&first);
    let second_admin = world.owner(&second);
    let (_directory, mut window) = member(cx, &world, &first.address(), &first_admin, "Luca");
    window.join_further(&world, &second, &second_admin);
    // What the second server calls the person and its text channel.
    let luca = window.own_account();
    let general = window.selected_channel();
    window.until("the messages of the second server are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });

    // The first server is shown. The second one stays connected in the background.
    window.click("server-0");
    window.until_ready();
    let view = window.view();
    assert!(view.servers[0].selected && !view.servers[1].selected);
    assert_eq!(view.selected.as_ref().unwrap().name, "Acme Studio");
    assert!(!view.servers[1].unread.any() && !view.servers[1].head_unread.any());
    assert!(!window.shows("server-head-unread-1"));

    // The administrator of the second server writes to the person directly, and
    // mentions the person in a channel there.
    let conversation = second_admin.write_to(&world, luca, "Hello from Harbor");
    second_admin.mention(&world, general, "Ping, ", luca);
    window.until("the second server shows its unread signs", |window| {
        let row = &window.view().servers[1];
        row.head_unread.any()
            && row
                .workspaces
                .first()
                .is_some_and(|workspace| workspace.unread.mentions == 1)
    });
    let view = window.view();
    // The head of the server shows the direct message, which no workspace row shows.
    let row = &view.servers[1];
    assert_eq!((row.head_unread.messages, row.head_unread.mentions), (1, 0));
    assert!(window.shows("server-head-unread-1"));
    // The workspace shows the mention.
    assert_eq!(row.workspaces[0].unread.messages, 1);
    assert_eq!(row.workspaces[0].unread.mentions, 1);
    assert_eq!((row.unread.messages, row.unread.mentions), (2, 1));
    // The first server stays shown, and has no sign.
    assert!(view.servers[0].selected && !view.servers[1].selected);
    assert_eq!(view.selected.as_ref().unwrap().name, "Acme Studio");
    assert!(!view.servers[0].unread.any() && !view.servers[0].head_unread.any());
    assert!(!window.shows("server-head-unread-0"));

    // Selecting the second server shows the content.
    window.click("server-1");
    window.until_ready();
    assert_eq!(window.view().selected.unwrap().name, "Harbor");
    window.until_message("Ping, @Luca");
    let directs = window.view().lists().unwrap().directs.clone();
    assert_eq!(directs.len(), 1);
    assert_eq!(directs[0].id, conversation);
    assert!(directs[0].unread.any());
    window.click_named(format!("direct-row:{conversation}"));
    window.until_message("Hello from Harbor");
    window.until("everything of the second server is read", |window| {
        if window.shows("mark-as-read") {
            window.click("mark-as-read");
        }
        let view = window.view();
        if view
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
            && !view.servers[1].head_unread.any()
            && view.servers[1].unread.any()
        {
            // The direct conversation is read: the channel is shown, and read too.
            window.click_channel("general");
        }
        !window.view().servers[1].unread.any()
    });
    assert!(!window.shows("server-head-unread-1"));

    // "Keep connected" is turned off for the second server.
    window.open_server_menu();
    window.click("keep-connected");
    window.until(
        "the second server connects only when it is selected",
        |window| {
            window
                .view()
                .selected
                .is_some_and(|server| server.name == "Harbor" && !server.keep_connected)
        },
    );
    window.press("escape");
    assert!(!window.shows("server-menu"));

    // Another server is shown: the second one has no connection now.
    window.click("server-0");
    window.until_ready();
    window.until("the second server has no connection", |window| {
        window.view().servers[1].status == ServerStatus::Standby
    });
    let quiet = window.view().servers[1].clone();
    assert!(!quiet.unread.any() && !quiet.head_unread.any());

    // A further direct message and a further mention arrive there. No sign changes
    // while the server is not selected (ADR 0107).
    second_admin.write_to(&world, luca, "Are you there?");
    second_admin.mention(&world, general, "Second ping, ", luca);
    window.stays(
        "the server book shows no sign of a server without a connection",
        Duration::from_secs(3),
        |window| {
            let view = window.view();
            view.servers[1] == quiet
                && view.servers[0].selected
                && !window.shows("server-head-unread-1")
        },
    );

    // The server is selected: it connects, and shows what arrived meanwhile.
    window.click("server-1");
    window.until_ready();
    window.until("the second server shows what arrived", |window| {
        let view = window.view();
        view.lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|direct| direct.id == conversation && direct.unread.any())
        })
    });
    window.click_named(format!("direct-row:{conversation}"));
    window.until_message("Are you there?");
    window.click_channel("general");
    window.until_message("Second ping, @Luca");
}

#[gpui::test]
fn a_server_is_moved_in_the_server_book_and_a_second_start_shows_the_same_order(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let first_admin = world.owner(&first);
    let second_admin = world.owner(&second);
    let (directory, mut window) = member(cx, &world, &first.address(), &first_admin, "Luca");
    window.join_further(&world, &second, &second_admin);
    assert_eq!(window.server_names(), ["Acme Studio", "Harbor"]);

    // The menu of the selected server has the two items. The second server is the last.
    window.open_server_menu();
    assert!(window.shows("menu-move-up") && window.shows("menu-move-down"));
    // "Move down" does nothing at the end.
    window.click("menu-move-down");
    window.stays(
        "the last server stays the last",
        Duration::from_millis(400),
        |window| window.server_names() == ["Acme Studio", "Harbor"],
    );

    // The second server moves up. The menu stays open, and the selection stays.
    window.click("menu-move-up");
    window.until("the second server is the first", |window| {
        window.server_names() == ["Harbor", "Acme Studio"]
    });
    assert!(window.shows("server-menu"));
    let view = window.view();
    assert!(view.servers[0].selected && !view.servers[1].selected);
    assert_eq!(view.selected.as_ref().unwrap().name, "Harbor");
    assert!(view.workspace().is_some());
    // The server book draws the new order.
    assert!(window.shows("server-0") && window.shows("server-1"));

    // The keyboard: the focus is on "Move up", which does nothing now. The next item is
    // "Move down": Enter moves the server down, and the focus stays on the item, so
    // that Enter does nothing more and the item before it is "Move up" again.
    window.press("enter");
    window.stays(
        "the first server stays the first",
        Duration::from_millis(400),
        |window| window.server_names() == ["Harbor", "Acme Studio"],
    );
    window.press("down enter");
    window.until("the keyboard moved the server down", |window| {
        window.server_names() == ["Acme Studio", "Harbor"]
    });
    assert!(window.shows("server-menu"));
    assert!(window.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)));
    window.press("enter");
    window.stays(
        "the last server stays the last",
        Duration::from_millis(400),
        |window| window.server_names() == ["Acme Studio", "Harbor"],
    );
    window.press("up enter");
    window.until("the keyboard moved the server up", |window| {
        window.server_names() == ["Harbor", "Acme Studio"]
    });
    assert!(window.shows("server-menu"));
    window.press("escape");
    assert!(!window.shows("server-menu"));

    // The order is in the server book on the disk: a second start shows it.
    window.close();
    let mut again = open(cx, directory.path());
    assert_eq!(again.server_names(), ["Harbor", "Acme Studio"]);
    again.until_ready();
    assert_eq!(again.server_names(), ["Harbor", "Acme Studio"]);
}

#[gpui::test]
fn a_newer_version_is_told_above_the_profile_strip_and_the_line_opens_the_version(
    cx: &mut TestAppContext,
) {
    const PAGE: &str = "https://junto.example.test/releases/0.4.0";
    let build = Version::new(0, 3, 1);
    let release = |version: Version, page: Option<&str>| Ok(Release::new(version, page));
    // The source offers the build that runs.
    let source = FakeSource::new(release(build, None));
    let directory = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, directory.path(), source.clone(), build);
    window.choose_name("Luca");
    let check = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.version_view().check)
    };
    // The application checks at its start.
    window.until("the first check has ended", |window| {
        check(window) == CheckShown::Current
    });
    assert_eq!(source.checks(), 1);
    assert!(!window.shows("update-notice"));
    assert_eq!(window.view().update, None);

    // The settings state the version of this build and that it is the newest.
    window.version_settings();
    assert!(window.shows("settings-dialog") && window.shows("version-section"));
    assert!(window.shows("version-build") && window.shows("version-state"));
    assert!(window.shows("version-no-install") && window.shows("check-update"));
    assert!(!window.shows("release-page") && !window.shows("copy-release-page"));

    // An older version shows nothing.
    source.set(release(Version::new(0, 2, 9), Some(PAGE)));
    window.version_settings();
    window.click("check-update");
    window.until("the second check has ended", |_| source.checks() == 2);
    window.stays(
        "an older version is not told",
        Duration::from_millis(400),
        |window| {
            check(window) == CheckShown::Current
                && window.view().update.is_none()
                && !window.shows("release-page")
        },
    );
    // A source that fails keeps the earlier answer.
    source.set(Err(UpdateError::Unreachable));
    window.until("the button is free again", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| !state.version_view().checking)
    });
    window.version_settings();
    window.click("check-update");
    window.until("the third check has ended", |_| source.checks() == 3);
    window.stays(
        "a failed check changes nothing",
        Duration::from_millis(400),
        |window| check(window) == CheckShown::Current && window.view().update.is_none(),
    );

    // The source reports a newer version.
    let newer = Version::new(0, 4, 0);
    source.set(release(newer, Some(PAGE)));
    window.until("the button is free again", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| !state.version_view().checking)
    });
    window.version_settings();
    window.click("check-update");
    window.until("the newer version is known", |window| {
        check(window)
            == CheckShown::Available {
                version: newer,
                page: Some(PAGE.to_owned()),
            }
    });
    // The settings show the address of the page as text, and copy it on request.
    window.until("the settings show the address", |window| {
        window.shows("release-page") && window.shows("copy-release-page")
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.release_page_shown(cx)),
        PAGE
    );
    window.version_settings();
    window.click("copy-release-page");
    assert!(window.in_shell(|shell, _, _| shell.release_page_was_copied()));
    assert_eq!(
        window.cx.read_from_clipboard().and_then(|item| item.text()),
        Some(PAGE.to_owned())
    );
    window.click("dialog-close");
    assert!(!window.shows("settings-dialog"));
    assert_eq!(
        window.view().update.map(|update| update.version),
        Some(newer)
    );
    assert_eq!(
        text::book::update_available(newer),
        "Junto 0.4.0 is available."
    );

    // The server book is empty: the channel column holds the profile strip alone, and
    // the line is above it.
    assert!(window.view().servers.is_empty());
    assert!(window.shows("update-notice"));
    window.above_the_profile_strip("update-notice");

    // A server that shows its lists: the line is in its channel column, and the server
    // book holds nothing of the notice. No control puts the line away.
    let world = World::new();
    let server = world.server("Harbor");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    assert!(window.shows("channel-list") && window.shows("update-notice"));
    window.above_the_profile_strip("update-notice");
    for gone in ["update-dismiss", "update-details", "update-notice-icon"] {
        assert!(!window.shows(gone), "{gone} is drawn");
    }

    // The whole line is one control. A click opens the settings, which hold the section
    // "Version".
    window.click("update-notice");
    assert!(window.shows("settings-dialog") && window.shows("version-section"));
    assert!(window.shows("release-page") && window.shows("version-no-install"));
    assert!(window.in_shell(|shell, _, _| shell.is_settings_open()));
    window.press("escape");
    assert!(!window.shows("settings-dialog"));

    // The line is a stop of the keyboard, and Enter and Space activate it.
    let focused = |window: &mut Window| {
        window.in_shell(|shell, window, _| shell.update_notice_is_focused(window))
    };
    assert!(focused(&mut window), "the focus returns to the line");
    window.press("tab");
    assert!(!focused(&mut window));
    window.press("shift-tab");
    assert!(focused(&mut window));
    for key in ["enter", "space"] {
        window.press(key);
        assert!(
            window.shows("settings-dialog") && window.shows("version-section"),
            "{key} does not open the section"
        );
        window.press("escape");
        assert!(!window.shows("settings-dialog"));
        assert!(focused(&mut window));
    }

    // The collapsed server book changes nothing of the line.
    window.click("collapse-server-book");
    window.until("the server book is collapsed", |window| {
        window.shows("expand-server-book")
    });
    assert!(window.shows("update-notice") && !window.shows("update-notice-icon"));
    window.above_the_profile_strip("update-notice");
    window.click("expand-server-book");
    window.until("the server book is expanded", |window| {
        window.shows("collapse-server-book")
    });

    // The line stays while the check says "available": a further check with the same
    // answer changes nothing.
    let checks = source.checks();
    window.version_settings();
    window.click("check-update");
    window.until("a further check has ended", |_| {
        source.checks() == checks + 1
    });
    window.press("escape");
    window.stays("the line stays", Duration::from_millis(400), |window| {
        window.shows("update-notice")
    });

    // A second server whose administrator has not answered shows a state panel. The
    // channel column is drawn with its profile strip, and the line with it.
    let second = world.server("Quarry");
    window.find_server(&second.address());
    window.click("request-access");
    window.until("the request waits", |window| {
        window.shows("cancel-request")
            && window.status()
                == Some(ServerStatus::WaitingForApproval {
                    name: "Luca".to_owned(),
                })
    });
    window.click("close-join");
    assert!(window.shows("waiting-for-approval") && !window.shows("channel-list"));
    assert!(window.shows("update-notice"));
    window.above_the_profile_strip("update-notice");

    // A release without a page shows no address, and the line stays.
    let newest = Version::new(0, 5, 0);
    source.set(release(newest, None));
    window.version_settings();
    window.until("the button is free again", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| !state.version_view().checking)
    });
    window.version_settings();
    window.click("check-update");
    window.until("the newest version is known", |window| {
        check(window)
            == CheckShown::Available {
                version: newest,
                page: None,
            }
    });
    window.until("the address is gone", |window| {
        !window.shows("release-page")
    });
    assert!(!window.shows("copy-release-page"));
    window.press("escape");
    assert!(window.shows("update-notice"));

    // The source offers the build that runs again: the line is gone.
    source.set(release(build, None));
    window.version_settings();
    window.until("the button is free again", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| !state.version_view().checking)
    });
    window.version_settings();
    window.click("check-update");
    window.until("the build is the newest again", |window| {
        check(window) == CheckShown::Current
    });
    window.press("escape");
    assert!(!window.shows("update-notice"));
    assert_eq!(window.view().update, None);

    // A second start with a newer version shows the line after its first check.
    source.set(release(newest, None));
    window.close();
    let mut again = open_with_updates(cx, directory.path(), source.clone(), build);
    again.until("the line is shown after a start", |window| {
        window.shows("update-notice")
    });
}

/// The texts that the window paints in one frame.
fn painted_texts(window: &mut Window) -> Vec<String> {
    let _record = junto_client::ui::record_painted_text();
    window.cx.update(|window, _| window.refresh());
    window.settle();
    junto_client::ui::take_painted_text()
        .into_iter()
        .map(|painted| painted.text.to_string())
        .collect()
}

#[gpui::test]
fn the_server_menu_states_the_version_of_the_server_and_a_newer_release_of_the_source(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    // The server of the tests is built from this checkout, with its version.
    let stated = ServerVersion::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let release = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let version_line = text::book::server_version(&stated);
    let shown = |window: &mut Window| window.view().selected.and_then(|selected| selected.release);

    // Without a source of releases the menu states the version only.
    let (_plain_directory, mut plain) = member(cx, &world, &server.address(), &admin, "Ana");
    plain.until("the server states its version", |window| {
        shown(window).is_some()
    });
    assert_eq!(
        shown(&mut plain),
        Some(ServerRelease {
            version: stated.clone(),
            newer: None,
            newer_events: false,
        })
    );
    plain.open_server_menu();
    assert!(plain.shows("server-version"));
    assert!(!plain.shows("server-release-available"));
    let painted = painted_texts(&mut plain);
    assert!(painted.contains(&version_line), "{painted:?}");
    plain.close();

    // The source names the release of the server: no newer release is told.
    let source = FakeSource::new(Ok(Release::new(release, None)));
    let directory = tempfile::tempdir().unwrap();
    let mut window = open_with_updates(cx, directory.path(), source.clone(), release);
    window.choose_name("Luca");
    let code = admin.invite(&world, 1);
    window.find_server(&server.address());
    window.join_with_code(code.expose());
    window.until_ready();
    window.until("the first check has ended", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.version_view().check)
            == CheckShown::Current
    });
    window.until("the server states its version", |window| {
        shown(window).is_some()
    });
    assert_eq!(
        shown(&mut window),
        Some(ServerRelease {
            version: stated.clone(),
            newer: None,
            newer_events: false,
        })
    );
    window.open_server_menu();
    assert!(window.shows("server-version"));
    assert!(!window.shows("server-release-available"));
    window.press("escape");

    // The source names a newer release: the menu tells it under the version.
    let newer = Version::new(release.major(), release.minor(), release.patch() + 1);
    source.set(Ok(Release::new(newer, None)));
    window
        .state
        .update(&mut window.cx, |state, cx| state.check_for_update(cx));
    window.until("the newer release is known", |window| {
        shown(window).and_then(|release| release.newer) == Some(newer)
    });
    window.open_server_menu();
    assert!(window.shows("server-version"));
    assert!(window.shows("server-release-available"));
    let painted = painted_texts(&mut window);
    assert!(painted.contains(&version_line), "{painted:?}");
    let available = text::book::server_release_available(newer);
    assert!(painted.contains(&available), "{painted:?}");
}

#[gpui::test]
fn the_application_without_a_source_of_releases_shows_no_notice_and_asks_nothing(
    cx: &mut TestAppContext,
) {
    let (_directory, mut window) = named(cx, "Luca");
    assert!(!window.shows("update-notice"));
    assert_eq!(window.view().update, None);
    window.version_settings();
    assert!(window.shows("version-section") && window.shows("version-build"));
    assert!(window.shows("version-state") && window.shows("version-no-install"));
    // Without a source there is nothing to ask.
    assert!(!window.shows("check-update") && !window.shows("release-page"));
    let version = window
        .state
        .read_with(&window.cx, |state, _| state.version_view());
    assert_eq!(version.check, CheckShown::NotConfigured);
    assert_eq!(version.build, Version::THIS_BUILD);
    assert!(!version.can_check && !version.checking);
}

/// The page of the recordings that the open administration shows.
fn recordings_page(
    window: &mut Window,
) -> Option<junto_client::state::admin::view::recordings::RecordingsPage> {
    window.run_tasks();
    let view = window
        .state
        .read_with(&window.cx, |state, _| state.admin_view())?;
    match view.scope {
        Scope::Server {
            page: ServerPage::Recordings(page),
            ..
        } => Some(page),
        _ => None,
    }
}

#[gpui::test]
fn a_member_reads_the_recordings_of_a_workspace_that_has_none_and_a_read_that_failed_again(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let workspace_id = window.view().workspace().unwrap().id;
    // One read asks for the 50 newest recordings.
    let path = format!("/api/v1/workspaces/{workspace_id}/recordings?limit=50");

    // A member has the page in the group of the shown workspace.
    window.open_server_menu();
    window.click("menu-administration");
    window.until("the administration is open", |window| {
        window.shows("admin") && window.shows("admin-nav-recordings")
    });
    // The first read gets no answer: the page states the failure with "Read again".
    proxy.fail(&path, [Fault::Cut; 8]);
    window.click("admin-nav-recordings");
    window.until("the read failed", |window| {
        window.shows("recordings-failed")
    });
    let page = recordings_page(&mut window).unwrap();
    assert!(matches!(
        page.state,
        RecordingsRead::Failed { older: false, .. }
    ));
    assert!(page.rows.is_empty());
    assert!(window.shows("recordings-retry"));
    assert!(!window.shows("recordings-empty") && !window.shows("recordings-older"));
    assert!(!proxy.seen(&path).is_empty());

    // The server answers again: the list is empty, and the page says so.
    window.until("the list was read", |window| {
        if window.shows("recordings-retry") {
            window.click("recordings-retry");
        }
        window.shows("recordings-empty")
    });
    let page = recordings_page(&mut window).unwrap();
    assert_eq!(page.state, RecordingsRead::Loaded);
    assert_eq!(page.workspace_id, workspace_id);
    assert!(page.rows.is_empty() && !page.older && !page.at_bound);
    assert!(!window.shows("recordings-failed") && !window.shows("recordings-older"));
    assert!(!window.shows("recordings-bound"));
    assert!(window.shows("recordings-reload") && window.shows("recordings-count"));
    // Every request asked for the first page, and for nothing else of the recordings.
    let all = proxy.all();
    assert!(
        all.iter()
            .filter(|seen| seen.path.contains("/recordings"))
            .all(|seen| seen.path == path && seen.method == "GET")
    );

    // "Read again" reads the list again.
    let reads = proxy.seen(&path).len();
    window.click("recordings-reload");
    window.until("the list was read again", |window| {
        proxy.seen(&path).len() > reads && window.shows("recordings-empty")
    });

    // The page offers no player and no control that saves or removes a recording.
    for absent in ["recording-play", "recording-save", "recording-remove"] {
        assert!(!window.shows(absent), "{absent}");
    }
    window.click("admin-close");
    assert!(!window.shows("admin"));
}

/// Finding 11 of the first live test: the menu of a server kept an empty space for a
/// refusal of a move between "Move down" and the next item. The refusal now follows the
/// last item.
#[gpui::test]
fn the_server_menu_has_no_empty_space_after_its_move_items(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Ana");
    window.open_server_menu();
    window.until("the menu offers the administration", |window| {
        window.shows("menu-move-down") && window.shows("menu-administration")
    });
    let down = window.cx.debug_bounds("menu-move-down").unwrap();
    let next = window.cx.debug_bounds("menu-administration").unwrap();
    let gap = next.top() - down.bottom();
    assert!(
        gap < gpui::px(16.0),
        "{gap:?} between Move down and the next item"
    );
}
