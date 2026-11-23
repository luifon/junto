//! Several servers in one window against the real server (ADR 0107, ADR 0780):
//! a second server that is joined by approval, a notice from a server in the background,
//! the order of the server book, the notice of a newer build, and the version of a server in
//! its menu.

use std::time::{Duration, Instant};

use gpui::TestAppContext;
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
use crate::support::TestServer;

impl Window {
    pub(crate) fn server_names(&mut self) -> Vec<String> {
        self.view()
            .servers
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    /// Joins a further server with an invite code of its administrator. The new server
    /// is shown afterwards.
    #[track_caller]
    pub(crate) fn join_further(&mut self, world: &World, server: &TestServer, admin: &Admin) {
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
}

impl Admin {
    /// Writes to a person in the direct conversation with that person.
    pub(crate) fn write_to(
        &self,
        world: &World,
        account_id: AccountId,
        text: &str,
    ) -> ConversationId {
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
    assert_eq!(window.channel_names().len(), 3);
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
    // The head of the server counts the direct message and the mention of the whole
    // server (ADR 0741).
    let row = &view.servers[1];
    assert_eq!((row.head_unread.messages, row.head_unread.mentions), (1, 0));
    assert_eq!(
        window.drawn_number("server-head-unread-1").as_deref(),
        Some("2")
    );
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
    window.click("menu-arrange");
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
fn a_newer_version_is_told_once_in_the_version_block_whose_button_opens_the_version(
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
    assert!(!window.shows("client-version-action"));
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

    // The version block of the server book is the one notice: the version with its button
    // beside it. The channel column draws no line above the profile strip.
    let told = |window: &mut Window| {
        window.shows("client-version-action") && !window.shows("update-notice")
    };
    assert!(window.view().servers.is_empty());
    assert!(told(&mut window));
    let number = window.cx.debug_bounds("client-version-number").unwrap();
    let button = window.cx.debug_bounds("client-version-action").unwrap();
    assert!(
        button.left() >= number.right() && button.top() < number.bottom(),
        "the button {button:?} is beside the version {number:?}"
    );

    // A server that shows its lists: the block stays the one notice.
    let world = World::new();
    let server = world.server("Harbor");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    assert!(window.shows("channel-list") && told(&mut window));

    // This copy installs nothing: the button opens the settings at the section "Version".
    window.click("client-version-action");
    assert!(window.shows("settings-dialog") && window.shows("version-section"));
    assert!(window.shows("release-page") && window.shows("version-no-install"));
    assert!(window.in_shell(|shell, _, _| shell.is_settings_open()));
    window.press("escape");
    assert!(!window.shows("settings-dialog"));

    // The collapsed server book keeps the button of the notice.
    window.click("collapse-server-book");
    window.until("the server book is collapsed", |window| {
        window.shows("expand-server-book")
    });
    assert!(told(&mut window));
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
    window.stays("the notice stays", Duration::from_millis(400), told);

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
    assert!(told(&mut window));

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
    assert!(told(&mut window));

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
    assert!(!window.shows("client-version-action"));
    assert_eq!(window.view().update, None);

    // A second start with a newer version shows the line after its first check.
    source.set(release(newest, None));
    window.close();
    let mut again = open_with_updates(cx, directory.path(), source.clone(), build);
    again.until("the notice is shown after a start", |window| {
        window.shows("client-version-action")
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
    assert!(!window.shows("client-version-action"));
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
    window.click("menu-arrange");
    // The item after the move items is the choice of the group of the server (ADR 0775).
    window.until("the menu offers the group of the server", |window| {
        window.shows("menu-move-down") && window.shows("menu-server-group")
    });
    let down = window.cx.debug_bounds("menu-move-down").unwrap();
    let next = window.cx.debug_bounds("menu-server-group").unwrap();
    let gap = next.top() - down.bottom();
    assert!(
        gap < gpui::px(16.0),
        "{gap:?} between Move down and the next item"
    );
}
