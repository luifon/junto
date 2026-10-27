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

use crate::harness::{Admin, Window, World, member, selector};
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
        self.run_tasks();
        self.state
            .read_with(&self.cx, |state, _| state.kept_notifications())
    }

    pub(crate) fn notices(&mut self) -> Option<NoticesPanel> {
        self.view().lists()?.notices.clone()
    }

    pub(crate) fn unread_notices(&mut self) -> usize {
        self.view().lists().map_or(0, |lists| lists.unread_notices)
    }

    pub(crate) fn open_notices(&mut self) {
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
    window.click("settings-section-quiet");
    assert!(window.shows("settings-dialog") && window.shows("do-not-disturb"));
    assert_eq!(
        window.view().quiet.do_not_disturb,
        Some(Agreement::All(false))
    );
    window.click_in_settings("do-not-disturb");
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
    window.click_in_settings("do-not-disturb");
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
    // show the change before that.
    let reloads = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.reloads_of(server_id))
    };

    // A daily quiet time is written as two times of day, in the zone of the system or in
    // a zone that the person chooses from the list (ADR 0798).
    assert!(window.shows("quiet-zone"));
    let scroll = window.in_shell(|shell, _, _| shell.settings_scroll_handle());
    window.reveal(&scroll, "quiet-zone-change");
    window.click("quiet-zone-change");
    window.in_shell(|shell, window, cx| shell.type_quiet_zone_search("tokyo", window, cx));
    window.reveal(&scroll, "quiet-zone-choice:Asia/Tokyo");
    window.click("quiet-zone-choice:Asia/Tokyo");
    assert!(!window.shows("quiet-zone-search"));
    window.in_shell(|shell, window, cx| shell.type_quiet_times("22:00", "7:30", window, cx));
    let before = reloads(&mut window);
    window.click("set-daily-quiet");
    window.until("the daily quiet time", |window| {
        matches!(
            window.view().quiet.daily,
            Some(Agreement::All(Some(daily)))
                if daily.start_minute == 22 * 60
                    && daily.end_minute == 7 * 60 + 30
                    && daily.time_zone == "Asia/Tokyo"
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

    // The replacement of the state keeps the rows of the notices while their messages
    // are read again, and the list offers "Mark all as read" at once (ADR 0742).
    window
        .cx
        .update(|_, cx| state.update(cx, |state, cx| state.open_notices(server_id, cx)));
    assert!(window.shows("mark-all-read"));
    assert_eq!(window.notices().unwrap().rows.len(), 3);

    // "Mark all as read" marks the three notices in one request, and the count ends.
    window.until("the notices are listed", |window| {
        window.notices().is_some_and(|panel| !panel.reading)
    });
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
                    default.level.is_none() && !default.may_set
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
    for number in 0..60 {
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

/// After a jump to the message of a notice the list stays at the message, and follows the end
/// again when the person brings it there with the wheel: the messages after the message are
/// read and the button to the end goes away (ADR 0740).
#[gpui::test]
fn after_a_jump_to_a_notice_the_wheel_to_the_end_reads_the_channel(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    let first = admin.mention(&world, general, "far above, ", own);
    for number in 0..60 {
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
    window.cx.update(|window, _| window.activate_window());
    window.settle();

    window.click_named(format!("notice:{}", first.id.as_uuid()));
    window.until("the message of the notice is shown", |window| {
        window.selected_channel() == general
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == first.id && row.target)
    });
    assert!(!window.pane_at_end(Pane::Main));
    assert!(window.feed().is_some_and(|feed| feed.unread > 0));

    window.until("the wheel brings the list to its end", |window| {
        if !window.pane_at_end(Pane::Main) {
            window.wheel_over(Pane::Main, -300.0);
        }
        window.pane_at_end(Pane::Main)
    });
    window.until("the channel is read", |window| {
        window.feed().is_some_and(|feed| feed.unread == 0)
    });
    assert!(!window.shows("go-to-end"));
}

/// A jump to a message with more later messages than the page around it: at the bottom of
/// its loaded rows the list reads one newer page, which arrives below the view; the list does
/// not follow, reads no further page by itself, and the place is not read (ADR 0740).
#[gpui::test]
fn at_the_bottom_of_the_rows_of_a_jump_one_newer_page_arrives_below_the_view(
    cx: &mut TestAppContext,
) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::SendMessage;
    let world = World::new();
    let (server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    // The page around a message holds 100 later messages; 160 later ones leave more. Two
    // accounts write them: the server takes at most 120 writes of one account in a minute.
    let code = admin.invite(&world, 1);
    let (_device, bruno) = crate::admin_pages::joined(&world, &server, &code, "Bruno");
    let first = admin.mention(&world, general, "far above, ", own);
    for number in 0..160 {
        let text = format!("filler {number}");
        if number % 2 == 0 {
            admin.send(&world, general, document(&text));
        } else {
            let message = SendMessage {
                document: document(&text),
                reply_to: None,
                attachments: Vec::new(),
            };
            world
                .run(
                    bruno
                        .api()
                        .send_message(PlaceId::Channel(general), &message),
                )
                .unwrap();
        }
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
    window.cx.update(|window, _| window.activate_window());
    window.settle();
    window.click_named(format!("notice:{}", first.id.as_uuid()));
    window.until("the message of the notice is shown", |window| {
        window.selected_channel() == general
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == first.id && row.target)
    });
    assert!(window.feed().is_some_and(|feed| feed.more_after));
    let loaded = window.messages().len();
    let last = window.messages().last().unwrap().message.id;

    // One turn of the wheel brings the list to the bottom of its loaded rows, before the
    // newer page arrives.
    let bounds_of = |window: &mut Window, message_id| {
        window
            .cx
            .debug_bounds(crate::harness::selector(format!("message:{message_id}")))
    };
    window.wheel_over(Pane::Main, -20000.0);
    let list = window
        .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
        .expect("the list was drawn");
    assert!(
        bounds_of(&mut window, last)
            .is_some_and(|row| row.bottom() <= list.bottom() + gpui::px(2.0)),
        "the wheel did not bring the list to the bottom of its loaded rows"
    );
    window.until("the newer page is read", |window| {
        window.messages().len() > loaded
    });
    window.settle();
    // The list does not move past the last row that the person saw, and the rows of the newer
    // page are below the view.
    assert!(
        bounds_of(&mut window, last).is_none_or(|row| row.top() >= list.top()),
        "the list moved past the last row that the person saw"
    );
    let newest = window.messages().last().unwrap().message.id;
    assert!(
        bounds_of(&mut window, newest).is_none_or(|row| row.top() >= list.bottom()),
        "the rows of the newer page are in view"
    );
    assert!(!window.pane_at_end(Pane::Main));
    assert_eq!(window.messages().len(), loaded + 50);
    let feed = window.feed().unwrap();
    assert!(feed.more_after && feed.unread > 0, "the place was read");
    let stored = world
        .run(
            window
                .handle()
                .api()
                .message_state(PlaceId::Channel(general)),
        )
        .unwrap()
        .unread;
    assert!(stored > 0, "the place was read on the server");
    // The client core is told a row, not the end of the history.
    let server_id = window.view().selected.unwrap().id;
    assert!(
        matches!(
            crate::messages::core_anchor(&mut window, server_id, PlaceId::Channel(general)),
            Some(Some(_))
        ),
        "the client core was told that the list stands at the end"
    );

    // The button to the end reads the latest page and the list follows it there.
    window.click("go-to-end");
    window.until("the end of the history is read", |window| {
        window.pane_at_end(Pane::Main)
            && window
                .feed()
                .is_some_and(|feed| !feed.more_after && feed.unread == 0)
    });
    assert!(
        window
            .texts()
            .last()
            .is_some_and(|text| text == "filler 159")
    );
}

/// The notice of a reply in a thread opens the thread over the channel. The jump belongs to
/// the thread: the list of the channel is read as usual while the thread stays open.
#[gpui::test]
fn after_a_notice_of_a_reply_the_channel_under_the_thread_is_read(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    let root = admin.send(&world, general, document("Tracks or mixdown?"));
    let reply = admin.reply(&world, root.id, mention("what do you think, ", own));
    window.until("the notice is counted", |window| {
        window.unread_notices() == 1
    });
    window.open_notices();
    window.until("the notice is listed", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 1 && panel.rows.iter().all(|row| row.jump.is_some())
        })
    });
    window.cx.update(|window, _| window.activate_window());
    window.settle();
    window.click_named(format!("notice:{}", reply.as_uuid()));
    window.until("the reply is shown in its thread", |window| {
        window.selected_channel() == general
            && window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| thread.root_id == root.id)
    });

    admin.send(&world, general, document("A new message in the channel"));
    window.until_message("A new message in the channel");
    let own_handle = window.handle();
    window.until("the channel is read", |window| {
        window.feed().is_some_and(|feed| feed.unread == 0)
            && world
                .run(own_handle.api().message_state(PlaceId::Channel(general)))
                .unwrap()
                .unread
                == 0
    });
    assert!(
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.thread.is_some()),
        "the thread closed"
    );
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

