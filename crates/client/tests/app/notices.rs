//! Notices, notification settings and system notifications in the window against the real
//! server (ADR 0742): a mention makes a notice in the list and a system notification under
//! the rules, the person jumps from a notice, marks notices as read, sets a level, a mute
//! and a quiet time, and loses the notices of a channel with the access to it.
//!
//! The system notifications of a test go to a notifier that keeps what it is told. No test
//! shows a notification of the system.

use gpui::TestAppContext;
use junto_client::notify::Kept;
use junto_client::shell::Pane;
use junto_client::state::notices::{Agreement, Mute, MuteFor, NoticesPanel, Whereabouts};
use junto_client::text;
use junto_client_core::api::SettingsScope;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::discovery::{NotificationKind, NotificationLevel};
use junto_client_core::protocol::ids::{AccountId, ChannelId};
use junto_client_core::protocol::message::{Block, Document, Inline, Message};
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel, SetChannelOverride};

use crate::harness::{Admin, Window, World, member};
use crate::messages::document;

/// A text that names an account.
pub fn mention(text: &str, account_id: AccountId) -> Document {
    Document {
        blocks: vec![Block::Paragraph {
            content: vec![
                Inline::Text {
                    text: text.to_owned(),
                    marks: Vec::new(),
                },
                Inline::Account { account_id },
            ],
        }],
    }
}

impl Admin {
    pub fn text_channel(&self, world: &World, window: &mut Window, name: &str) -> ChannelId {
        let workspace_id = window.view().workspace().unwrap().id;
        let created = world
            .run(self.handle.api().create_channel(
                workspace_id,
                &CreateChannel {
                    name: name.to_owned(),
                    kind: ChannelKind::Text,
                    category_id: None,
                    position: None,
                },
            ))
            .unwrap();
        let name = name.to_owned();
        window.until("the new channel is listed", move |window| {
            window.channel_names().contains(&name)
        });
        created.id
    }

    /// Writes a message that mentions a person into a channel.
    pub fn mention(
        &self,
        world: &World,
        channel_id: ChannelId,
        text: &str,
        of: AccountId,
    ) -> Message {
        self.send(world, channel_id, mention(text, of))
    }
}

impl Window {
    /// What the notifier of the application was told.
    pub fn kept(&mut self) -> Kept {
        self.cx.run_until_parked();
        self.state
            .read_with(&self.cx, |state, _| state.kept_notifications())
    }

    fn notices(&mut self) -> Option<NoticesPanel> {
        self.view().workspace()?.notices.clone()
    }

    fn unread_notices(&mut self) -> usize {
        self.view()
            .workspace()
            .map_or(0, |workspace| workspace.unread_notices)
    }

    fn open_notices(&mut self) {
        self.click("notices-row");
        self.until("the notices are listed", |window| {
            window
                .notices()
                .is_some_and(|panel| !panel.reading && window.shows("notices-panel"))
        });
    }

    fn channel(&mut self, channel_id: ChannelId) -> junto_client::state::view::ChannelRow {
        self.channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .unwrap()
    }
}

/// A window of a member in the text channel of a new server. The window is not the active
/// window of the system.
fn scene(
    cx: &mut TestAppContext,
    world: &World,
) -> (
    crate::support::TestServer,
    Admin,
    tempfile::TempDir,
    Window,
    ChannelId,
) {
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel_id = window.selected_channel();
    (server, admin, directory, window, channel_id)
}

