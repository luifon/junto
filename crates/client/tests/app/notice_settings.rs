//! The notification settings that a person changes in the window against the real server
//! (ADR 0742): the level of a workspace and the default level of a channel, the switches of
//! role and everyone mentions, each length of a mute and the end of a mute, the mute of a
//! direct conversation, "do not disturb" on two servers, and the older notices.

use gpui::TestAppContext;
use junto_client::state::notices::{Agreement, Mute};
use junto_client::state::view::ServerStatus;
use junto_client_core::ServerHandle;
use junto_client_core::api::SettingsScope;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::discovery::{NotificationLevel, NotificationSettings};
use junto_client_core::protocol::ids::{ChannelId, MessageId, WorkspaceId};
use junto_client_core::protocol::message::{Block, Document, Inline, SendMessage};
use junto_client_core::protocol::roles::CreateWorkspaceRole;
use time::OffsetDateTime;

use crate::harness::{Window, World, member};
use crate::messages::document;
use crate::notices::mention;
use crate::support::{Fault, FaultProxy};

impl Window {
    fn row_of(&mut self, channel_id: ChannelId) -> junto_client::state::view::ChannelRow {
        self.channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .unwrap()
    }

    /// The level that the open dialog shows as stored, once no change is on its way.
    fn stored_level(&mut self) -> Option<Option<NotificationLevel>> {
        self.view()
            .notify
            .filter(|panel| !panel.busy)
            .map(|panel| panel.level.stored)
    }

    /// Opens the notification settings of a channel from the menu of its row.
    fn open_channel_settings(&mut self, channel_id: ChannelId) {
        self.right_click_named(format!("channel-row:{channel_id}"));
        self.click("channel-notify-settings");
        self.until("the settings of the channel", |window| {
            window.shows("notify-dialog")
                && window
                    .view()
                    .notify
                    .is_some_and(|panel| panel.scope == SettingsScope::Channel(channel_id))
        });
    }

    /// Opens the notification settings of the shown workspace from the menu of the server.
    fn open_workspace_settings(&mut self) {
        self.click("server-menu-button");
        self.click("workspace-notifications");
        self.until("the settings of the workspace", |window| {
            window
                .view()
                .notify
                .is_some_and(|panel| matches!(panel.scope, SettingsScope::Workspace(_)))
        });
    }

    /// The messages of the notices that the state of the server holds, the newest first.
    fn notice_ids(&mut self) -> Vec<MessageId> {
        self.settle();
        let server_id = self.view().selected.unwrap().id;
        self.state
            .read_with(&self.cx, |state, _| {
                let model = state.server(server_id)?.live().view().model?;
                Some(
                    model
                        .notices
                        .iter()
                        .map(|notice| notice.message_id)
                        .collect(),
                )
            })
            .unwrap_or_default()
    }

    /// Waits until the state holds the notice of `marker`, a message that the server wrote
    /// after the ones before it, and returns the notices then. A notice of an earlier
    /// message is in the same read of the list, so the list is complete up to the marker.
    #[track_caller]
    fn notices_up_to(&mut self, marker: MessageId) -> Vec<MessageId> {
        self.until_value("the notice of the marker", |window| {
            let held = window.notice_ids();
            held.contains(&marker).then_some(held)
        })
    }

    /// The texts of the notices that the list holds, the newest first.
    fn notice_texts(&mut self) -> Vec<String> {
        self.notices()
            .map(|panel| panel.rows.iter().map(|row| row.text.clone()).collect())
            .unwrap_or_default()
    }
}

/// The settings of a scope that the account stored on its server.
fn stored(world: &World, handle: &ServerHandle, scope: SettingsScope) -> NotificationSettings {
    world
        .run(handle.api().notification_settings(scope))
        .unwrap()
}