/// gpui has no activation of the application on Linux, so the press asks the window, with
/// the activation token of the press when the system gave one. The window of the test
/// platform is activated with or without a token; the Wayland compositor is checked by the
/// test target `notification_focus`.
#[cfg(target_os = "linux")]
#[gpui::test]
fn a_pressed_notification_brings_the_window_to_the_front(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = crate::harness::open(cx, directory.path());
    for token in [None, Some("a token of the press")] {
        window.simulate(|cx| cx.deactivate_window());
        assert!(!window.cx.update(|window, _| window.is_window_active()));
        TestAppContext::update(&window.cx, |cx| {
            junto_client::notify::bring_to_front(token, cx)
        });
        window.run_tasks();
        assert!(window.cx.update(|window, _| window.is_window_active()));
    }
    window.close();
}

#[gpui::test]
fn a_workspace_default_result_keeps_the_dialog_controls_in_place(cx: &mut TestAppContext) {
    let world = World::new();
    let mut server = world.server("Studio");
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = crate::harness::named(cx, "Owner");
    window.find_server(&proxy.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    window.click("server-menu-button");
    window.click("workspace-notifications");
    window.until("the workspace default is drawn", |window| {
        window.shows("workspace-default-not-set") && window.shows("notify-level-0")
    });
    let before = window.cx.debug_bounds("notify-level-0").unwrap();
    proxy.hold_ending("/notification-default");
    window.click("workspace-default-1");
    window.until("the default request is pending", |window| {
        window.shows("workspace-default-sending")
    });
    let pending = window.cx.debug_bounds("notify-level-0").unwrap();
    proxy.release();
    window.until("the default request has completed", |window| {
        window
            .view()
            .notify
            .and_then(|panel| panel.workspace_default)
            .is_some_and(|default| {
                !default.changing && default.level == Some(NotificationLevel::Mentions)
            })
    });
    let completed = window.cx.debug_bounds("notify-level-0").unwrap();
    assert_eq!(pending, completed);
    assert_eq!(before, completed);

    server.stop();
    window.click("workspace-default-0");
    window.until("the default request failure is drawn", |window| {
        window.shows("notify-refused")
    });
    assert_eq!(window.cx.debug_bounds("notify-level-0"), Some(before));
}

#[gpui::test]
fn the_zone_of_the_newest_reading_is_shown_when_readings_end_in_another_order(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.settle();
    // The person comes back to the window 40 times, and the zone of the system is another
    // one each time. The readings run in parallel on the background threads; in a test
    // they end in a random order. The zone of the last return is shown.
    let zones: Vec<chrono_tz::Tz> = chrono_tz::TZ_VARIANTS.iter().copied().take(40).collect();
    let newest = *zones.last().unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        for zone in zones {
            state.set_zone_reader(move || zone);
            state.set_window_active(false, cx);
            state.set_window_active(true, cx);
        }
    });
    window.settle();
    let shown = window
        .state
        .read_with(&window.cx, |state, _| state.system_zone_name());
    assert_eq!(shown, newest.name());
}