#[gpui::test]
fn a_mention_makes_a_notice_and_a_system_notification_under_the_rules(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let name = window.channel(general).name;
    let random = admin.text_channel(&world, &mut window, "random");

    // Nothing was told yet, and the list is empty.
    assert_eq!(window.kept(), Kept::default());
    assert_eq!(window.unread_notices(), 0);
    window.open_notices();
    assert!(window.shows("no-notices") && !window.shows("mark-all-read"));
    window.click("close-notices");
    assert!(!window.shows("notices-panel"));

    // A mention while the window is not the active window of the system: the notice is
    // counted at the row of the notices, and the system is told with the author, the
    // place and the text.
    let first = admin.mention(&world, general, "can you review this, ", own);
    window.until("the notice is counted", |window| {
        window.unread_notices() == 1
    });
    window.until("the system notification", |window| {
        window.kept().shown.len() == 1
    });
    let alert = window.kept().shown[0].clone();
    assert_eq!(alert.title, format!("Ada mentioned you in {name}"));
    assert_eq!(alert.body, "can you review this, @Luca");
    assert!(window.shows("notices-count"));

    // The list shows the notice with its place, its author and its text.
    window.open_notices();
    let panel = window.notices().unwrap();
    assert_eq!(panel.rows.len(), 1);
    let row = panel.rows[0].clone();
    assert_eq!(row.kind, NotificationKind::Mention);
    assert!(!row.read);
    assert_eq!(
        row.place,
        Whereabouts::Channel {
            name: name.clone(),
            kind: ChannelKind::Text
        }
    );
    assert_eq!(
        (row.author.as_str(), row.text.as_str()),
        ("Ada", "can you review this, @Luca")
    );
    assert!(window.shows_named(format!("notice:{}", row.id)) && window.shows("mark-all-read"));

    // The person looks at the window: the end of the list of the channel is seen, the
    // server marks the notice as read, and the system notification is taken away.
    window.cx.update(|window, _| window.activate_window());
    window.until("the notice is read", |window| {
        window.unread_notices() == 0
            && window
                .notices()
                .is_some_and(|panel| panel.rows.len() == 1 && panel.rows[0].read)
    });
    window.until("the system notification is taken away", |window| {
        let kept = window.kept();
        kept.shown.is_empty() && kept.dismissed == [alert.tag.clone()]
    });
    assert!(!window.shows("notices-count") && !window.shows("mark-all-read"));

    // The active window shows the channel: a second mention there is no system
    // notification.
    admin.mention(&world, general, "and this, ", own);
    window.until_message("and this, @Luca");
    window.until("the second notice is listed and read", |window| {
        window.unread_notices() == 0
            && window
                .notices()
                .is_some_and(|panel| panel.rows.len() == 2 && panel.unread == 0)
    });
    // Only the first mention was ever told to the system.
    assert_eq!(window.kept().told.len(), 1);

    // The active window shows another channel: a mention in the first one is told.
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    let third = admin.mention(&world, general, "one more, ", own);
    window.until("the system notification of the third mention", |window| {
        window.kept().shown.len() == 1
    });
    assert_eq!(window.kept().shown[0].body, "one more, @Luca");
    assert_eq!(window.unread_notices(), 1);

    // The row of the notice leads to its message: the channel is shown, and the message
    // is the one that the list marks. The notice is read then.
    window.until("the third notice is listed", |window| {
        window.notices().is_some_and(|panel| panel.rows.len() == 3)
    });
    window.click_named(format!("notice:{}", third.id.as_uuid()));
    window.until("the channel of the notice is shown", |window| {
        window.selected_channel() == general
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == third.id && row.target)
    });
    window.until("the third notice is read", |window| {
        window.unread_notices() == 0
    });
    assert!(window.shows("notices-panel"));
    assert!(
        !window
            .messages()
            .iter()
            .any(|row| row.message.id == first.id && row.target)
    );

    // A press on a system notification does the same: it shows the message of its notice
    // and marks the notice as read.
    window.click_channel("random");
    window.until("the other channel is shown again", |window| {
        window.selected_channel() == random
    });
    let fourth = admin.mention(&world, general, "and a last one, ", own);
    window.until("the system notification of the fourth mention", |window| {
        window
            .kept()
            .shown
            .iter()
            .any(|alert| alert.body == "and a last one, @Luca")
    });
    let tag = window
        .kept()
        .shown
        .iter()
        .find(|alert| alert.body == "and a last one, @Luca")
        .unwrap()
        .tag
        .clone();
    let state = window.state.clone();
    window
        .cx
        .update(|_, cx| state.update(cx, |state, cx| state.alert_pressed(&tag, cx)));
    window.until(
        "the message of the pressed notification is shown",
        |window| {
            window.selected_channel() == general
                && window
                    .messages()
                    .iter()
                    .any(|row| row.message.id == fourth.id && row.target)
                && window.unread_notices() == 0
        },
    );
    window.until("the pressed notification is taken away", |window| {
        window.kept().dismissed.contains(&tag)
    });
}