/// A window of a member that shows a second channel, so that what arrives in the first
/// channel is not read.
fn scene(
    cx: &mut TestAppContext,
    world: &World,
) -> (
    crate::support::TestServer,
    crate::harness::Admin,
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
    let general = window.selected_channel();
    let random = admin.text_channel(world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    (server, admin, directory, window, general)
}

#[gpui::test]
fn a_person_sets_the_level_of_a_workspace_and_gives_a_channel_back_to_it(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let handle = window.handle();
    let workspace_id: WorkspaceId = window.view().workspace().unwrap().id;

    // The level of the workspace for the person: "All messages". A message without a
    // mention is a notice then.
    window.open_workspace_settings();
    assert_eq!(window.stored_level(), Some(None));
    window.click("notify-level-1");
    window.until("the level of the workspace", |window| {
        window.stored_level() == Some(Some(NotificationLevel::All))
    });
    assert_eq!(
        stored(&world, &handle, SettingsScope::Workspace(workspace_id)).level,
        Some(NotificationLevel::All)
    );
    window.click("dialog-close");
    admin.send(&world, general, document("for the workspace"));
    window.until("the notice of a plain message", |window| {
        window.unread_notices() == 1 && window.kept().shown.len() == 1
    });

    // The channel states that its default is the level of the workspace. The person sets
    // "Nothing" for the channel, and then "Default" again.
    window.open_channel_settings(general);
    assert_eq!(window.stored_level(), Some(None));
    assert_eq!(
        window.view().notify.unwrap().level.inherited,
        Some(NotificationLevel::All)
    );
    assert!(window.shows("level-default"));
    window.click("notify-level-3");
    window.until("the level nothing", |window| {
        window.stored_level() == Some(Some(NotificationLevel::Nothing))
    });
    assert_eq!(
        stored(&world, &handle, SettingsScope::Channel(general)).level,
        Some(NotificationLevel::Nothing)
    );
    window.click("dialog-close");
    admin.send(&world, general, document("for nobody"));
    window.until("the second message is counted", |window| {
        window.row_of(general).unread.messages == 2
    });

    window.open_channel_settings(general);
    window.click("notify-level-0");
    window.until("the default level of the channel", |window| {
        window.stored_level() == Some(None)
    });
    assert_eq!(
        stored(&world, &handle, SettingsScope::Channel(general)).level,
        None
    );
    window.click("dialog-close");
    admin.send(&world, general, document("for the workspace again"));
    window.until("the notice of the channel at its default", |window| {
        window.unread_notices() == 2
    });

    // The workspace takes the level of the server again: "Mentions only". A plain message
    // is no notice, and a mention is one.
    window.open_workspace_settings();
    window.click("notify-level-0");
    window.until("the default level of the workspace", |window| {
        window.stored_level() == Some(None)
    });
    assert_eq!(
        stored(&world, &handle, SettingsScope::Workspace(workspace_id)).level,
        None
    );
    window.click("dialog-close");
    admin.send(&world, general, document("for nobody again"));
    admin.mention(&world, general, "only this, ", own);
    // The server lists a notice of a plain message only while the level of its place is
    // "All messages" (ADR 0409): the two earlier ones are not listed now.
    window.open_notices();
    window.until("the notice of the mention only", |window| {
        window.unread_notices() == 1 && window.notice_texts() == ["only this, @Luca"]
    });
    window.click("close-notices");

    // "All messages" for the workspace again: the earlier notices are listed again, and
    // the two messages that were sent at "Nothing" and at "Mentions only" made none.
    window.open_workspace_settings();
    window.click("notify-level-1");
    window.until("the level of the workspace again", |window| {
        window.stored_level() == Some(Some(NotificationLevel::All))
    });
    window.click("dialog-close");
    window.open_notices();
    window.until("the three notices", |window| {
        window.notice_texts()
            == [
                "only this, @Luca",
                "for the workspace again",
                "for the workspace",
            ]
    });
}

#[gpui::test]
fn the_switches_of_role_and_everyone_mentions_turn_those_mentions_off(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let handle = window.handle();
    let api = admin.handle.api();
    let workspace_id = window.view().workspace().unwrap().id;
    let role = CreateWorkspaceRole {
        name: "Editors".to_owned(),
        permissions: Vec::new(),
    };
    let role_id = world
        .run(api.create_workspace_role(workspace_id, &role))
        .unwrap()
        .id;
    world
        .run(api.assign_workspace_role(workspace_id, own, role_id))
        .unwrap();
    let says = |words: &str, inline: Inline| Document {
        blocks: vec![Block::Paragraph {
            content: vec![
                Inline::Text {
                    text: words.to_owned(),
                    marks: Vec::new(),
                },
                inline,
            ],
        }],
    };
    let channel = |window: &mut Window| window.view().notify.and_then(|panel| panel.channel);

    // Each step ends with a mention of the account, which always makes a notice: when the
    // state holds its notice, it holds the notices of the messages before it.
    let mark = |world: &World, step: &str| admin.mention(world, general, step, own).id;

    // With both switches on, a role of the person and everyone are mentions of the person.
    let role_first = admin
        .send(&world, general, says("editors, ", Inline::Role { role_id }))
        .id;
    let marker = mark(&world, "first step, ");
    assert_eq!(window.notices_up_to(marker), [marker, role_first]);
    window.open_channel_settings(general);
    let shown = channel(&mut window).unwrap();
    assert!(shown.role_mentions && shown.everyone_mentions);

    // The person turns role mentions off. The server lists no notice of a role mention
    // now, and the next role mention makes none; an everyone mention still does.
    window.click("role-mentions");
    window.until("role mentions are off", |window| {
        channel(window).is_some_and(|shown| !shown.role_mentions && shown.everyone_mentions)
            && window.view().notify.is_some_and(|panel| !panel.busy)
    });
    let settings = stored(&world, &handle, SettingsScope::Channel(general));
    assert_eq!(
        (settings.role_mentions, settings.everyone_mentions),
        (Some(false), None)
    );
    admin.send(
        &world,
        general,
        says("editors again, ", Inline::Role { role_id }),
    );
    let everyone_first = admin
        .send(&world, general, says("all of you, ", Inline::Everyone))
        .id;
    let second = mark(&world, "second step, ");
    assert_eq!(
        window.notices_up_to(second),
        [second, everyone_first, marker]
    );

    // And everyone mentions off: the next one makes no notice. A mention of the account
    // still does.
    window.click("everyone-mentions");
    window.until("everyone mentions are off", |window| {
        channel(window).is_some_and(|shown| !shown.role_mentions && !shown.everyone_mentions)
            && window.view().notify.is_some_and(|panel| !panel.busy)
    });
    let settings = stored(&world, &handle, SettingsScope::Channel(general));
    assert_eq!(
        (settings.role_mentions, settings.everyone_mentions),
        (Some(false), Some(false))
    );
    admin.send(
        &world,
        general,
        says("all of you again, ", Inline::Everyone),
    );
    let third = mark(&world, "third step, ");
    assert_eq!(window.notices_up_to(third), [third, second, marker]);

    // Both switches on again: the notices of the first role and everyone mentions are
    // listed again, and the mentions that were sent while a switch was off made none.
    window.click("role-mentions");
    window.until("role mentions are on", |window| {
        channel(window).is_some_and(|shown| shown.role_mentions)
            && window.view().notify.is_some_and(|panel| !panel.busy)
    });
    window.click("everyone-mentions");
    window.until("everyone mentions are on", |window| {
        channel(window).is_some_and(|shown| shown.everyone_mentions)
            && window.view().notify.is_some_and(|panel| !panel.busy)
    });
    window.click("dialog-close");
    window.open_notices();
    window.until("the five notices", |window| {
        window.notice_texts()
            == [
                "third step, @Luca",
                "second step, @Luca",
                "all of you, @everyone",
                "first step, @Luca",
                "editors, @Editors",
            ]
    });
    // Every message arrived and counts as a message.
    assert_eq!(window.row_of(general).unread.messages, 7);
}

#[gpui::test]
fn each_length_of_a_mute_is_stored_and_a_mute_ends_at_its_time(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    let handle = window.handle();
    let zone = chrono_tz::Asia::Tokyo;
    // The zone of the system is read when the person comes back to the window. The window
    // stays inactive, so that a mention is told to the system.
    window.state.update(&mut window.cx, |state, cx| {
        state.set_zone_reader(move || zone);
        state.set_window_active(true, cx);
        state.set_window_active(false, cx);
    });
    window.settle();
    let mute_of = |window: &mut Window| window.row_of(general).mute;
    let stored_mute = |world: &World| {
        let settings = stored(world, &handle, SettingsScope::Channel(general));
        (settings.mute, settings.muted_until)
    };
    let near = |until: Option<OffsetDateTime>, expected: OffsetDateTime| {
        until.is_some_and(|until| (until - expected).abs() < time::Duration::minutes(2))
    };

    // "8 hours" from the menu of the row, and "Unmute" from the same menu.
    window.right_click_named(format!("channel-row:{general}"));
    window.click("channel-mute-1");
    window.until("the mute of 8 hours", |window| {
        matches!(mute_of(window), Mute::Until(_))
    });
    let (muted, until) = stored_mute(&world);
    assert!(muted && near(until, OffsetDateTime::now_utc() + time::Duration::hours(8)));
    window.right_click_named(format!("channel-row:{general}"));
    window.click("channel-unmute");
    window.until("no mute", |window| mute_of(window) == Mute::Off);
    assert_eq!(stored_mute(&world), (false, None));

    // "Until tomorrow" ends at the next midnight in the zone of the person.
    // The next midnight is computed from the clock before and after the click: the click
    // can fall on either side of a midnight of the zone.
    let next_midnight = |now: OffsetDateTime| {
        now.to_offset(offset_in(zone))
            .replace_time(time::Time::MIDNIGHT)
            .checked_add(time::Duration::days(1))
            .unwrap()
            .to_offset(time::UtcOffset::UTC)
    };
    let before = next_midnight(OffsetDateTime::now_utc());
    window.right_click_named(format!("channel-row:{general}"));
    window.click("channel-mute-2");
    window.until("the mute until tomorrow", |window| {
        matches!(mute_of(window), Mute::Until(_))
    });
    let after = next_midnight(OffsetDateTime::now_utc());
    let (muted, until) = stored_mute(&world);
    assert!(
        muted && (until == Some(before) || until == Some(after)),
        "{until:?} {before:?} {after:?}"
    );
    // The dialog of the channel ends it.
    window.open_channel_settings(general);
    assert!(window.shows("dialog-unmute"));
    window.click("dialog-unmute");
    window.until("no mute after the dialog", |window| {
        mute_of(window) == Mute::Off && window.shows("dialog-mute-3")
    });
    assert_eq!(stored_mute(&world), (false, None));

    // "Until I turn it off" from the dialog has no end.
    window.click("dialog-mute-3");
    window.until("the mute without an end", |window| {
        mute_of(window) == Mute::UntilTurnedOff
    });
    assert_eq!(stored_mute(&world), (true, None));
    window.click("dialog-close");
    window.right_click_named(format!("channel-row:{general}"));
    assert!(window.shows("channel-muted-state"));
    window.click("channel-unmute");
    window.until("no mute again", |window| mute_of(window) == Mute::Off);

    // A mute that ends a few seconds from now. At its end the row shows the channel as
    // not muted without an action of the person, and a mention there is told to the
    // system.
    // The end leaves the window enough time to show the mute on a loaded machine.
    let end = OffsetDateTime::now_utc() + time::Duration::seconds(20);
    let settings = NotificationSettings {
        mute: true,
        muted_until: Some(end),
        ..stored(&world, &handle, SettingsScope::Channel(general))
    };
    world
        .run(
            handle
                .api()
                .set_notification_settings(SettingsScope::Channel(general), &settings),
        )
        .unwrap();
    window.until("the short mute is shown", |window| {
        let row = window.row_of(general);
        matches!(row.mute, Mute::Until(_)) && row.unread.muted
    });
    window.until("the mute has ended on screen", |window| {
        let row = window.row_of(general);
        row.mute == Mute::Off && !row.unread.muted
    });
    assert!(OffsetDateTime::now_utc() >= end);
    let told = window.kept().shown.len();
    admin.mention(&world, general, "after the mute, ", own);
    window.until("the mention after the mute is told", |window| {
        window.kept().shown.len() == told + 1 && window.row_of(general).unread.mentions == 1
    });
}

/// The offset of a zone at this moment.
fn offset_in(zone: chrono_tz::Tz) -> time::UtcOffset {
    use chrono::Offset as _;
    use chrono::TimeZone as _;
    let seconds = zone
        .offset_from_utc_datetime(&chrono::Utc::now().naive_utc())
        .fix()
        .local_minus_utc();
    time::UtcOffset::from_whole_seconds(seconds).unwrap()
}

#[gpui::test]
fn a_direct_conversation_is_muted_and_unmuted_from_its_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _general) = scene(cx, &world);
    let ada = admin.handle.account().unwrap();
    let handle = window.handle();
    window.click("members-button");
    window.until("the members", |window| window.shows_member("Ada"));
    window.click_named(format!("message-member:{ada}"));
    window.until("the conversation is shown", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
    });
    let conversation = window.view().lists().unwrap().conversation.clone().unwrap();
    window.click_channel("random");
    let mute_of = |window: &mut Window| {
        window
            .view()
            .lists()
            .and_then(|lists| {
                lists
                    .directs
                    .iter()
                    .find(|row| row.id == conversation.id)
                    .map(|row| row.mute.clone())
            })
            .unwrap()
    };
    let stored_mute = |world: &World| {
        world
            .run(handle.api().snapshot())
            .unwrap()
            .snapshot
            .direct_mutes
            .into_iter()
            .find(|mute| mute.account_id == ada)
            .map(|mute| mute.until)
    };
    let write = |world: &World, words: &str| {
        let message = SendMessage {
            document: document(words),
            reply_to: None,
            attachments: Vec::new(),
        };
        world
            .run(
                admin
                    .handle
                    .api()
                    .send_message(PlaceId::Direct(conversation.id), &message),
            )
            .unwrap();
    };

    // "1 hour" from the menu of the row.
    window.right_click_named(format!("direct-row:{}", conversation.id));
    assert!(window.shows("direct-menu") && !window.shows("direct-unmute"));
    window.click("direct-mute-0");
    window.until("the conversation is muted", |window| {
        matches!(mute_of(window), Mute::Until(_))
    });
    let until = stored_mute(&world).flatten().unwrap();
    assert!(
        (until - (OffsetDateTime::now_utc() + time::Duration::hours(1))).abs()
            < time::Duration::minutes(2)
    );
    write(&world, "while muted");
    window.until("the muted message is counted", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == conversation.id && row.unread.messages == 1)
        })
    });

    // The menu of the muted conversation states the mute and ends it.
    window.right_click_named(format!("direct-row:{}", conversation.id));
    assert!(window.shows("direct-muted-state"));
    window.click("direct-unmute");
    window.until("the conversation is not muted", |window| {
        mute_of(window) == Mute::Off
    });
    assert_eq!(stored_mute(&world), None);
    write(&world, "after the mute");
    window.until("the message after the mute is told", |window| {
        window
            .kept()
            .shown
            .iter()
            .any(|alert| alert.body == "after the mute")
    });
    // The message of the mute was never told.
    assert!(
        window
            .kept()
            .told
            .iter()
            .all(|alert| alert.body != "while muted")
    );

    // "Until I turn it off" has no end.
    window.right_click_named(format!("direct-row:{}", conversation.id));
    window.click("direct-mute-3");
    window.until("the mute without an end", |window| {
        mute_of(window) == Mute::UntilTurnedOff
    });
    assert_eq!(stored_mute(&world), Some(None));
    window.right_click_named(format!("direct-row:{}", conversation.id));
    window.click("direct-unmute");
    window.until("no mute again", |window| mute_of(window) == Mute::Off);
    assert_eq!(stored_mute(&world), None);
}