/// Clippy refuses `chrono::Local` in a type position and in `use`, but not as a value
/// (`chrono::Local.timestamp_opt(..)`, `with_timezone(&chrono::Local)`), and each of these
/// reads `/etc/localtime` on the thread that asks (ADR 0730, ADR 0798). No file of the code
/// of the crate uses the name `Local`.
#[test]
fn no_code_of_the_application_names_the_local_zone_of_chrono() {
    fn files(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, found);
            } else if path.extension().is_some_and(|ending| ending == "rs") {
                found.push(path);
            }
        }
    }
    // The directory of this package in the checkout that runs the test, which `cargo test`
    // sets at run time; a path compiled into the binary can name another checkout whose build
    // the shared target directory reused.
    let package = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            panic!("CARGO_MANIFEST_DIR is not set: run the test through cargo test")
        });
    let mut found = Vec::new();
    files(&package.join("src"), &mut found);
    assert!(found.len() > 50, "{found:?}");
    let named: Vec<String> = found
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path).unwrap();
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.split(|letter: char| !(letter.is_alphanumeric() || letter == '_'))
                        .any(|word| word == "Local")
                })
                .map(|(number, line)| format!("{}:{}: {line}", path.display(), number + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        named.is_empty(),
        "the zone of the system is the zone that the state read, not chrono::Local: {named:#?}"
    );
}