#[gpui::test]
fn a_quiet_time_shows_no_system_notification_and_mark_all_reads_the_list(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    // The window shows another channel than the one of the mentions.
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });

    // "Do not disturb" is a switch of the settings. The server holds it.
    window.click("app-settings");
    assert!(window.shows("settings-dialog") && window.shows("do-not-disturb"));
    assert_eq!(
        window.view().quiet.do_not_disturb,
        Some(Agreement::All(false))
    );
    window.click("do-not-disturb");
    window.until("do not disturb is on", |window| {
        let quiet = window.view().quiet;
        quiet.do_not_disturb == Some(Agreement::All(true)) && !quiet.busy && quiet.quiet_now
    });
    assert!(window.shows("quiet-now"));

    // Two mentions in the quiet time: the sign of the channel counts them, and the
    // system is told nothing. The server tells no notice in a quiet time (ADR 0409).
    admin.mention(&world, general, "at night, ", own);
    admin.mention(&world, general, "again at night, ", own);
    window.until("the mentions of the quiet time", |window| {
        window.channel(general).unread.mentions == 2
    });
    assert_eq!(window.kept(), Kept::default());

    // The quiet time ends, and the list holds its notices. They are not told
    // afterwards; a later one is.
    window.click("do-not-disturb");
    window.until("do not disturb is off", |window| {
        let quiet = window.view().quiet;
        quiet.do_not_disturb == Some(Agreement::All(false)) && !quiet.busy
    });
    window.until("the notices of the quiet time", |window| {
        window.unread_notices() == 2
    });
    // The rules decide when the messages of the notices are read: the list has their
    // texts then.
    let server_id = window.view().selected.unwrap().id;
    let state = window.state.clone();
    window
        .cx
        .update(|_, cx| state.update(cx, |state, cx| state.open_notices(server_id, cx)));
    window.until("the texts of the notices of the quiet time", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 2 && panel.rows.iter().all(|row| row.jump.is_some())
        })
    });
    window
        .cx
        .update(|_, cx| state.update(cx, |state, cx| state.close_notices(cx)));
    assert_eq!(window.kept(), Kept::default());
    admin.mention(&world, general, "in the morning, ", own);
    window.until("the system notification after the quiet time", |window| {
        window.kept().shown.len() == 1
    });
    assert_eq!(window.kept().shown[0].body, "in the morning, @Luca");

    // A change of the notification settings replaces the state (ADR 0722). The settings
    // show the change before that. The list of the notices has no row and no "Mark all
    // as read" from the replacement until the messages of its notices are read again, so
    // each change is followed to its replacement before the list is used below.
    let reloads = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.reloads_of(server_id))
    };

    // A daily quiet time is written as two times of day.
    window.in_shell(|shell, window, cx| shell.type_quiet_times("22:00", "7:30", window, cx));
    let before = reloads(&mut window);
    window.click("set-daily-quiet");
    window.until("the daily quiet time", |window| {
        matches!(
            window.view().quiet.daily,
            Some(Agreement::All(Some(daily)))
                if daily.start_minute == 22 * 60 && daily.end_minute == 7 * 60 + 30
        )
    });
    window.until(
        "the state was replaced after the quiet time was set",
        |window| reloads(window) > before,
    );
    // Two equal times are no time span.
    window.in_shell(|shell, window, cx| shell.type_quiet_times("8:00", "08:00", window, cx));
    window.click("set-daily-quiet");
    assert!(window.shows("quiet-time-refused"));
    let before = reloads(&mut window);
    window.click("daily-quiet");
    window.until("no daily quiet time", |window| {
        window.view().quiet.daily == Some(Agreement::All(None))
    });
    window.until(
        "the state was replaced after the quiet time was removed",
        |window| reloads(window) > before,
    );
    window.click("dialog-close");

    // "Mark all as read" marks the three notices, and the count ends.
    window.open_notices();
    assert_eq!(window.notices().unwrap().unread, 3);
    window.click("mark-all-read");
    window.until("every notice is read", |window| {
        window.unread_notices() == 0
            && window
                .notices()
                .is_some_and(|panel| panel.unread == 0 && !panel.marking && panel.rows.len() == 3)
    });
    assert!(window.kept().shown.is_empty());
    // The mentions of the channel are still unread: a notice is not a read position.
    assert_eq!(window.channel(general).unread.mentions, 3);
}

