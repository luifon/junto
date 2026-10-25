//! The screens of the server book with several servers, of the update notice, and of the
//! page of the recordings of a workspace (ADR 0780, ADR 0784).

use std::sync::Arc;

use junto_client::shell::{Open, Shell};
use junto_client::state::admin::{AdminOpen, AdminPage, RecordingList};
use junto_client::state::book::UpdatesKnown;
use junto_client::state::feed::Zone;
use junto_client::state::view::ServerSnapshot;
use junto_client::text;
use junto_client_core::Reach;
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::StreamStatus;
use junto_client_core::protocol::ids::{ChannelId, RecordingId};
use junto_client_core::protocol::message::{Conversation, MessagePlace, MessageState};
use junto_client_core::protocol::recording::{
    Recording, RecordingFailure, RecordingState, RecordingStop, RecordingStopReason,
};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::updates::{Release, UpdateState, Version};
use junto_client_core::version::Incompatibility;
use junto_media::calls::Phase;
use time::{Duration, UtcOffset};

use super::{
    Screen, Studio, acme, acme_model, at, channel, harbor, harbor_as, id, in_call, person,
    reachable, recording,
};

// The numbers of the identifiers of the servers of the screens.
const ACME: u32 = 1;
const HARBOR: u32 = 2;
const QUARRY: u32 = 3;
const STUDIO: u32 = 10;
const DAILY: u32 = 34;
const HR: u32 = 36;
const RECORDING: u32 = 4100;

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

/// "Acme Studio" with more channels than the channel column shows at once.
fn acme_with_long_list() -> ServerSnapshot {
    ServerSnapshot {
        model: Some(acme_model(|studio| {
            let workspace_id = studio.workspace.id;
            studio.channels.extend((0..40u32).map(|number| {
                let name = format!("project-{number:02}");
                channel(500 + number, workspace_id, ChannelKind::Text, &name)
            }));
        })),
        ..acme()
    }
}