#[gpui::test]
fn do_not_disturb_on_two_servers_states_a_difference_and_a_server_that_did_not_take_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let acme = world.server("Acme Studio");
    let admin = world.owner(&acme);
    let harbor = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&harbor));
    let (_directory, mut window) = member(cx, &world, &acme.address(), &admin, "Luca");
    window.find_server(&proxy.address());
    window.join_with_code(&harbor.owner_code().unwrap());
    window.until("both servers are ready", |window| {
        let view = window.view();
        view.servers.len() == 2
            && view
                .servers
                .iter()
                .all(|server| server.status == ServerStatus::Ready)
    });
    let handles = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers());
    let names: Vec<(junto_client_core::protocol::ids::ServerId, String)> = window
        .view()
        .servers
        .iter()
        .map(|server| (server.id, server.name.clone()))
        .collect();
    let handle_of = |name: &str| {
        let id = names.iter().find(|(_, held)| held == name).unwrap().0;
        handles
            .iter()
            .find(|handle| handle.id() == id)
            .unwrap()
            .clone()
    };
    let (acme_handle, harbor_handle) = (handle_of("Acme Studio"), handle_of("Harbor"));
    let quiet_on = |world: &World, handle: &ServerHandle| {
        stored(world, handle, SettingsScope::Account).do_not_disturb
    };

    // "Do not disturb" is on for one server only: the settings say that the servers
    // differ.
    let settings = NotificationSettings {
        do_not_disturb: true,
        ..stored(&world, &harbor_handle, SettingsScope::Account)
    };
    world
        .run(
            harbor_handle
                .api()
                .set_notification_settings(SettingsScope::Account, &settings),
        )
        .unwrap();
    window.click("app-settings");
    window.click("settings-section-quiet");
    window.until("the servers differ", |window| {
        window.view().quiet.do_not_disturb == Some(Agreement::Differs)
    });
    assert!(window.shows("quiet-differs"));

    // The switch turns it on for both.
    window.click_in_settings("do-not-disturb");
    window.until("do not disturb on both servers", |window| {
        let quiet = window.view().quiet;
        quiet.do_not_disturb == Some(Agreement::All(true)) && !quiet.busy
    });
    assert!(!window.shows("quiet-differs") && !window.shows("quiet-failed"));
    assert!(quiet_on(&world, &acme_handle) && quiet_on(&world, &harbor_handle));

    // One server does not take the next change: the settings name it, and say that the
    // servers differ again.
    proxy.fail_ending(
        &format!("/notification-settings/{}", uuid::Uuid::nil()),
        [Fault::Cut],
    );
    window.click_in_settings("do-not-disturb");
    window.until("the change of one server failed", |window| {
        let quiet = window.view().quiet;
        !quiet.busy && quiet.failed.len() == 1
    });
    let quiet = window.view().quiet;
    assert_eq!(quiet.failed[0].0, "Harbor");
    assert_eq!(quiet.do_not_disturb, Some(Agreement::Differs));
    assert!(window.shows("quiet-failed") && window.shows("quiet-differs"));
    assert!(!quiet_on(&world, &acme_handle) && quiet_on(&world, &harbor_handle));

    // The switch of servers that differ is drawn off, and a press turns it on for both.
    // The failure is not shown any more.
    window.click_in_settings("do-not-disturb");
    window.until("do not disturb on both servers again", |window| {
        let quiet = window.view().quiet;
        !quiet.busy && quiet.failed.is_empty() && quiet.do_not_disturb == Some(Agreement::All(true))
    });
    assert!(!window.shows("quiet-failed") && !window.shows("quiet-differs"));
    assert!(quiet_on(&world, &acme_handle) && quiet_on(&world, &harbor_handle));
}