#[gpui::test]
fn a_person_sets_the_level_and_the_mute_of_a_channel(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let server_id = window.view().selected.unwrap().id;
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });

    // With the level "mentions only" a message without a mention is no notice and shows
    // no sign.
    admin.send(&world, general, document("nothing for you"));
    window.until("the unread message without a sign", |window| {
        let unread = window.channel(general).unread;
        unread.messages == 1 && !unread.any()
    });
    assert_eq!(window.unread_notices(), 0);

    // The menu of the channel opens its notification settings. "All messages" makes a
    // notice and a system notification of the next message.
    window.right_click_named(format!("channel-row:{general}"));
    assert!(window.shows("channel-menu"));
    window.click("channel-notify-settings");
    window.until("the settings of the channel", |window| {
        window.shows("notify-dialog")
            && window
                .view()
                .notify
                .is_some_and(|panel| panel.scope == SettingsScope::Channel(general))
    });
    let level = |window: &mut Window| window.view().notify.and_then(|panel| panel.level.stored);
    assert_eq!(level(&mut window), None);
    assert!(window.shows("role-mentions") && window.shows("everyone-mentions"));
    assert!(!window.shows("workspace-default-0"));
    window.click("notify-level-1");
    window.until("the level all messages", |window| {
        level(window) == Some(NotificationLevel::All)
            && window.view().notify.is_some_and(|panel| !panel.busy)
    });
    window.click("dialog-close");
    assert!(!window.shows("notify-dialog"));
    admin.send(&world, general, document("for everyone"));
    window.until("the notice of a plain message", |window| {
        window.unread_notices() == 1 && window.kept().shown.len() == 1
    });
    assert_eq!(window.kept().shown[0].title, "Ada in general");
    assert!(window.channel(general).unread.sign);

    // A mute from the menu: the row says so, shows no sign, and a mention there is no
    // system notification.
    window.right_click_named(format!("channel-row:{general}"));
    assert!(!window.shows("channel-unmute"));
    window.click("channel-mute-1");
    window.until("the channel is muted", |window| {
        let row = window.channel(general);
        matches!(row.mute, Mute::Until(_)) && row.unread.muted && !row.unread.any()
    });
    let told = window.kept().shown.len();
    admin.mention(&world, general, "muted, ", own);
    window.until("the muted mention is counted as a message", |window| {
        window.channel(general).unread.messages == 3
    });
    assert_eq!(window.kept().shown.len(), told);

    // The menu of a muted channel ends the mute.
    window.right_click_named(format!("channel-row:{general}"));
    assert!(window.shows("channel-muted-state"));
    window.click("channel-unmute");
    window.until("the channel is not muted", |window| {
        let row = window.channel(general);
        row.mute == Mute::Off && !row.unread.muted
    });

    // The settings of the server and of the workspace open from the menu of the server.
    window.click("server-menu-button");
    window.click("server-notifications");
    window.until("the settings of the server", |window| {
        window
            .view()
            .notify
            .is_some_and(|panel| panel.scope == SettingsScope::Account)
    });
    assert_eq!(
        window.view().notify.unwrap().name,
        window.view().selected.unwrap().name
    );
    window.click("notify-level-2");
    window.until("the level of the server", |window| {
        level(window) == Some(NotificationLevel::Nothing)
    });
    window.click("dialog-close");
    window.click("server-menu-button");
    window.click("workspace-notifications");
    window.until("the settings of the workspace", |window| {
        window.view().notify.is_some_and(|panel| {
            matches!(panel.scope, SettingsScope::Workspace(_))
                && panel.workspace_default.is_some_and(|default| {
                    // The workspace of a new server sets no level (ADR 0412).
                    default.level == junto_client::state::notices::DefaultRead::Read(None)
                        && !default.may_set
                })
        })
    });
    // A member does not set the level of the workspace for the other members.
    assert!(window.shows("workspace-default-held"));
    assert_eq!(
        window.view().notify.unwrap().server_id,
        server_id,
        "the dialog belongs to the shown server"
    );
    window.click("dialog-close");
}