#[gpui::test]
fn a_notice_of_an_archived_channel_is_listed_and_leads_to_its_message(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    admin.mention(&world, general, "first, ", own);
    let archived = admin.mention(&world, random, "in the old channel, ", own);
    admin.mention(&world, general, "last, ", own);
    window.until("the three notices are counted", |window| {
        window.unread_notices() == 3
    });

    // The channel is archived: it leaves the channel list, and the state does not know it
    // until the archived list of its workspace is read.
    world
        .run(admin.handle.api().archive_channel(random))
        .unwrap();
    window.until("the archived channel leaves the list", |window| {
        !window.channel_names().contains(&"random".to_owned())
    });

    // The list of the notices names the archived channel at the notice of its message,
    // in the order of the notices: the oldest first.
    window.open_notices();
    window.until("the notice of the archived channel is listed", |window| {
        window.notices().is_some_and(|panel| panel.rows.len() == 3)
    });
    let panel = window.notices().unwrap();
    let texts: Vec<&str> = panel.rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(
        texts,
        ["first, @Luca", "in the old channel, @Luca", "last, @Luca"]
    );
    assert_eq!(
        panel.rows[1].place,
        Whereabouts::Channel {
            name: "random".to_owned(),
            kind: ChannelKind::Text
        }
    );

    // The row leads to the message: the main area shows the archived channel and marks
    // the message.
    window.click_named(format!("notice:{}", panel.rows[1].id));
    window.until("the message in the archived channel is shown", |window| {
        let shows_archived = window.view().workspace().is_some_and(|workspace| {
            workspace.archived.is_some()
                && workspace
                    .channel
                    .as_ref()
                    .is_some_and(|channel| channel.id == random)
        });
        shows_archived
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == archived.id && row.target)
    });
    assert!(window.shows("notices-panel"));
}

#[gpui::test]
fn the_notices_of_other_places_are_listed_when_a_deleted_workspace_took_every_held_notice(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    admin.mention(&world, general, "older, ", own);

    // A second workspace with a channel that holds the newest 50 notices: they fill the
    // first page, and the notice of the first workspace lies on the next page.
    let api = admin.handle.api();
    let lab = world
        .run(api.create_workspace("Lab".to_owned()))
        .unwrap()
        .id;
    world.run(api.add_workspace_member(lab, own)).unwrap();
    let bench = world
        .run(api.create_channel(
            lab,
            &CreateChannel {
                name: "bench".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        ))
        .unwrap()
        .id;
    for number in 1..=50 {
        admin.mention(&world, bench, &format!("bench {number}, "), own);
    }
    // The notices follow the events; a window that starts now holds the first page of them
    // (ADR 0722).
    window.until("every notice arrived", |window| {
        window.unread_notices() == 51
    });
    window.close();
    let mut window = crate::harness::open(cx, directory.path());
    window.until("the window holds the first page", |window| {
        window.unread_notices() == 50
    });
    window.open_notices();
    window.until("the first page holds the notices of the bench", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 50
                && panel.more
                && panel.rows.iter().all(|row| row.text.starts_with("bench"))
        })
    });

    // The workspace is deleted: its notices leave, and none is left to continue from. The
    // list is read again, and the older notice of the first workspace is listed.
    world.run(api.delete_workspace(lab)).unwrap();
    window.until("the older notice is listed", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 1 && panel.rows[0].text == "older, @Luca" && !panel.more
        })
    });
    assert!(!window.shows("no-notices"));
}

