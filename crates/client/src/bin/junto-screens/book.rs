//! The screens of the server book with several servers and of the update notice
//! (ADR 0780).

use std::sync::Arc;

use junto_client::shell::add_server::AddTab;
use junto_client::shell::{Open, Shell};
use junto_client::state::book::UpdatesKnown;
use junto_client::state::view::ServerSnapshot;
use junto_client::text;
use junto_client_core::Reach;
use junto_client_core::arrangement::{Arrangement, Change, GroupColour, GroupId, WorkspaceSpot};
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::StreamStatus;
use junto_client_core::protocol::message::{Conversation, MessagePlace, MessageState};
use junto_client_core::updates::{Release, UpdateState, Version};
use junto_client_core::version::Incompatibility;
use junto_media::calls::Phase;

use super::{Screen, Studio, acme, harbor, harbor_as, id, in_call, person, reachable};

// The numbers of the identifiers of the servers of the screens.
const ACME: u32 = 1;
const HARBOR: u32 = 2;
const QUARRY: u32 = 3;
const STUDIO: u32 = 10;

const BUILD: Version = Version::new(0, 3, 1);
const NEWER: Version = Version::new(0, 4, 0);
const PAGE: &str = "https://junto.example.test/releases/0.4.0";

/// "Harbor" with a direct conversation that holds unread messages, and this number of
/// mentions in it. No row of a workspace shows a direct conversation.
fn harbor_with_direct(mentions: u64) -> ServerSnapshot {
    let mut server = harbor();
    if let Some(model) = &mut server.model {
        let model = Arc::make_mut(model);
        let (luca, ana) = (person("Luca"), person("Ana"));
        let conversation = Conversation {
            id: id(900),
            low_account_id: luca.min(ana),
            high_account_id: luca.max(ana),
        };
        model.message_states = vec![MessageState {
            read_revision: 0,
            place: MessagePlace::Direct {
                conversation_id: conversation.id,
                low_account_id: conversation.low_account_id,
                high_account_id: conversation.high_account_id,
            },
            last_read: 1,
            latest: 4,
            unread: 3,
            mentions,
            unread_sign: true,
            notification_mentions: mentions,
            muted: false,
        }];
        model.conversations = vec![conversation];
    }
    server
}

/// A third server, on which the request of the person waits for an administrator.
fn quarry() -> ServerSnapshot {
    ServerSnapshot {
        id: id(QUARRY),
        name: "Quarry".to_owned(),
        address: "quarry.example:7443".to_owned(),
        ..harbor_as(
            AccessState::WaitingForApproval {
                join_request_id: Some(id(701)),
                name: "Luca".to_owned(),
            },
            reachable(),
            None,
        )
    }
}

fn three_servers(mentions: u64) -> Vec<ServerSnapshot> {
    vec![acme(), harbor_with_direct(mentions), quarry()]
}

fn select(shell: &mut Shell, server: u32, cx: &mut gpui::Context<Shell>) {
    shell
        .state()
        .clone()
        .update(cx, |state, cx| state.select_server(id(server), cx));
}

fn available(page: Option<&str>) -> UpdatesKnown {
    UpdatesKnown {
        build: BUILD,
        has_source: true,
        state: UpdateState::Available(Release::new(NEWER, page)),
        newest: Some(NEWER),
        checking: false,
    }
}

fn checked(state: UpdateState) -> UpdatesKnown {
    UpdatesKnown {
        build: BUILD,
        has_source: true,
        newest: (state == UpdateState::Current).then_some(BUILD),
        state,
        checking: false,
    }
}

fn show_updates(shell: &mut Shell, updates: UpdatesKnown, cx: &mut gpui::Context<Shell>) {
    shell
        .state()
        .clone()
        .update(cx, |state, cx| state.show_updates_fixture(updates, cx));
}