fn show_updates(shell: &mut Shell, updates: UpdatesKnown, cx: &mut gpui::Context<Shell>) {
    shell
        .state()
        .clone()
        .update(cx, |state, cx| state.show_updates_fixture(updates, cx));
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

    // The menu of a server with the two items that move it.
    studio.draw(
        Screen::new("book-menu-move-first", three_servers(0)),
        |shell, window, cx| shell.open(Open::ServerMenu, window, cx),
    )?;
    studio.draw(
        Screen::new("book-menu-move-middle", three_servers(0)),
        |shell, window, cx| {
            select(shell, HARBOR, cx);
            shell.open(Open::ServerMenu, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("book-menu-move-last", three_servers(0)),
        |shell, window, cx| {
            select(shell, QUARRY, cx);
            shell.open(Open::ServerMenu, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("book-menu-move-failed", three_servers(0)),
        |shell, window, cx| {
            select(shell, HARBOR, cx);
            shell.open(Open::ServerMenu, window, cx);
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
    Ok(())
}

fn draw_update(studio: &mut Studio) -> Result<(), String> {
    // The line of the channel column, directly above the profile strip.
    studio.draw(
        Screen::new("update-line", three_servers(0)),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    // At 150 % the window is narrow: the server book is collapsed, and the line stays.
    studio.draw(
        Screen::new("update-line-scale-150", three_servers(0)).scale(150),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    // A channel list that is longer than the column: the line stays above the strip.
    studio.draw(
        Screen::new(
            "update-line-long-list",
            vec![acme_with_long_list(), harbor()],
        ),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    studio.draw(
        Screen::new(
            "update-line-long-list-scale-150",
            vec![acme_with_long_list(), harbor()],
        )
        .scale(150),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    // Under the strip of the call of this device.
    studio.draw(
        Screen::call("update-line-in-call", in_call(Phase::Connected)),
        |shell, _, cx| show_updates(shell, available(Some(PAGE)), cx),
    )?;
    // A server that shows a state panel, and an empty server book: the channel column
    // holds the profile strip, and the line with it.
    studio.draw(
        Screen::new("update-line-state-panel", three_servers(0)),
        |shell, _, cx| {
            show_updates(shell, available(Some(PAGE)), cx);
            select(shell, QUARRY, cx);
        },
    )?;
    studio.draw(
        Screen::new("update-line-no-server", Vec::new()),
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

/// One recording of a call of "Studio", this number of hours before the newest record of
/// the screens.
fn recorded(
    number: u32,
    channel: u32,
    hours: i64,
    state: RecordingState,
    change: impl FnOnce(&mut Recording),
) -> Recording {
    let requested = at() - Duration::hours(hours);
    let since = requested + Duration::seconds(4);
    let ended = !state.is_active();
    let mut held = recording(|recording| {
        recording.id = id(RECORDING + number);
        recording.workspace_id = id(STUDIO);
        recording.channel_id = id(channel);
        recording.state = state;
        recording.requested_at = requested;
        recording.recording_since = Some(since);
        if ended {
            let at = since + Duration::minutes(36) + Duration::seconds(5);
            recording.stop = Some(RecordingStop {
                reason: RecordingStopReason::Stopped,
                by: Some(person("Ana")),
                at,
            });
            recording.ended_at = Some(at);
        }
    });
    change(&mut held);
    held
}

/// The recordings of "Studio": one in each state, and what the state can lack.
fn recordings() -> Vec<Recording> {
    let gone: ChannelId = id(3999);
    vec![
        recorded(1, DAILY, 0, RecordingState::Starting, |recording| {
            recording.recording_since = None;
            recording.segments = 0;
        }),
        recorded(2, HR, 1, RecordingState::Recording, |recording| {
            recording.started_by = person("Marta");
        }),
        recorded(3, DAILY, 26, RecordingState::Processing, |_| {}),
        recorded(4, DAILY, 50, RecordingState::Ready, |recording| {
            recording.bytes = Some(48_200_000);
            recording.transcript = TranscriptState::Ready;
            recording.transcript_lines = 212;
        }),
        recorded(5, DAILY, 74, RecordingState::Ready, |recording| {
            recording.bytes = Some(1_260_000_000);
            recording.transcript = TranscriptState::Running;
            recording.ended_at = recording
                .recording_since
                .map(|since| since + Duration::hours(1) + Duration::minutes(4));
        }),
        recorded(6, DAILY, 98, RecordingState::PartlyFailed, |recording| {
            recording.failed_segments = 2;
            recording.bytes = Some(31_000_000);
            recording.transcript = TranscriptState::Partial;
            recording.transcript_lines = 40;
        }),
        recorded(7, DAILY, 122, RecordingState::Failed, |recording| {
            recording.recording_since = None;
            recording.failure = Some(RecordingFailure::RecorderUnavailable);
            recording.segments = 0;
        }),
        recorded(8, DAILY, 146, RecordingState::Failed, |recording| {
            recording.failure = Some(RecordingFailure::NothingRecorded);
            recording.transcript = TranscriptState::Failed;
        }),
        // The channel was deleted, and the account that started the recording left.
        recorded(9, 0, 170, RecordingState::Ready, |recording| {
            recording.channel_id = gone;
            recording.started_by = id(4999);
            recording.bytes = Some(912_000);
            recording.transcript = TranscriptState::Waiting;
        }),
    ]
}

/// Opens the page of the recordings of "Studio" and shows this list. The opening of the
/// administration puts the list away, so the list is shown after it.
fn open_recordings(
    shell: &mut Shell,
    list: impl FnOnce(RecordingList) -> RecordingList,
    recordings: Vec<Recording>,
    older: bool,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    shell.state().clone().update(cx, |state, cx| {
        state.set_zone(Zone::Fixed(UtcOffset::UTC), cx)
    });
    let open = AdminOpen::Server {
        server_id: id(ACME),
        page: AdminPage::Recordings,
    };
    shell.open_admin(open, window, cx);
    let loaded = RecordingList::loaded(id(ACME), id(STUDIO), person("Luca"), recordings, older);
    shell.state().clone().update(cx, |state, cx| {
        state.show_recording_list_fixture(list(loaded), cx)
    });
}

fn draw_recordings(studio: &mut Studio) -> Result<(), String> {
    let servers = || vec![acme(), harbor()];
    studio.draw(
        Screen::new("admin-recordings", servers()),
        |shell, window, cx| open_recordings(shell, |list| list, recordings(), true, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-recordings-scale-150", servers()).scale(150),
        |shell, window, cx| open_recordings(shell, |list| list, recordings(), false, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-recordings-empty", servers()),
        |shell, window, cx| open_recordings(shell, |list| list, Vec::new(), false, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-recordings-reading", servers()),
        |shell, window, cx| {
            open_recordings(
                shell,
                RecordingList::with_reading,
                Vec::new(),
                false,
                window,
                cx,
            );
        },
    )?;
    studio.draw(
        Screen::new("admin-recordings-refused", servers()),
        |shell, window, cx| {
            open_recordings(
                shell,
                |list| {
                    list.with_failure("Your account may not read the recordings of this workspace.")
                },
                Vec::new(),
                false,
                window,
                cx,
            );
        },
    )?;
    studio.draw(
        Screen::new("admin-recordings-unreachable", servers()),
        |shell, window, cx| {
            open_recordings(
                shell,
                |list| {
                    list.with_failure(
                        "No connection to the server. Check the address and the network \
                         connection.",
                    )
                },
                recordings(),
                true,
                window,
                cx,
            );
        },
    )?;
    // The page holds the most recordings that it keeps: the end of the list.
    studio.draw(
        Screen::new("admin-recordings-bound", servers()),
        |shell, window, cx| {
            let one = recorded(0, DAILY, 2, RecordingState::Ready, |recording| {
                recording.bytes = Some(48_200_000);
            });
            let many: Vec<Recording> = (0..2000u32)
                .map(|number| Recording {
                    id: id::<RecordingId>(100_000 + number),
                    ..one.clone()
                })
                .collect();
            open_recordings(shell, |list| list, many, true, window, cx);
            shell.scroll_admin_page_to_end();
        },
    )?;
    Ok(())
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    draw_book(studio)?;
    draw_update(studio)?;
    draw_recordings(studio)
}