#[gpui::test]
fn show_older_lists_the_notices_after_the_first_page(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general) = scene(cx, &world);
    let own = window.own_account();
    for number in 0..55 {
        admin.send(&world, general, mention(&format!("number {number}, "), own));
    }
    // The window counts the notices that it holds: the first page.
    window.until("the notices are counted", |window| {
        window.unread_notices() == 50
    });
    window.open_notices();
    // The list is read again after each notice: the first page is the one with the newest.
    window.until("the first page of notices", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 50
                && panel.more
                && !panel.reading
                && panel.rows[0].text == "number 54, @Luca"
        })
    });

    // The person scrolls to the end of the list, where "Show older" is.
    window.wheel_to("notice-list", "more-notices");
    window.click("more-notices");
    window.until("every notice", |window| {
        window
            .notices()
            .is_some_and(|panel| panel.rows.len() == 55 && !panel.more && !panel.reading)
    });
    assert!(!window.shows("more-notices"));
    let texts = window.notice_texts();
    assert_eq!(texts.last().map(String::as_str), Some("number 0, @Luca"));
    let listed: Vec<uuid::Uuid> = window
        .notices()
        .unwrap()
        .rows
        .iter()
        .map(|row| row.id)
        .collect();
    let stored: Vec<uuid::Uuid> = world
        .run(window.handle().api().notifications(None, Some(100)))
        .unwrap()
        .notifications
        .iter()
        .map(|notice| notice.id)
        .collect();
    assert_eq!(listed, stored);
}