/// Acme Studio and Harbor in the group "Work", in cypress, before Quarry; the workspace
/// "Client Beta" of Acme Studio first. `collapsed`: the group is collapsed.
fn arranged(collapsed: bool, workspaces_collapsed: bool) -> (Arrangement, GroupId) {
    let mut order = vec![id(ACME), id(HARBOR), id(QUARRY)];
    let mut arrangement = Arrangement::default();
    let mut change = |change: Change| {
        arrangement
            .apply(&mut order, change)
            .unwrap_or_else(|error| panic!("{error}"));
    };
    change(Change::NewGroup {
        server: id(ACME),
        name: "Work".to_owned(),
    });
    change(Change::MoveWorkspace {
        server: id(ACME),
        workspace: id(12),
        to: WorkspaceSpot::Before(id(STUDIO)),
        shown: vec![id(STUDIO), id(11), id(12)],
    });
    change(Change::CollapseWorkspaces {
        server: id(ACME),
        collapsed: workspaces_collapsed,
    });
    let group = arrangement
        .group_of(id(ACME))
        .unwrap_or_else(|| panic!("a group"));
    let mut change = |change: Change| {
        arrangement
            .apply(&mut order, change)
            .unwrap_or_else(|error| panic!("{error}"));
    };
    change(Change::MoveServer {
        server: id(HARBOR),
        to: junto_client_core::arrangement::Spot::InGroup(group),
    });
    change(Change::ColourGroup {
        group,
        colour: GroupColour::Cypress,
    });
    change(Change::CollapseGroup { group, collapsed });
    (arrangement, group)
}

fn arrange(shell: &mut Shell, arrangement: Arrangement, cx: &mut gpui::Context<Shell>) {
    shell.state().clone().update(cx, |state, cx| {
        state.show_arrangement_fixture(arrangement, cx)
    });
}