#[gpui::test]
fn a_person_who_loses_a_channel_loses_its_notices_on_screen(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    admin.mention(&world, general, "soon hidden, ", own);
    window.open_notices();
    window.until("the notice is listed", |window| {
        window.notices().is_some_and(|panel| panel.rows.len() == 1) && window.unread_notices() == 1
    });
    window.until("the system notification of the mention", |window| {
        window.kept().shown.len() == 1
    });
    let tag = window.kept().shown[0].tag.clone();

    // The administrator hides the channel from the role of every member.
    let api = admin.handle.api();
    let workspace_id = window.view().workspace().unwrap().id;
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap().id;
    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    world
        .run(api.set_channel_override(general, everyone, &hide))
        .unwrap();
    window.until("the channel and its notice are gone", |window| {
        !window.channels().iter().any(|row| row.id == general)
            && window.unread_notices() == 0
            && window.notices().is_some_and(|panel| panel.rows.is_empty())
    });
    assert!(window.shows("no-notices"));
    let _ = text::NO_NOTICES;
    // The system notification of the hidden channel is taken away.
    window.until("the system notification is taken away", |window| {
        let kept = window.kept();
        kept.shown.is_empty() && kept.dismissed == [tag.clone()]
    });

    // A change of the settings that the server refuses, asked for without the dialog of
    // its scope as the menu of a row does: the refusal is told above the composer.
    let server_id = window.view().selected.unwrap().id;
    let state = window.state.clone();
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| { state.message_refusal().is_none() })
    );
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_channel_mute(server_id, general, Some(MuteFor::OneHour), cx);
        });
    });
    window.until("the refusal of the mute is told", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.message_refusal().is_some())
    });
}

#[gpui::test]
fn a_second_press_on_a_notice_goes_to_its_message_again(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    let first = admin.mention(&world, general, "far above, ", own);
    for number in 0..30 {
        admin.send(&world, general, document(&format!("filler {number}")));
    }
    window.until("the notice is counted", |window| {
        window.unread_notices() == 1
    });
    window.open_notices();
    window.until("the notice is listed", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 1 && panel.rows.iter().all(|row| row.jump.is_some())
        })
    });

    // The row of the first notice shows its message, away from the end of the list.
    let at_end =
        |window: &mut Window| window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main));
    let at_message = |window: &mut Window| {
        window.selected_channel() == general
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == first.id && row.target)
            && !window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main))
    };
    window.click_named(format!("notice:{}", first.id.as_uuid()));
    window.until("the message of the notice is shown", at_message);
    window.until("the notice is marked read", |window| {
        window
            .notices()
            .is_some_and(|panel| panel.rows.len() == 1 && panel.rows.iter().all(|row| row.read))
    });
    // The person goes to the end of the list and presses the same row again: the list
    // goes to the message again.
    window.click("go-to-end");
    window.until("the end of the list", |window| at_end(window));
    window.click_named(format!("notice:{}", first.id.as_uuid()));
    window.until("the message of the notice is shown again", at_message);
}

#[gpui::test]
fn two_marks_asked_for_one_after_the_other_are_both_sent(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let server_id = window.view().selected.unwrap().id;
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    admin.mention(&world, general, "the first, ", own);
    admin.mention(&world, general, "the second, ", own);
    window.until("the two notices are counted", |window| {
        window.unread_notices() == 2
    });
    window.open_notices();
    window.until("the two notices are listed", |window| {
        window.notices().is_some_and(|panel| panel.rows.len() == 2)
    });
    let unread: Vec<uuid::Uuid> = window
        .notices()
        .unwrap()
        .rows
        .iter()
        .map(|row| row.id)
        .collect();
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.mark_notice_read(server_id, unread[0], cx);
            state.mark_notice_read(server_id, unread[1], cx);
        });
    });
    window.until("both notices are read", |window| {
        window.unread_notices() == 0
    });
}

/// gpui has no activation of the application on Linux, so the press asks the window.
#[cfg(target_os = "linux")]
#[gpui::test]
fn a_pressed_notification_brings_the_window_to_the_front(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = crate::harness::open(cx, directory.path());
    window.cx.deactivate_window();
    assert!(!window.cx.update(|window, _| window.is_window_active()));
    TestAppContext::update(&window.cx, junto_client::notify::bring_to_front);
    window.cx.run_until_parked();
    assert!(window.cx.update(|window, _| window.is_window_active()));
    window.close();
}
