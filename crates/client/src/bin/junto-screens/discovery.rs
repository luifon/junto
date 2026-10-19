//! The screens of notices, notification settings, quiet times and search (ADR 0742,
//! ADR 0743): the list of the notices with its states, the menu of a channel, the dialog of
//! each scope, the quiet times of the settings, and view H of the design with the states
//! of a search.

use std::sync::Arc;

use junto_client::shell::{FilterKind, Open, Shell};
use junto_client::state::search::{DateFilter, Filters, Hit, Progress, Search};
use junto_client::state::view::{Known, ServerSnapshot};
use junto_client::text;
use junto_client_core::api::SettingsScope;
use junto_client_core::model::{Named, ServerModel};
use junto_client_core::protocol::discovery::{
    DailyQuiet, Notification, NotificationKind, NotificationLevel, NotificationSettings,
    ScopedNotificationSettings,
};
use junto_client_core::protocol::ids::{
    AccountId, ChannelId, ConversationId, MessageId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::message::{Conversation, Message, MessagePlace};
use time::Duration;

use super::messages::{GENERAL, acme_with, document, general, morning, none, place, prepare};
use super::{Screen, Studio, harbor, id, person};

const ALPHA: u32 = 31;
const RANDOM: u32 = 33;
const DAILY: u32 = 34;

fn server_id() -> ServerId {
    id(1)
}

fn studio_id() -> WorkspaceId {
    id(10)
}

/// The moment of the screens: Thursday, 8 October 2026 at 10:40.
fn now() -> time::OffsetDateTime {
    morning() + Duration::minutes(100)
}

fn with_ana() -> Conversation {
    let (luca, ana) = (person("Luca"), person("Ana"));
    Conversation {
        id: id::<ConversationId>(8001),
        low_account_id: luca.min(ana),
        high_account_id: luca.max(ana),
    }
}

fn message(number: u32, place: MessagePlace, author: &str, minutes: i64, text: &str) -> Message {
    Message {
        id: id(6100 + number),
        place,
        position: 30 + u64::from(number),
        author_id: Some(person(author)),
        author_name: Some(author.to_owned()),
        created_at: morning() + Duration::minutes(minutes),
        edited_at: None,
        document: Some(document(text)),
        system: None,
        reply_to: None,
        permanent: false,
        deleted: false,
        attachments: Vec::new(),
        reactions: Vec::new(),
        thread_root: None,
        reply_count: 0,
        last_reply: None,
        pinned: false,
        files_removed_by_retention: false,
        reply_authors: Vec::new(),
    }
}

/// The notices of the design's morning: a mention, a reply of a thread, a message of a
/// channel at the level "all messages", and a direct message that was read.
fn notices(model: &mut ServerModel) {
    let conversation = with_ana();
    let direct = MessagePlace::Direct {
        conversation_id: conversation.id,
        low_account_id: conversation.low_account_id,
        high_account_id: conversation.high_account_id,
    };
    model.conversations = vec![conversation];
    let mut reply = message(
        2,
        place(GENERAL),
        "Diego",
        72,
        "Done, @Luca. I am writing it up now, and I will fork this thread if it grows.",
    );
    reply.thread_root = Some(id(6199));
    let held = [
        (
            NotificationKind::Mention,
            message(
                1,
                place(ALPHA),
                "Marta",
                74,
                "Agreed. @Luca, please add our legal contact to the invite.",
            ),
            false,
        ),
        (NotificationKind::Thread, reply, false),
        (
            NotificationKind::Message,
            message(3, place(RANDOM), "Carla", 31, "Lunch at one?"),
            true,
        ),
        (
            NotificationKind::Direct,
            message(
                4,
                direct,
                "Ana",
                -(60 * 18),
                "Can you take the demo on Friday?",
            ),
            true,
        ),
    ];
    for (kind, message, read) in held {
        model.notices.push(Notification {
            id: message.id.as_uuid(),
            message_id: message.id,
            kind,
            created_at: message.created_at,
            read_at: read.then(|| message.created_at + Duration::minutes(5)),
        });
        model
            .named
            .push((message.id, Named::Read(Arc::new(message))));
    }
}

fn settings(model: &mut ServerModel, scope: SettingsScope, settings: NotificationSettings) {
    model
        .notification_settings
        .push(ScopedNotificationSettings {
            scope_id: scope.id(),
            settings,
        });
}

/// "Acme Studio" with the messages of `#general` and the notices of the morning.
fn acme(change: impl FnOnce(&mut ServerModel)) -> Vec<ServerSnapshot> {
    vec![
        acme_with(|model| {
            model.timelines = vec![Arc::new(general().timeline())];
            notices(model);
            change(model);
        }),
        harbor(),
    ]
}

fn fixed(shell: &mut Shell, change: impl FnOnce(&mut Known), cx: &mut gpui::Context<Shell>) {
    prepare(shell, none(), cx);
    shell.state().clone().update(cx, |app, cx| {
        app.show_notices_fixture(now(), change, cx);
    });
}

fn hit(number: u32, place_id: uuid::Uuid, author: &str, minutes: i64, text: &str) -> Hit {
    Hit {
        message_id: Some(id::<MessageId>(6300 + number)),
        recording_id: None,
        line: None,
        place_id,
        workspace_id: None,
        author_id: person(author),
        created_at: morning() + Duration::minutes(minutes),
        text: text.to_owned(),
        position: Some(u64::from(number)),
        thread_root: None,
        start_ms: None,
        page: 0,
    }
}

/// The search of view H: two messages and one line of a transcript.
fn retention() -> Search {
    let spoken = Hit {
        message_id: None,
        recording_id: Some(id(6400)),
        line: Some(211),
        place_id: id::<ChannelId>(DAILY).as_uuid(),
        workspace_id: None,
        author_id: person("Ana"),
        created_at: morning() + Duration::minutes(41),
        text: "Diego writes the retention spec, and Luca sends Marta the demo invite.".to_owned(),
        position: None,
        thread_root: None,
        start_ms: Some(1_843_000),
        page: 0,
    };
    Search {
        server_id: server_id(),
        text: "retention".to_owned(),
        filters: Filters::default(),
        progress: Progress::Done,
        hits: vec![
            hit(
                1,
                id::<ChannelId>(GENERAL).as_uuid(),
                "Luca",
                48,
                "@Bruno can you review the retention settings before lunch? The spec is attached.",
            ),
            hit(
                2,
                id::<ChannelId>(ALPHA).as_uuid(),
                "Marta",
                -(60 * 41) + 12,
                "Our legal team asks how long the retention period for call recordings is.",
            ),
            spoken,
        ],
        more: false,
        reading_more: false,
        open: true,
        going: None,
        go_failed: None,
    }
}

fn searching(
    shell: &mut Shell,
    search: Search,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    shell.type_search(&search.text.clone(), window, cx);
    fixed(shell, |known| known.search = Some(search), cx);
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    // The list of the notices beside the channel, and the count at its entry.
    let show_notices = |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
        fixed(shell, |_| {}, cx);
        shell.toggle_notices(cx);
    };
    studio.draw(Screen::new("notices-panel", acme(|_| {})), show_notices)?;
    studio.draw(
        Screen::new("notices-150", acme(|_| {})).scale(150),
        show_notices,
    )?;
    studio.draw(
        Screen::new(
            "notices-empty",
            acme(|model| {
                model.notices.clear();
                model.named.clear();
            }),
        ),
        show_notices,
    )?;
    studio.draw(
        Screen::new("notices-older", acme(|model| model.notices_more = true)),
        show_notices,
    )?;
    studio.draw(
        Screen::new("notices-marking", acme(|_| {})),
        |shell, _, cx| {
            fixed(
                shell,
                |known| {
                    known.marking_all.insert(server_id(), None);
                },
                cx,
            );
            shell.toggle_notices(cx);
        },
    )?;
    studio.draw(
        Screen::new("notices-failed", acme(|_| {})),
        |shell, _, cx| {
            fixed(
                shell,
                |known| {
                    known.notices_failed.insert(
                        server_id(),
                        "The server gave no answer. Try again.".to_owned(),
                    );
                },
                cx,
            );
            shell.toggle_notices(cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "notices-unreadable",
            acme(|model| {
                let failed: MessageId = id(6190);
                model.notices.insert(
                    0,
                    Notification {
                        id: failed.as_uuid(),
                        message_id: failed,
                        kind: NotificationKind::Mention,
                        created_at: morning() + Duration::minutes(90),
                        read_at: None,
                    },
                );
                model.named.push((failed, Named::Failed));
            }),
        ),
        show_notices,
    )?;

    // The menu of a channel, of a muted channel and of a direct conversation.
    studio.draw(
        Screen::new("channel-menu", acme(|_| {})),
        |shell, window, cx| {
            fixed(shell, |_| {}, cx);
            shell.open_channel_menu(id(ALPHA), window, cx);
        },
    )?;
    let muted = || {
        acme(|model| {
            settings(
                model,
                SettingsScope::Channel(id(RANDOM)),
                NotificationSettings {
                    mute: true,
                    muted_until: Some(now() + Duration::hours(8)),
                    ..NotificationSettings::default()
                },
            );
        })
    };
    studio.draw(
        Screen::new("channel-menu-muted", muted()),
        |shell, window, cx| {
            fixed(shell, |_| {}, cx);
            shell.open_channel_menu(id(RANDOM), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("direct-menu", acme(|_| {})),
        |shell, window, cx| {
            fixed(shell, |_| {}, cx);
            shell.open_direct_menu(with_ana().id, window, cx);
        },
    )?;

    // The dialog of the notification settings of each scope.
    let dialog = |scope: SettingsScope, change: fn(&mut Known)| {
        move |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            shell.open_notify_settings(server_id(), scope, window, cx);
            fixed(shell, change, cx);
        }
    };
    studio.draw(
        Screen::new("notify-channel", acme(|_| {})),
        dialog(SettingsScope::Channel(id(GENERAL)), |_| {}),
    )?;
    studio.draw(
        Screen::new("notify-channel-muted", muted()),
        dialog(SettingsScope::Channel(id(RANDOM)), |_| {}),
    )?;
    let chosen = || {
        acme(|model| {
            settings(
                model,
                SettingsScope::Channel(id(GENERAL)),
                NotificationSettings {
                    level: Some(NotificationLevel::All),
                    everyone_mentions: Some(false),
                    ..NotificationSettings::default()
                },
            );
            settings(
                model,
                SettingsScope::Account,
                NotificationSettings {
                    level: Some(NotificationLevel::Nothing),
                    ..NotificationSettings::default()
                },
            );
        })
    };
    studio.draw(
        Screen::new("notify-channel-set", chosen()),
        dialog(SettingsScope::Channel(id(GENERAL)), |_| {}),
    )?;
    studio.draw(
        Screen::new("notify-channel-refused", acme(|_| {})),
        dialog(SettingsScope::Channel(id(GENERAL)), |known| {
            known.notify_refused = Some((
                server_id(),
                id::<ChannelId>(GENERAL).as_uuid(),
                "The server limits how often an account may do this. Try again in a minute."
                    .to_owned(),
            ));
        }),
    )?;
    studio.draw(
        Screen::new("notify-workspace-member", chosen()),
        dialog(SettingsScope::Workspace(studio_id()), |_| {}),
    )?;
    let administered = || {
        acme(|model| {
            model.account.is_owner = true;
            for workspace in &mut model.workspaces {
                std::sync::Arc::make_mut(workspace).notification_default =
                    Some(NotificationLevel::All);
            }
        })
    };
    studio.draw(
        Screen::new("notify-workspace-administrator", administered()),
        dialog(SettingsScope::Workspace(studio_id()), |_| {}),
    )?;
    // A change of the level that the state does not hold yet (ADR 0214).
    studio.draw(
        Screen::new("notify-workspace-changing", administered()),
        dialog(SettingsScope::Workspace(studio_id()), |known| {
            known.defaults_sent.insert(
                (server_id(), studio_id()),
                junto_client::state::notices::SentDefault {
                    level: Some(NotificationLevel::Nothing),
                    answered: None,
                },
            );
        }),
    )?;
    studio.draw(
        Screen::new("notify-server", chosen()),
        dialog(SettingsScope::Account, |_| {}),
    )?;
    studio.draw(
        Screen::new("server-menu-notifications", acme(|_| {})),
        |shell, window, cx| {
            fixed(shell, |_| {}, cx);
            shell.open(Open::ServerMenu, window, cx);
        },
    )?;

    // The quiet times of the settings of the application.
    let quiet = |do_not_disturb: bool, daily: Option<(u16, u16)>| NotificationSettings {
        do_not_disturb,
        daily_quiet: daily.map(|(start_minute, end_minute)| DailyQuiet {
            start_minute,
            end_minute,
            time_zone: junto_client::zones::FALLBACK.to_owned(),
        }),
        ..NotificationSettings::default()
    };
    let open_settings =
        |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            fixed(shell, |_| {}, cx);
            shell.open(Open::Settings, window, cx);
        };
    studio.draw(Screen::new("settings-quiet", acme(|_| {})), open_settings)?;
    let quiet_on = || {
        let mut servers = acme(|model| {
            settings(
                model,
                SettingsScope::Account,
                quiet(true, Some((22 * 60, 7 * 60 + 30))),
            );
        });
        if let Some(model) = &mut servers[1].model {
            settings(
                Arc::make_mut(model),
                SettingsScope::Account,
                quiet(true, Some((22 * 60, 7 * 60 + 30))),
            );
        }
        servers
    };
    studio.draw(Screen::new("settings-quiet-on", quiet_on()), open_settings)?;
    // The list of time zones of the daily quiet time (ADR 0798).
    studio.draw(
        Screen::new("settings-quiet-zones", quiet_on()),
        |shell, window, cx| {
            fixed(shell, |_| {}, cx);
            shell.open(Open::Settings, window, cx);
            shell.toggle_quiet_zones(cx);
            shell.type_quiet_zone_search("america", window, cx);
        },
    )?;
    // The notice of a removal from a workspace, with the message of the moderator
    // (ADR 0764).
    studio.draw(
        Screen::new(
            "moderation-workspace-removed",
            acme(|model| {
                model.moderation_notices = vec![
                    junto_client_core::protocol::account::ModerationNotice {
                        id: uuid::Uuid::from_u128(6500),
                        kind: junto_client_core::protocol::account::ModerationNoticeKind::WorkspaceRemoved,
                        workspace_id: Some(id(6501)),
                        name: "Lab".to_owned(),
                        message: Some("Thank you for your work on the project.".to_owned()),
                        at: now() - time::Duration::days(1),
                    },
                ];
            }),
        ),
        |shell, _, cx| fixed(shell, |_| {}, cx),
    )?;
    studio.draw(
        Screen::new(
            "settings-quiet-differs",
            acme(|model| {
                settings(model, SettingsScope::Account, quiet(true, None));
            }),
        ),
        |shell, window, cx| {
            fixed(
                shell,
                |known| {
                    known.quiet_failed =
                        vec![(id(2), "The server gave no answer. Try again.".to_owned())];
                },
                cx,
            );
            shell.open(Open::Settings, window, cx);
        },
    )?;

    // View H of the design: a search with its results, and its states.
    studio.draw(
        Screen::new("search-results", acme(|_| {})),
        |shell, window, cx| searching(shell, retention(), window, cx),
    )?;
    studio.draw(
        Screen::new("search-150", acme(|_| {})).scale(150),
        |shell, window, cx| searching(shell, retention(), window, cx),
    )?;
    studio.draw(
        Screen::new("search-filters", acme(|_| {})),
        |shell, window, cx| {
            let mut search = retention();
            search.filters = Filters {
                channel: Some(id(GENERAL)),
                author: Some(person("Luca")),
                date: DateFilter::Last7Days,
            };
            search.hits.truncate(1);
            searching(shell, search, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("search-filter-menu", acme(|_| {})),
        |shell, window, cx| {
            searching(shell, retention(), window, cx);
            shell.open(Open::SearchFilter(FilterKind::Channel), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("search-more", acme(|_| {})),
        |shell, window, cx| {
            let mut search = retention();
            search.more = true;
            searching(shell, search, window, cx);
        },
    )?;
    let state = |progress: Progress| {
        move |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            let mut search = retention();
            search.hits.clear();
            search.progress = progress;
            searching(shell, search, window, cx);
        }
    };
    studio.draw(
        Screen::new("search-searching", acme(|_| {})),
        state(Progress::Searching),
    )?;
    studio.draw(
        Screen::new("search-nothing", acme(|_| {})),
        state(Progress::Done),
    )?;
    studio.draw(
        Screen::new("search-refused", acme(|_| {})),
        state(Progress::Refused(text::SEARCH_TEXT_REFUSED.to_owned())),
    )?;
    studio.draw(
        Screen::new("search-failed", acme(|_| {})),
        state(Progress::Failed(
            "The server gave no answer. Try again.".to_owned(),
        )),
    )?;
    studio.draw(
        Screen::new("search-go-failed", acme(|_| {})),
        |shell, window, cx| {
            let mut search = retention();
            search.go_failed = Some(text::RESULT_GONE.to_owned());
            searching(shell, search, window, cx);
        },
    )?;
    let _: AccountId = person("Luca");
    Ok(())
}