fn draw_arrangement(studio: &mut Studio) -> Result<(), String> {
    // A group of two servers, expanded, with a mention in Harbor; the workspaces of Acme
    // Studio in the order of the person.
    studio.draw(
        Screen::new("book-group-expanded", three_servers(2)),
        |shell, _, cx| arrange(shell, arranged(false, false).0, cx),
    )?;
    // Collapsed: the head counts its servers and shows the mention of Harbor.
    studio.draw(
        Screen::new("book-group-collapsed", three_servers(2)),
        |shell, _, cx| arrange(shell, arranged(true, false).0, cx),
    )?;
    // The workspaces of Acme Studio collapsed: its head shows what they hold.
    studio.draw(
        Screen::new("book-workspaces-collapsed", three_servers(0)),
        |shell, _, cx| arrange(shell, arranged(false, true).0, cx),
    )?;
    // The collapsed server book: the tile of the collapsed group with the pictures of its
    // servers, and the expanded group on the wash of its colour.
    for (name, collapsed) in [
        ("book-group-collapsed-rail", true),
        ("book-group-expanded-rail", false),
    ] {
        studio.draw(Screen::new(name, three_servers(2)), move |shell, _, cx| {
            arrange(shell, arranged(collapsed, false).0, cx);
            shell
                .state()
                .clone()
                .update(cx, |state, cx| state.set_server_book_collapsed(true, cx));
        })?;
    }
    // The menu of the group, the choice of the group of a server, and the dialog of the
    // name.
    studio.draw(
        Screen::new("book-group-menu", three_servers(2)),
        |shell, window, cx| {
            let (arrangement, group) = arranged(false, false);
            arrange(shell, arrangement, cx);
            shell.open_group_menu(group, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("book-group-choice", three_servers(0)),
        |shell, window, cx| {
            arrange(shell, arranged(false, false).0, cx);
            shell.open_group_choice(id(ACME), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("book-group-rename", three_servers(0)),
        |shell, window, cx| {
            let (arrangement, group) = arranged(false, false);
            arrange(shell, arrangement, cx);
            shell.open_rename_dialog(group, window, cx);
        },
    )?;
    // The menu of a workspace, and the menu of a server in a group.
    studio.draw(
        Screen::new("book-workspace-menu", three_servers(0)),
        |shell, window, cx| {
            arrange(shell, arranged(false, false).0, cx);
            shell.open_workspace_menu(id(ACME), id(12), window, cx);
        },
    )?;
    // The menu of the shown workspace under the name of the channel column, without the items
    // that arrange it (ADR 0780).
    studio.draw(
        Screen::new("workspace-menu-header", three_servers(0)),
        |shell, window, cx| shell.open(Open::HeaderMenu, window, cx),
    )?;
    studio.draw(
        Screen::new("book-menu-in-group", three_servers(0)),
        |shell, window, cx| {
            arrange(shell, arranged(false, false).0, cx);
            shell.open(Open::ServerMenu, window, cx);
        },
    )?;
    Ok(())
}

fn draw_book(studio: &mut Studio) -> Result<(), String> {
    // Three servers: the shown one, one with unread direct messages, and one that waits
    // for an administrator.
    studio.draw(
        Screen::new("book-three-servers", three_servers(0)),
        |_, _, _| {},
    )?;
    // A mention in a direct conversation outranks plain unread on the head.
    studio.draw(
        Screen::new("book-three-servers-mention", three_servers(2)),
        |_, _, _| {},
    )?;
    studio.draw(
        Screen::new("book-three-servers-collapsed", three_servers(0)),
        |shell, _, cx| {
            shell
                .state()
                .clone()
                .update(cx, |state, cx| state.set_server_book_collapsed(true, cx));
        },
    )?;
    studio.draw(
        Screen::new("book-three-servers-waiting-shown", three_servers(0)),
        |shell, _, cx| select(shell, QUARRY, cx),
    )?;

    // The submenu "Arrange" of the menu of a server, with the items that move it.
    studio.draw(
        Screen::new("book-menu-move-first", three_servers(0)),
        |shell, window, cx| {
            shell.open(Open::ServerMenu, window, cx);
            shell.show_arrange_items(cx);
        },
    )?;
    studio.draw(
        Screen::new("book-menu-move-middle", three_servers(0)),
        |shell, window, cx| {
            select(shell, HARBOR, cx);
            shell.open(Open::ServerMenu, window, cx);
            shell.show_arrange_items(cx);
        },
    )?;
    studio.draw(
        Screen::new("book-menu-move-last", three_servers(0)),
        |shell, window, cx| {
            select(shell, QUARRY, cx);
            shell.open(Open::ServerMenu, window, cx);
            shell.show_arrange_items(cx);
        },
    )?;
    studio.draw(
        Screen::new("book-menu-move-failed", three_servers(0)),
        |shell, window, cx| {
            select(shell, HARBOR, cx);
            shell.open(Open::ServerMenu, window, cx);
            shell.show_arrange_items(cx);
            // The opening of the menu puts a failure of before away.
            shell.state().clone().update(cx, |state, cx| {
                state.show_move_failure_fixture(id(HARBOR), text::book::MOVE_SERVER_GONE, cx);
            });
        },
    )?;
    // The version of the server under its name, and a newer release of the source.
    studio.draw(
        Screen::new("server-menu-newer-release", three_servers(0)),
        |shell, window, cx| {
            show_updates(shell, available(Some(PAGE)), cx);
            shell.open(Open::ServerMenu, window, cx);
        },
    )?;
    // The window "Add server or workspace" at each of its tabs.
    studio.draw(
        Screen::new("add-window-server", three_servers(0)),
        |shell, window, cx| {
            shell.open_add_window(AddTab::Server, None, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("add-window-workspace", three_servers(0)),
        |shell, window, cx| {
            shell.open_add_window(AddTab::Workspace, None, window, cx);
        },
    )?;
    Ok(())
}

fn draw_update(studio: &mut Studio) -> Result<(), String> {
    // The version block of the server book is the one notice of a newer version; the
    // channel column draws no line above the profile strip (ADR 0780).
    studio.draw(
        Screen::new("update-notice", three_servers(0)),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    // At 150 % the window is narrow: the server book is collapsed, and keeps the button.
    studio.draw(
        Screen::new("update-notice-scale-150", three_servers(0)).scale(150),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    studio.draw(
        Screen::call("update-notice-in-call", in_call(Phase::Connected)),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    studio.draw(
        Screen::new("update-notice-no-server", Vec::new()),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;

    // The section "Version" of the settings in each state of the check.
    let states: [(&'static str, UpdatesKnown); 7] = [
        (
            "settings-version-not-configured",
            UpdatesKnown {
                build: BUILD,
                ..UpdatesKnown::default()
            },
        ),
        (
            "settings-version-not-checked",
            checked(UpdateState::NotChecked),
        ),
        ("settings-version-current", checked(UpdateState::Current)),
        ("settings-version-failed", checked(UpdateState::Failed)),
        ("settings-version-available", available(Some(PAGE))),
        ("settings-version-available-without-page", available(None)),
        (
            "settings-version-checking",
            UpdatesKnown {
                checking: true,
                ..checked(UpdateState::Current)
            },
        ),
    ];
    for (name, updates) in states {
        studio.draw(
            Screen::new(name, three_servers(0)),
            move |shell, window, cx| {
                show_updates(shell, updates, cx);
                shell.open_version_settings(window, cx);
            },
        )?;
    }

    // The version of this copy at the end of the server book in each state and each way of its
    // update, expanded and collapsed (ADR 0780).
    {
        use junto_client::state::update_install::UpdateWay;
        const DOWNLOAD_PAGE: &str = "https://releases.example.test/junto/latest";
        let ways: [(&'static str, UpdatesKnown, UpdateWay, bool); 9] = [
            (
                "book-version-current",
                checked(UpdateState::Current),
                UpdateWay::InApp,
                false,
            ),
            (
                "book-version-no-source",
                UpdatesKnown {
                    build: BUILD,
                    ..UpdatesKnown::default()
                },
                UpdateWay::NoSource,
                false,
            ),
            (
                "book-version-failed",
                checked(UpdateState::Failed),
                UpdateWay::InApp,
                false,
            ),
            (
                "book-version-available",
                available(Some(PAGE)),
                UpdateWay::InApp,
                false,
            ),
            (
                "book-version-installed",
                available(Some(PAGE)),
                UpdateWay::InApp,
                true,
            ),
            (
                "book-version-deb",
                available(Some(PAGE)),
                UpdateWay::SystemPackage,
                false,
            ),
            (
                "book-version-move-first",
                available(Some(PAGE)),
                UpdateWay::MoveFirst,
                false,
            ),
            (
                "book-version-not-a-package",
                available(Some(PAGE)),
                UpdateWay::NotAPackage,
                false,
            ),
            (
                "book-version-available-collapsed",
                available(Some(PAGE)),
                UpdateWay::InApp,
                false,
            ),
        ];
        for (name, updates, way, installed) in ways {
            let collapsed = name.ends_with("-collapsed");
            studio.draw(Screen::new(name, three_servers(0)), move |shell, _, cx| {
                show_updates(shell, updates, cx);
                shell.state().clone().update(cx, |state, cx| {
                    state.show_update_way_fixture(way, Some(DOWNLOAD_PAGE), installed, cx);
                    if collapsed {
                        state.set_server_book_collapsed(true, cx);
                    }
                });
            })?;
        }
        for (name, way, installed) in [
            ("settings-version-deb", UpdateWay::SystemPackage, false),
            ("settings-version-move-first", UpdateWay::MoveFirst, false),
            (
                "settings-version-not-a-package",
                UpdateWay::NotAPackage,
                false,
            ),
            ("settings-version-installed", UpdateWay::InApp, true),
        ] {
            studio.draw(
                Screen::new(name, three_servers(0)),
                move |shell, window, cx| {
                    show_updates(shell, available(Some(PAGE)), cx);
                    shell.state().clone().update(cx, |state, cx| {
                        state.show_update_way_fixture(way, Some(DOWNLOAD_PAGE), installed, cx);
                    });
                    shell.open_version_settings(window, cx);
                },
            )?;
        }
    }

    // A server that needs a newer client, with the newer version that is known and
    // without one.
    let too_old = || {
        vec![
            acme(),
            ServerSnapshot {
                stream: StreamStatus::Off,
                ..harbor_as(
                    AccessState::LoggedOut {
                        account_id: person("Luca"),
                    },
                    Reach::Incompatible(Incompatibility::ClientTooOld { server_oldest: 3 }),
                    None,
                )
            },
        ]
    };
    studio.draw(
        Screen::new("server-needs-update-with-version", too_old()),
        |shell, _, cx| {
            show_updates(shell, available(Some(PAGE)), cx);
            select(shell, HARBOR, cx);
        },
    )?;
    studio.draw(
        Screen::new("server-needs-update", too_old()),
        |shell, _, cx| select(shell, HARBOR, cx),
    )?;
    Ok(())
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    draw_book(studio)?;
    draw_arrangement(studio)?;
    draw_update(studio)
}