/// The notices are ordered as the messages of a list are (ADR 0742): the oldest at the top
/// and the newest at the bottom. The list opens at its end, a new notice is drawn at the
/// bottom while the list stands at its end, and a list that the person moved away from its
/// end keeps the rows in view.
#[gpui::test]
fn the_notices_are_ordered_as_messages_with_the_newest_at_the_bottom(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let mut sent = Vec::new();
    for number in 0..16 {
        sent.push(admin.mention(&world, general, &format!("number {number}, "), own));
    }
    window.until("the notices are counted", |window| {
        window.unread_notices() == 16
    });
    window.open_notices();
    window.until("every notice is listed", |window| {
        window
            .notices()
            .is_some_and(|panel| panel.rows.len() == 16 && !panel.reading)
    });
    let texts: Vec<String> = window
        .notices()
        .unwrap()
        .rows
        .iter()
        .map(|row| row.text.clone())
        .collect();
    let expected: Vec<String> = (0..16)
        .map(|number| format!("number {number}, @Luca"))
        .collect();
    assert_eq!(texts, expected);

    // The list opens at its end: the newest notice is drawn whole at the bottom, and each
    // drawn notice lies above the next newer one.
    let row = |message: &Message| selector(format!("notice:{}", message.id.as_uuid()));
    let list = notice_bounds(&mut window, "notice-list");
    let newest = notice_bounds(&mut window, row(&sent[15]));
    assert!(
        newest.bottom() <= list.bottom() && newest.bottom() >= list.bottom() - gpui::px(12.0),
        "the newest notice {newest:?} is not at the bottom of the list {list:?}"
    );
    let older = notice_bounds(&mut window, row(&sent[14]));
    assert!(older.bottom() <= newest.top() + gpui::px(1.0));
    assert!(
        window
            .cx
            .debug_bounds(row(&sent[0]))
            .is_none_or(|oldest| oldest.bottom() <= list.top()),
        "the oldest notice is in view of a list that opened at its end"
    );

    // A new notice while the list stands at its end is drawn at the bottom, in view.
    let next = admin.mention(&world, general, "number 16, ", own);
    window.until("the new notice is drawn at the bottom", |window| {
        window.cx.debug_bounds(row(&next)).is_some_and(|shown| {
            shown.bottom() <= list.bottom() && shown.bottom() >= list.bottom() - gpui::px(12.0)
        })
    });

    // The person scrolls up: a new notice is added below and the rows in view stay.
    window.dispatch(gpui::ScrollWheelEvent {
        position: list.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(300.0))),
        ..Default::default()
    });
    window.settle();
    let kept = notice_bounds(&mut window, row(&sent[12]));
    let last = admin.mention(&world, general, "number 17, ", own);
    window.until("the newest notice is listed", |window| {
        window.notices().is_some_and(|panel| {
            panel
                .rows
                .last()
                .is_some_and(|newest| newest.id == last.id.as_uuid())
        })
    });
    assert_eq!(notice_bounds(&mut window, row(&sent[12])), kept);
    assert!(
        window
            .cx
            .debug_bounds(row(&last))
            .is_none_or(|shown| shown.top() >= list.bottom())
    );
}

/// The bounds of an element that the window draws now.
fn notice_bounds(window: &mut Window, selector: &'static str) -> gpui::Bounds<gpui::Pixels> {
    window.settle();
    window
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

/// A new notice plays its sound under the rules of a system notification (ADR 0707): the
/// mention sound for a mention, the message sound for a message of a channel with the level
/// "all messages", none while the active window shows the place, and none in a muted
/// channel. Each step waits longer than the shortest time between two sounds of messages.
#[gpui::test]
fn a_notice_plays_its_sound_under_the_rules_of_a_system_notification(cx: &mut TestAppContext) {
    use junto_client_core::protocol::discovery::NotificationSettings;
    use junto_media::audio::sounds::{Sound, heard};
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let random = admin.text_channel(&world, &mut window, "random");
    let pause = |world: &World| {
        world.run(async { tokio::time::sleep(std::time::Duration::from_millis(2_200)).await });
    };
    let last_sound = |window: &mut Window| {
        let (played, rate) = window.tones.played_since_clear();
        heard::recognise(&played, rate)
    };
    let count = |window: &mut Window| window.tones.opened_outputs().len();

    // A mention while no window is active: the mention sound.
    admin.mention(&world, general, "first, ", own);
    window.until("the mention sound", |window| {
        count(window) == 1 && last_sound(window) == Some(Sound::Mention)
    });

    // The active window shows the channel: a mention there plays nothing.
    pause(&world);
    window.cx.update(|window, _| window.activate_window());
    admin.mention(&world, general, "second, ", own);
    window.until_message("second, @Luca");
    pause(&world);
    window.settle();
    assert_eq!(count(&mut window), 1);

    // The channel has the level "all messages" and the active window shows another
    // channel: a message without a mention plays the message sound.
    world
        .run(window.handle().api().set_notification_settings(
            SettingsScope::Channel(general),
            &NotificationSettings {
                level: Some(NotificationLevel::All),
                ..NotificationSettings::default()
            },
        ))
        .unwrap();
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    admin.send(&world, general, document("for everyone"));
    window.until("the message sound", |window| {
        count(window) == 2 && last_sound(window) == Some(Sound::Message)
    });

    // A muted channel plays nothing.
    pause(&world);
    world
        .run(window.handle().api().set_notification_settings(
            SettingsScope::Channel(general),
            &NotificationSettings {
                level: Some(NotificationLevel::All),
                mute: true,
                ..NotificationSettings::default()
            },
        ))
        .unwrap();
    window.until("the channel is muted", |window| {
        window.channel(general).unread.muted
    });
    admin.mention(&world, general, "muted, ", own);
    window.until("the message of the muted channel arrived", |window| {
        window.channel(general).unread.messages >= 2
    });
    pause(&world);
    window.settle();
    assert_eq!(count(&mut window), 2);
}
