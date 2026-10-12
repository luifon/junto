use std::sync::Arc;

use junto_client_core::protocol::account::OwnAccount;
use junto_client_core::protocol::call::{CallInvitation, CallPlace, DirectMute};
use junto_client_core::protocol::discovery::{Notification, ScopedNotificationSettings};
use junto_client_core::protocol::events::Cursor;
use junto_client_core::protocol::ids::{CallId, ConversationId, StreamEpoch};
use junto_client_core::protocol::message::{Conversation, MessageState};
use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
use junto_client_core::protocol::structure::{Channel, Workspace};
use time::{Duration, UtcOffset};

use super::*;

/// Thursday, 8 October 2026 at 09:00 UTC.
fn morning() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_450_000).unwrap()
}

/// One server with the workspace "Studio", its text channel "general" and its voice
/// channel "Daily", and a direct conversation with Ana.
struct Scene {
    server_id: ServerId,
    model: ServerModel,
    known: Known,
    own: AccountId,
    ana: AccountId,
    studio: WorkspaceId,
    general: ChannelId,
    daily: ChannelId,
    conversation: Conversation,
}

fn scene() -> Scene {
    let (own, ana) = (AccountId::generate(), AccountId::generate());
    let studio = WorkspaceId::generate();
    let channel = |kind, name: &str, position| Channel {
        id: ChannelId::generate(),
        workspace_id: studio,
        category_id: None,
        kind,
        name: name.to_owned(),
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    };
    let general = channel(ChannelKind::Text, "general", 0);
    let daily = channel(ChannelKind::Voice, "Daily", 1);
    let snapshot = Snapshot {
        cursor: Cursor {
            epoch: StreamEpoch::generate(),
            position: 1,
        },
        account: OwnAccount {
            id: own,
            name: "Luca".to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: vec![studio],
        },
        workspaces: vec![WorkspaceState {
            workspace: Workspace {
                id: studio,
                name: "Studio".to_owned(),
                is_default: true,
            },
            own_role_ids: Vec::new(),
            roles: Vec::new(),
            categories: Vec::new(),
            channels: vec![general.clone(), daily.clone()],
            calls: Vec::new(),
        }],
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        direct_mutes: Vec::new(),
        messages: Vec::new(),
        threads: Vec::new(),
        threads_more: false,
        threads_next: None,
        notifications: Vec::new(),
        notification_settings: Vec::new(),
    };
    let conversation = Conversation {
        id: ConversationId::generate(),
        low_account_id: own.min(ana),
        high_account_id: own.max(ana),
    };
    let mut model = ServerModel::from_snapshot(snapshot);
    model.conversations = vec![conversation.clone()];
    let server_id = ServerId::generate();
    let mut known = Known {
        zone: Zone::Fixed(UtcOffset::UTC),
        today: Some(morning().date()),
        clock: Some(morning()),
        ..Known::default()
    };
    known.names.insert((server_id, ana), "Ana".to_owned());
    Scene {
        server_id,
        model,
        known,
        own,
        ana,
        studio,
        general: general.id,
        daily: daily.id,
        conversation,
    }
}

impl Scene {
    fn in_channel(&self, channel_id: ChannelId) -> MessagePlace {
        MessagePlace::Channel {
            workspace_id: self.studio,
            channel_id,
        }
    }

    fn direct(&self) -> MessagePlace {
        MessagePlace::Direct {
            conversation_id: self.conversation.id,
            low_account_id: self.conversation.low_account_id,
            high_account_id: self.conversation.high_account_id,
        }
    }

    fn message(&self, place: MessagePlace, position: u64, text: &str) -> Message {
        Message {
            id: MessageId::generate(),
            place,
            position,
            author_id: Some(self.ana),
            author_name: None,
            created_at: morning() - Duration::minutes(12),
            edited_at: None,
            document: Some(crate::compose::parse(text, &[])),
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
        }
    }

    /// A notice of a message that the client core has read. The newest notice stands
    /// first, as the server lists them.
    fn notice(&mut self, kind: NotificationKind, message: Message, minutes: i64) -> Uuid {
        let id = self.unread(kind, message.id, minutes);
        self.model
            .named
            .push((message.id, Named::Read(Arc::new(message))));
        id
    }

    /// A notice whose message the client core has not read.
    fn unread(&mut self, kind: NotificationKind, message_id: MessageId, minutes: i64) -> Uuid {
        let id = message_id.as_uuid();
        self.model.notices.insert(
            0,
            Notification {
                id,
                message_id,
                kind,
                created_at: morning() + Duration::minutes(minutes),
                read_at: None,
            },
        );
        id
    }

    fn settings(&mut self, scope: SettingsScope, settings: NotificationSettings) {
        self.model
            .notification_settings
            .retain(|held| held.scope_id != scope.id());
        self.model
            .notification_settings
            .push(ScopedNotificationSettings {
                scope_id: scope.id(),
                settings,
            });
    }

    fn alert_scene(&self, active: bool, shown: Shown) -> AlertScene<'_> {
        AlertScene {
            server_id: self.server_id,
            server_name: "Acme Studio",
            several_servers: false,
            model: &self.model,
            known: &self.known,
            now: morning(),
            active,
            shown,
        }
    }

    /// The rules for a window that is not the active one.
    fn alerts(&self, seen: &mut Seen) -> Alerts {
        alerts(seen, &self.alert_scene(false, Shown::default()))
    }

    /// The window shows this channel of the server.
    fn showing(&self, channel_id: ChannelId) -> Shown {
        Shown {
            server: Some(self.server_id),
            workspace: Some(self.studio),
            channel: Some(channel_id),
            ..Shown::default()
        }
    }
}

fn level(level: NotificationLevel) -> NotificationSettings {
    NotificationSettings {
        level: Some(level),
        ..NotificationSettings::default()
    }
}

#[test]
fn a_notice_is_listed_with_the_place_and_the_text_of_its_message() {
    let mut scene = scene();
    let mention = scene.message(scene.in_channel(scene.general), 7, "can you review this?");
    let direct = scene.message(scene.direct(), 3, "hello");
    let mut reply = scene.message(scene.in_channel(scene.general), 9, "Agreed.");
    let root_id = MessageId::generate();
    reply.thread_root = Some(root_id);
    let elsewhere = scene.message(scene.in_channel(ChannelId::generate()), 2, "not for you");
    let (mention_id, reply_id, direct_id) = (mention.id, reply.id, direct.id);
    scene.notice(NotificationKind::Mention, mention, 0);
    scene.notice(NotificationKind::Direct, direct, 1);
    scene.notice(NotificationKind::Thread, reply, 2);
    // A message that is gone, one of a channel that the state does not hold, one that
    // was not read yet, and one whose read failed.
    let gone = scene.unread(NotificationKind::Mention, MessageId::generate(), 3);
    scene
        .model
        .named
        .push((MessageId::from_uuid(gone), Named::Gone));
    scene.notice(NotificationKind::Mention, elsewhere, 4);
    scene.unread(NotificationKind::Message, MessageId::generate(), 5);
    let failed = scene.unread(NotificationKind::Message, MessageId::generate(), 6);
    scene
        .model
        .named
        .push((MessageId::from_uuid(failed), Named::Failed));
    // The direct message was read on another device.
    scene.model.notices[5].read_at = Some(morning());

    let panel = notices_panel(scene.server_id, &scene.model, &scene.known);
    let ids: Vec<Uuid> = panel.rows.iter().map(|row| row.id).collect();
    assert_eq!(
        ids,
        [
            failed,
            reply_id.as_uuid(),
            direct_id.as_uuid(),
            mention_id.as_uuid()
        ]
    );
    assert!(panel.reading && !panel.more && !panel.marking);
    assert_eq!(panel.unread, 3);

    let unreadable = &panel.rows[0];
    assert_eq!(unreadable.text, text::NOTICE_NOT_READ);
    assert_eq!(
        (&unreadable.place, unreadable.jump),
        (&Whereabouts::Unknown, None)
    );

    let in_thread = &panel.rows[1];
    assert_eq!(in_thread.kind, NotificationKind::Thread);
    let jump = in_thread.jump.unwrap();
    assert_eq!(jump.thread_root, Some(root_id));
    assert_eq!(jump.place, PlaceId::Channel(scene.general));
    assert_eq!(jump.workspace_id, Some(scene.studio));

    let written = &panel.rows[2];
    assert!(written.read);
    assert_eq!(
        written.place,
        Whereabouts::Direct {
            name: Some("Ana".to_owned())
        }
    );
    let jump = written.jump.unwrap();
    assert_eq!(
        (jump.place, jump.workspace_id, jump.position),
        (PlaceId::Direct(scene.conversation.id), None, 3)
    );

    let named = &panel.rows[3];
    assert_eq!(
        named.place,
        Whereabouts::Channel {
            name: "general".to_owned(),
            kind: ChannelKind::Text
        }
    );
    assert_eq!(
        (named.author.as_str(), named.text.as_str()),
        ("Ana", "can you review this?")
    );
    assert_eq!(
        (named.day.as_str(), named.time.as_str()),
        ("Today", "08:48")
    );
    assert_eq!(named.jump.unwrap().message_id, mention_id);

    // The count for the channel column leaves out the message that is gone.
    assert_eq!(unread_notices(&scene.model), 5);
}

#[test]
fn the_list_tells_a_mark_that_runs_and_one_that_failed() {
    let mut scene = scene();
    scene.model.notices_more = true;
    scene.known.notices_marking.insert(scene.server_id);
    scene
        .known
        .notices_failed
        .insert(scene.server_id, "No answer.".to_owned());
    let panel = notices_panel(scene.server_id, &scene.model, &scene.known);
    assert!(panel.marking && panel.more && panel.rows.is_empty());
    assert_eq!(panel.failed.as_deref(), Some("No answer."));
    // Another server tells nothing here.
    let other = notices_panel(ServerId::generate(), &scene.model, &scene.known);
    assert!(!other.marking && other.failed.is_none());
}

#[test]
fn the_level_without_a_stored_one_follows_the_order_of_the_server() {
    let mut scene = scene();
    let (server_id, studio, general) = (scene.server_id, scene.studio, scene.general);
    let channel = SettingsScope::Channel(general);
    let workspace = SettingsScope::Workspace(studio);
    let inherited = |scene: &Scene, scope| {
        notify_panel(scene.server_id, "Acme", scope, &scene.model, &scene.known)
            .unwrap()
            .level
    };

    // The level of the administrators was not read: the client cannot tell.
    assert_eq!(inherited(&scene, channel).inherited, None);
    assert_eq!(
        inherited(&scene, SettingsScope::Account).inherited,
        Some(NotificationLevel::Mentions)
    );

    // The administrators set "all messages".
    scene.known.workspace_defaults.insert(
        (server_id, studio),
        DefaultRead::Read(NotificationLevel::All),
    );
    assert_eq!(
        inherited(&scene, channel).inherited,
        Some(NotificationLevel::All)
    );
    assert_eq!(
        inherited(&scene, workspace).inherited,
        Some(NotificationLevel::All)
    );

    // The route tells "mentions only" also for a workspace without a stored level. With
    // another level of the account the client cannot tell which holds.
    scene.known.workspace_defaults.insert(
        (server_id, studio),
        DefaultRead::Read(NotificationLevel::Mentions),
    );
    assert_eq!(
        inherited(&scene, channel).inherited,
        Some(NotificationLevel::Mentions)
    );
    scene.settings(SettingsScope::Account, level(NotificationLevel::Nothing));
    assert_eq!(inherited(&scene, channel).inherited, None);
    assert_eq!(inherited(&scene, workspace).inherited, None);
    assert_eq!(
        inherited(&scene, SettingsScope::Account).stored,
        Some(NotificationLevel::Nothing)
    );

    // The level that the person stored for the workspace decides for its channels.
    scene.settings(workspace, level(NotificationLevel::All));
    assert_eq!(
        inherited(&scene, channel),
        LevelView {
            stored: None,
            inherited: Some(NotificationLevel::All)
        }
    );
    scene.settings(channel, level(NotificationLevel::Nothing));
    assert_eq!(
        inherited(&scene, channel).stored,
        Some(NotificationLevel::Nothing)
    );

    // A scope that the state does not hold has no dialog.
    let unknown = SettingsScope::Channel(ChannelId::generate());
    assert!(notify_panel(server_id, "Acme", unknown, &scene.model, &scene.known).is_none());
}

#[test]
fn the_dialog_of_a_scope_shows_what_belongs_to_it() {
    let mut scene = scene();
    let (server_id, studio, general) = (scene.server_id, scene.studio, scene.general);
    let panel = |scene: &Scene, scope| {
        notify_panel(
            scene.server_id,
            "Acme Studio",
            scope,
            &scene.model,
            &scene.known,
        )
        .unwrap()
    };

    let account = panel(&scene, SettingsScope::Account);
    assert_eq!(account.name, "Acme Studio");
    assert!(account.channel.is_none() && account.workspace_default.is_none());

    let workspace = panel(&scene, SettingsScope::Workspace(studio));
    assert_eq!(workspace.name, "Studio");
    assert_eq!(
        workspace.workspace_default,
        Some(WorkspaceDefault {
            level: DefaultRead::Reading,
            may_set: false
        })
    );
    scene.model.account.is_owner = true;
    scene
        .known
        .workspace_defaults
        .insert((server_id, studio), DefaultRead::Failed);
    assert_eq!(
        panel(&scene, SettingsScope::Workspace(studio)).workspace_default,
        Some(WorkspaceDefault {
            level: DefaultRead::Failed,
            may_set: true
        })
    );

    // A channel: the two mention switches are on until the person turns one off.
    let scope = SettingsScope::Channel(general);
    let channel = panel(&scene, scope);
    assert_eq!(channel.name, "general");
    assert_eq!(
        channel.channel,
        Some(ChannelNotify {
            mute: Mute::Off,
            role_mentions: true,
            everyone_mentions: true
        })
    );
    scene.settings(
        scope,
        NotificationSettings {
            mute: true,
            everyone_mentions: Some(false),
            ..NotificationSettings::default()
        },
    );
    let channel = panel(&scene, scope).channel.unwrap();
    assert_eq!(channel.mute, Mute::UntilTurnedOff);
    assert!(channel.role_mentions && !channel.everyone_mentions);

    // A change that is on its way, one that the server took and the state does not hold
    // yet, and one that the server refused.
    let key = (server_id, scope.id());
    scene.known.notify_busy.insert(key);
    assert!(panel(&scene, scope).busy);
    assert!(!panel(&scene, SettingsScope::Account).busy);
    scene.known.notify_busy.clear();
    scene
        .known
        .notify_sent
        .insert(key, level(NotificationLevel::All));
    let shown = panel(&scene, scope);
    assert_eq!(shown.level.stored, Some(NotificationLevel::All));
    assert_eq!(shown.channel.unwrap().mute, Mute::Off);
    scene.known.notify_refused = Some((server_id, scope.id(), "Not allowed.".to_owned()));
    assert_eq!(
        panel(&scene, scope).refused.as_deref(),
        Some("Not allowed.")
    );
    assert_eq!(panel(&scene, SettingsScope::Account).refused, None);
}

#[test]
fn a_mute_with_an_end_is_told_until_that_end_and_ends_with_it() {
    let mut scene = scene();
    let general = scene.general;
    let scope = SettingsScope::Channel(general);
    let muted_until = |until| NotificationSettings {
        mute: true,
        muted_until: Some(until),
        ..NotificationSettings::default()
    };
    scene.settings(scope, muted_until(morning() + Duration::hours(1)));
    assert_eq!(
        Mute::of_channel(general, &scene.model, &scene.known),
        Mute::Until("10:00".to_owned())
    );
    scene.settings(scope, muted_until(morning() + Duration::hours(15)));
    assert_eq!(
        Mute::of_channel(general, &scene.model, &scene.known),
        Mute::Until("00:00 tomorrow".to_owned())
    );
    // The end has passed: the place is not muted any more.
    scene.settings(scope, muted_until(morning() - Duration::minutes(1)));
    assert_eq!(
        Mute::of_channel(general, &scene.model, &scene.known),
        Mute::Off
    );
    assert!(!Mute::of_channel(scene.daily, &scene.model, &scene.known).is_on());

    // The mute of a direct conversation is stored with the conversation (ADR 0515).
    assert_eq!(
        Mute::of_direct(scene.ana, &scene.model, &scene.known),
        Mute::Off
    );
    scene.model.direct_mutes.push(DirectMute {
        account_id: scene.ana,
        until: None,
    });
    assert_eq!(
        Mute::of_direct(scene.ana, &scene.model, &scene.known),
        Mute::UntilTurnedOff
    );
}

#[test]
fn a_mute_lasts_an_hour_eight_hours_or_until_the_next_day_of_the_person() {
    let zone = Zone::Fixed(UtcOffset::from_hms(-3, 0, 0).unwrap());
    let now = morning();
    assert_eq!(
        MuteFor::OneHour.until(now, zone),
        Some(now + Duration::hours(1))
    );
    assert_eq!(
        MuteFor::EightHours.until(now, zone),
        Some(now + Duration::hours(8))
    );
    // 09:00 UTC is 06:00 three hours behind UTC: the next day begins 18 hours later.
    assert_eq!(
        MuteFor::UntilTomorrow.until(now, zone),
        Some(now + Duration::hours(18))
    );
    assert_eq!(MuteFor::UntilTurnedOff.until(now, zone), None);
}

#[test]
fn the_quiet_times_are_shown_for_all_servers_and_tell_where_they_differ() {
    let mut first = scene();
    let second = scene();
    let quiet = DailyQuiet {
        start_minute: 22 * 60,
        end_minute: 7 * 60,
        utc_offset_minutes: 0,
    };
    let known = first.known.clone();
    // A saved server with a session whose state the device does not hold.
    let away = ServerId::generate();
    let view = |first: &Scene, known: &Known| {
        quiet_view(
            &[
                (first.server_id, "Acme Studio", &first.model),
                (second.server_id, "Harbor", &second.model),
            ],
            &[
                (first.server_id, "Acme Studio"),
                (second.server_id, "Harbor"),
                (away, "Offline"),
            ],
            Some(first.server_id),
            known,
        )
    };
    let shown = view(&first, &known);
    assert_eq!(shown.do_not_disturb, Some(Agreement::All(false)));
    assert_eq!(shown.daily, Some(Agreement::All(None)));
    assert!(!shown.quiet_now && !shown.busy && shown.failed.is_empty());

    first.settings(
        SettingsScope::Account,
        NotificationSettings {
            do_not_disturb: true,
            daily_quiet: Some(quiet.clone()),
            ..NotificationSettings::default()
        },
    );
    let mut known = known;
    known.quiet_busy = true;
    known
        .quiet_failed
        .push((second.server_id, "No answer.".to_owned()));
    let shown = view(&first, &known);
    assert_eq!(shown.do_not_disturb, Some(Agreement::Differs));
    assert_eq!(shown.daily, Some(Agreement::Differs));
    assert!(shown.quiet_now && shown.busy);
    assert_eq!(
        shown.failed,
        [("Harbor".to_owned(), "No answer.".to_owned())]
    );

    // A change that the second server took and that its state does not hold yet.
    known.notify_sent.insert(
        (second.server_id, SettingsScope::Account.id()),
        NotificationSettings {
            do_not_disturb: true,
            daily_quiet: Some(quiet.clone()),
            ..NotificationSettings::default()
        },
    );
    let shown = view(&first, &known);
    assert_eq!(shown.do_not_disturb, Some(Agreement::All(true)));
    assert_eq!(shown.daily, Some(Agreement::All(Some(quiet))));

    // A server that a change left out is named with the reason, also without its state.
    known
        .quiet_failed
        .push((away, text::QUIET_NO_STATE.to_owned()));
    let shown = view(&first, &known);
    assert_eq!(
        shown.failed[1],
        ("Offline".to_owned(), text::QUIET_NO_STATE.to_owned())
    );

    let none = quiet_view(&[], &[], None, &known);
    assert_eq!((none.do_not_disturb, none.daily), (None, None));
}

#[test]
fn a_time_of_day_is_read_as_hours_and_minutes() {
    assert_eq!(parse_clock("22:00"), Some(22 * 60));
    assert_eq!(parse_clock(" 7:30 "), Some(7 * 60 + 30));
    assert_eq!(parse_clock("00:00"), Some(0));
    assert_eq!(parse_clock("23:59"), Some(23 * 60 + 59));
    for refused in [
        "", "22", "24:00", "12:60", "7:5", "ab:cd", "1:2:3", "-1:00", "+7:30",
    ] {
        assert_eq!(parse_clock(refused), None, "{refused}");
    }
    assert_eq!(clock_text(7 * 60 + 5), "07:05");
}

// The rules of a system notification.

#[test]
fn the_first_state_of_a_server_shows_no_system_notification() {
    let mut scene = scene();
    let message = scene.message(scene.in_channel(scene.general), 7, "before the start");
    scene.notice(NotificationKind::Mention, message, -5);
    let mut seen = Seen::default();
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // The same state again shows nothing either.
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // An older notice that a later page of the list brings is not new.
    let older = scene.message(scene.in_channel(scene.general), 3, "long ago");
    let older_id = scene.notice(NotificationKind::Mention, older, -60);
    let last = scene.model.notices.remove(0);
    assert_eq!(last.id, older_id);
    scene.model.notices.push(last);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
}

#[test]
fn an_invitation_that_the_first_state_holds_is_a_system_notification() {
    let mut scene = scene();
    let message = scene.message(scene.in_channel(scene.general), 7, "before the start");
    scene.notice(NotificationKind::Mention, message, -5);
    let invitation = CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id: scene.studio,
            channel_id: scene.daily,
        },
        account_id: scene.own,
        invited_by: scene.ana,
        created_at: morning(),
        expires_at: morning() + Duration::minutes(1),
    };
    scene.model.invitations.push(invitation.clone());
    // The invitation waits for an answer now, also when the application has just
    // started. The notice of before the start is not told.
    let mut seen = Seen::default();
    let told = scene.alerts(&mut seen);
    let tags: Vec<&str> = told
        .show
        .iter()
        .map(|(alert, _)| alert.tag.as_str())
        .collect();
    assert_eq!(tags, [invitation_tag(scene.server_id, invitation.id)]);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // An active window draws the invitation itself, also at its start.
    let mut seen = Seen::default();
    assert_eq!(
        alerts(&mut seen, &scene.alert_scene(true, Shown::default())),
        Alerts::default()
    );
}

#[test]
fn a_new_notice_is_shown_once_with_its_author_its_place_and_its_text() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "can you review this?");
    let message_id = message.id;
    let notice_id = scene.notice(NotificationKind::Mention, message, 1);
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, Vec::<String>::new());
    assert_eq!(told.show.len(), 1);
    let (alert, target) = &told.show[0];
    assert_eq!(alert.tag, notice_tag(scene.server_id, notice_id));
    assert_eq!(alert.title, "Ana mentioned you in general");
    assert_eq!(alert.body, "can you review this?");
    let AlertTarget::Notice {
        notice_id: target_notice,
        jump: Some(jump),
        ..
    } = target
    else {
        panic!("a notice leads to its message");
    };
    assert_eq!(*target_notice, notice_id);
    assert_eq!(
        (jump.message_id, jump.place),
        (message_id, PlaceId::Channel(scene.general))
    );
    assert_eq!(scene.alerts(&mut seen), Alerts::default());

    // With several servers the title names the server. A direct message is titled by
    // its author.
    let direct = scene.message(scene.direct(), 2, "hello");
    scene.notice(NotificationKind::Direct, direct, 2);
    let mut several = scene.alert_scene(false, Shown::default());
    several.several_servers = true;
    let told = alerts(&mut seen, &several);
    assert_eq!(told.show[0].0.title, "Ana · Acme Studio");
}

#[test]
fn a_notice_of_a_quiet_time_is_not_shown_then_and_not_later() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    scene.settings(
        SettingsScope::Account,
        NotificationSettings {
            do_not_disturb: true,
            ..NotificationSettings::default()
        },
    );
    let message = scene.message(scene.in_channel(scene.general), 7, "at night");
    scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // The quiet time ends: the notice of that time stays in the list only.
    scene.settings(SettingsScope::Account, NotificationSettings::default());
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // A notice after the quiet time is shown.
    let message = scene.message(scene.in_channel(scene.general), 8, "in the morning");
    scene.notice(NotificationKind::Mention, message, 2);
    assert_eq!(scene.alerts(&mut seen).show.len(), 1);
}

#[test]
fn the_notices_that_a_quiet_time_held_back_are_not_shown_when_it_ends() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    // While "do not disturb" is on, the server tells no notice. When it is turned off,
    // the new state holds the notices of that time.
    scene.settings(
        SettingsScope::Account,
        NotificationSettings {
            do_not_disturb: true,
            ..NotificationSettings::default()
        },
    );
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    scene.settings(SettingsScope::Account, NotificationSettings::default());
    let message = scene.message(scene.in_channel(scene.general), 7, "held back");
    scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    let message = scene.message(scene.in_channel(scene.general), 8, "afterwards");
    scene.notice(NotificationKind::Mention, message, 2);
    assert_eq!(scene.alerts(&mut seen).show[0].0.body, "afterwards");

    // A daily quiet time ends without a change of the settings. A notice that was made
    // inside it and that the client learns of later is not shown; one that was made
    // after it is. The time of the scene is 09:00 UTC.
    scene.settings(
        SettingsScope::Account,
        NotificationSettings {
            daily_quiet: Some(DailyQuiet {
                start_minute: 2 * 60,
                end_minute: 8 * 60,
                utc_offset_minutes: 0,
            }),
            ..NotificationSettings::default()
        },
    );
    let at_night = scene.message(scene.in_channel(scene.general), 9, "at half past seven");
    scene.notice(NotificationKind::Mention, at_night, -90);
    // The notice of the night is older than the newest one that was seen: the list
    // holds it as a new entry all the same.
    seen.newest = None;
    let told = scene.alerts(&mut seen);
    assert!(
        told.show
            .iter()
            .all(|(alert, _)| alert.body != "at half past seven"),
        "{told:?}"
    );
    let after = scene.message(scene.in_channel(scene.general), 10, "at half past eight");
    scene.notice(NotificationKind::Mention, after, 3);
    // Later than the time span that bounds the system notifications.
    let mut later = scene.alert_scene(false, Shown::default());
    later.now = morning() + ALERT_SPAN + Duration::seconds(1);
    assert_eq!(
        alerts(&mut seen, &later).show[0].0.body,
        "at half past eight"
    );
}

#[test]
fn a_daily_quiet_time_holds_as_do_not_disturb_does() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    // The morning of the scene is 09:00 UTC.
    scene.settings(
        SettingsScope::Account,
        NotificationSettings {
            daily_quiet: Some(DailyQuiet {
                start_minute: 8 * 60,
                end_minute: 10 * 60,
                utc_offset_minutes: 0,
            }),
            ..NotificationSettings::default()
        },
    );
    let message = scene.message(scene.in_channel(scene.general), 7, "in the quiet hour");
    scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // The same hour in another place of the world is not this hour.
    scene.settings(
        SettingsScope::Account,
        NotificationSettings {
            daily_quiet: Some(DailyQuiet {
                start_minute: 8 * 60,
                end_minute: 10 * 60,
                utc_offset_minutes: 180,
            }),
            ..NotificationSettings::default()
        },
    );
    let message = scene.message(scene.in_channel(scene.general), 8, "outside the quiet hour");
    scene.notice(NotificationKind::Mention, message, 2);
    assert_eq!(scene.alerts(&mut seen).show.len(), 1);
}

#[test]
fn an_active_window_that_shows_the_place_of_a_message_shows_no_system_notification() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "you see this");
    scene.notice(NotificationKind::Message, message, 1);
    let shown = scene.showing(scene.general);
    assert_eq!(
        alerts(&mut seen, &scene.alert_scene(true, shown)),
        Alerts::default()
    );

    // The active window shows another channel: the notice is shown.
    let message = scene.message(scene.in_channel(scene.general), 8, "you do not see this");
    scene.notice(NotificationKind::Message, message, 2);
    let elsewhere = scene.showing(scene.daily);
    let told = alerts(&mut seen, &scene.alert_scene(true, elsewhere));
    assert_eq!(told.show[0].0.title, "Ana in general");

    // The window shows the channel and is not the active window of the system.
    let message = scene.message(scene.in_channel(scene.general), 9, "the window is behind");
    scene.notice(NotificationKind::Message, message, 3);
    assert_eq!(
        alerts(&mut seen, &scene.alert_scene(false, shown))
            .show
            .len(),
        1
    );

    // The window shows the same channel of another server.
    let message = scene.message(scene.in_channel(scene.general), 10, "another server");
    scene.notice(NotificationKind::Message, message, 4);
    let other = Shown {
        server: Some(ServerId::generate()),
        ..shown
    };
    assert_eq!(
        alerts(&mut seen, &scene.alert_scene(true, other))
            .show
            .len(),
        1
    );
}

#[test]
fn a_reply_of_a_thread_is_in_view_only_while_its_thread_is_shown() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let root_id = MessageId::generate();
    let reply = |scene: &Scene, position| {
        let mut reply = scene.message(scene.in_channel(scene.general), position, "a reply");
        reply.thread_root = Some(root_id);
        reply
    };
    // The channel is shown, and the thread is not: a reply is not in the channel list.
    let first = reply(&scene, 7);
    scene.notice(NotificationKind::Thread, first, 1);
    let channel = scene.showing(scene.general);
    let told = alerts(&mut seen, &scene.alert_scene(true, channel));
    assert_eq!(told.show[0].0.title, "Ana replied in a thread in general");

    let second = reply(&scene, 8);
    scene.notice(NotificationKind::Thread, second, 2);
    let thread = Shown {
        thread: Some((scene.general, root_id)),
        ..channel
    };
    assert_eq!(
        alerts(&mut seen, &scene.alert_scene(true, thread)),
        Alerts::default()
    );
}

#[test]
fn a_muted_place_shows_no_system_notification() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    // The summary of the channel says "muted".
    scene.model.message_states.push(MessageState {
        place: scene.in_channel(scene.general),
        last_read: 1,
        latest: 9,
        unread: 1,
        mentions: 0,
        unread_sign: false,
        notification_mentions: 0,
        muted: true,
    });
    let message = scene.message(scene.in_channel(scene.general), 7, "muted");
    scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());

    // The stored settings of a channel without a summary say "muted".
    scene.settings(
        SettingsScope::Channel(scene.daily),
        NotificationSettings {
            mute: true,
            ..NotificationSettings::default()
        },
    );
    let message = scene.message(scene.in_channel(scene.daily), 3, "muted too");
    scene.notice(NotificationKind::Mention, message, 2);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());

    // A muted direct conversation.
    scene.model.direct_mutes.push(DirectMute {
        account_id: scene.ana,
        until: None,
    });
    let message = scene.message(scene.direct(), 4, "muted as well");
    scene.notice(NotificationKind::Direct, message, 3);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());

    // The mutes end: a notice of each place is shown.
    scene.model.message_states.clear();
    scene.model.notification_settings.clear();
    scene.model.direct_mutes.clear();
    for (minute, place) in [
        scene.in_channel(scene.general),
        scene.in_channel(scene.daily),
        scene.direct(),
    ]
    .into_iter()
    .enumerate()
    {
        let message = scene.message(place, 20, "heard again");
        scene.notice(NotificationKind::Mention, message, 10 + minute as i64);
    }
    assert_eq!(scene.alerts(&mut seen).show.len(), 3);
}

#[test]
fn a_notice_waits_for_the_read_of_its_message() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "read a moment later");
    scene.unread(NotificationKind::Mention, message.id, 1);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    scene
        .model
        .named
        .push((message.id, Named::Read(Arc::new(message))));
    let told = scene.alerts(&mut seen);
    assert_eq!(told.show[0].0.body, "read a moment later");
    assert_eq!(scene.alerts(&mut seen), Alerts::default());

    // A message that the server no longer has shows nothing; one whose read failed is
    // shown without its text and leads to its server.
    let gone = scene.unread(NotificationKind::Mention, MessageId::generate(), 2);
    scene
        .model
        .named
        .push((MessageId::from_uuid(gone), Named::Gone));
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    let failed = scene.unread(NotificationKind::Mention, MessageId::generate(), 3);
    scene
        .model
        .named
        .push((MessageId::from_uuid(failed), Named::Failed));
    let told = scene.alerts(&mut seen);
    assert_eq!(told.show[0].0.body, text::ALERT_WITHOUT_TEXT);
    assert_eq!(
        told.show[0].1,
        AlertTarget::Notice {
            server_id: scene.server_id,
            notice_id: failed,
            jump: None
        }
    );
}

#[test]
fn one_change_shows_three_system_notifications_at_most() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    for minute in 1..=5 {
        let message = scene.message(
            scene.in_channel(scene.general),
            7,
            &format!("number {minute}"),
        );
        scene.notice(NotificationKind::Message, message, minute);
    }
    let told = scene.alerts(&mut seen);
    let bodies: Vec<&str> = told
        .show
        .iter()
        .map(|(alert, _)| alert.body.as_str())
        .collect();
    assert_eq!(bodies, ["number 1", "number 2", "number 3"]);
    // The two others are in the list only.
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
}

#[test]
fn a_system_notification_is_taken_away_when_its_notice_was_read() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "read on another device");
    let notice_id = scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen).show.len(), 1);
    scene.model.notices[0].read_at = Some(morning());
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, [notice_tag(scene.server_id, notice_id)]);
    assert!(told.show.is_empty());
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
}

#[test]
fn an_invitation_into_a_call_is_a_system_notification_while_the_window_is_not_active() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let invitation = |scene: &Scene| CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id: scene.studio,
            channel_id: scene.daily,
        },
        account_id: scene.own,
        invited_by: scene.ana,
        created_at: morning(),
        expires_at: morning() + Duration::minutes(1),
    };
    let first = invitation(&scene);
    scene.model.invitations.push(first.clone());
    let told = scene.alerts(&mut seen);
    assert_eq!(told.show.len(), 1);
    assert_eq!(
        told.show[0].0.tag,
        invitation_tag(scene.server_id, first.id)
    );
    assert_eq!(told.show[0].0.title, "Ana invites you to Daily.");
    assert_eq!(told.show[0].1, AlertTarget::Invitation(scene.server_id));
    assert_eq!(scene.alerts(&mut seen), Alerts::default());

    // The invitation ended: its system notification is taken away.
    scene.model.invitations.clear();
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, [invitation_tag(scene.server_id, first.id)]);

    // An active window draws the invitation itself.
    scene.model.invitations.push(invitation(&scene));
    assert!(
        alerts(&mut seen, &scene.alert_scene(true, Shown::default()))
            .show
            .is_empty()
    );
    // An invitation that the account made for another person is none of its own.
    let mut made = invitation(&scene);
    made.account_id = scene.ana;
    made.invited_by = scene.own;
    scene.model.invitations.push(made);
    assert!(scene.alerts(&mut seen).show.is_empty());
}

#[test]
fn the_target_of_a_system_notification_names_its_notice_and_not_its_message() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "another identifier");
    scene.notice(NotificationKind::Mention, message, 1);
    // A server is free to give a notice another identifier than its message has.
    let notice_id = Uuid::now_v7();
    scene.model.notices[0].id = notice_id;
    let told = scene.alerts(&mut seen);
    assert_eq!(told.show[0].0.tag, notice_tag(scene.server_id, notice_id));
    let AlertTarget::Notice {
        notice_id: target, ..
    } = told.show[0].1
    else {
        panic!("the target of a notice");
    };
    assert_eq!(target, notice_id);
}

#[test]
fn a_burst_of_notices_shows_three_system_notifications_in_ten_seconds() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    // Twenty notices arrive in one change, as after a connection that returned. Their
    // messages are read in the changes after it.
    let messages: Vec<Message> = (1..=20)
        .map(|number| {
            scene.message(
                scene.in_channel(scene.general),
                number,
                &format!("number {number}"),
            )
        })
        .collect();
    for (minute, message) in messages.iter().enumerate() {
        scene.unread(NotificationKind::Message, message.id, minute as i64 + 1);
    }
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    let mut shown = 0;
    for message in messages {
        scene
            .model
            .named
            .push((message.id, Named::Read(Arc::new(message))));
        shown += scene.alerts(&mut seen).show.len();
    }
    assert_eq!(shown, MOST_ALERTS);

    // A notice of a later moment is shown again.
    let later = scene.message(scene.in_channel(scene.general), 30, "after the burst");
    scene.notice(NotificationKind::Message, later, 30);
    let mut after = scene.alert_scene(false, Shown::default());
    after.now = morning() + ALERT_SPAN - Duration::seconds(1);
    assert!(alerts(&mut seen, &after).show.is_empty());
    let again = scene.message(scene.in_channel(scene.general), 31, "much later");
    scene.notice(NotificationKind::Message, again, 31);
    let mut after = scene.alert_scene(false, Shown::default());
    after.now = morning() + ALERT_SPAN + Duration::seconds(1);
    let told = alerts(&mut seen, &after);
    assert_eq!(told.show.len(), 1);
    assert_eq!(told.show[0].0.body, "much later");
}

#[test]
fn an_invitation_is_shown_before_the_notices_of_the_same_change() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    for minute in 1..=3 {
        let message = scene.message(
            scene.in_channel(scene.general),
            7,
            &format!("number {minute}"),
        );
        scene.notice(NotificationKind::Message, message, minute);
    }
    let invitation = CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id: scene.studio,
            channel_id: scene.daily,
        },
        account_id: scene.own,
        invited_by: scene.ana,
        created_at: morning(),
        expires_at: morning() + Duration::minutes(1),
    };
    scene.model.invitations.push(invitation.clone());
    let told = scene.alerts(&mut seen);
    let tags: Vec<&str> = told
        .show
        .iter()
        .map(|(alert, _)| alert.tag.as_str())
        .collect();
    assert_eq!(tags.len(), MOST_ALERTS);
    assert_eq!(tags[0], invitation_tag(scene.server_id, invitation.id));
}

#[test]
fn a_system_notification_is_taken_away_when_its_message_or_its_place_is_gone() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let deleted = scene.message(scene.in_channel(scene.general), 7, "deleted a moment later");
    let deleted_message = deleted.id;
    let deleted_id = scene.notice(NotificationKind::Mention, deleted, 1);
    let hidden = scene.message(
        scene.in_channel(scene.daily),
        8,
        "in a channel that is hidden",
    );
    let hidden_id = scene.notice(NotificationKind::Mention, hidden, 2);
    assert_eq!(scene.alerts(&mut seen).show.len(), 2);

    // The message was deleted: the notice stays in the state, and its message is gone.
    for (message_id, named) in &mut scene.model.named {
        if *message_id == deleted_message {
            *named = Named::Gone;
        }
    }
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, [notice_tag(scene.server_id, deleted_id)]);

    // The account lost the channel of the other message.
    let daily = scene.daily;
    for workspace in &mut scene.model.workspaces {
        Arc::make_mut(workspace)
            .channels
            .retain(|channel| channel.channel.id != daily);
    }
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, [notice_tag(scene.server_id, hidden_id)]);
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
}

#[test]
fn a_server_that_leaves_takes_its_system_notifications_with_it() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "before the sign-out");
    let notice_id = scene.notice(NotificationKind::Mention, message, 1);
    let invitation = CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id: scene.studio,
            channel_id: scene.daily,
        },
        account_id: scene.own,
        invited_by: scene.ana,
        created_at: morning(),
        expires_at: morning() + Duration::minutes(1),
    };
    scene.model.invitations.push(invitation.clone());
    assert_eq!(scene.alerts(&mut seen).show.len(), 2);
    assert_eq!(
        seen.tags(scene.server_id),
        [
            notice_tag(scene.server_id, notice_id),
            invitation_tag(scene.server_id, invitation.id)
        ]
    );
}

#[test]
fn the_text_of_a_system_notification_is_one_line_of_a_bounded_length() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    scene.known.names.insert(
        (scene.server_id, scene.ana),
        format!("Ana\n\u{7}{}", "a".repeat(400)),
    );
    let mut message = scene.message(scene.in_channel(scene.general), 7, "");
    message.document = None;
    message.attachments = vec![junto_client_core::protocol::message::Attachment {
        id: junto_client_core::protocol::ids::AttachmentId::generate(),
        name: format!("report\r\n{}.pdf", "b".repeat(400)),
        media_type: "application/pdf".to_owned(),
        bytes: 1,
        sha256: String::new(),
        width: None,
        height: None,
    }];
    scene.notice(NotificationKind::Mention, message, 1);
    let told = scene.alerts(&mut seen);
    let alert = &told.show[0].0;
    assert!(alert.title.starts_with("Ana  aaa"), "{}", alert.title);
    assert_eq!(alert.title.chars().count(), ALERT_TITLE_CHARACTERS);
    assert!(alert.body.starts_with("report  bbb"), "{}", alert.body);
    assert_eq!(alert.body.chars().count(), EXCERPT_CHARACTERS);
    assert!(!alert.title.contains(char::is_control) && !alert.body.contains(char::is_control));
}

#[test]
fn a_system_notification_of_a_deleted_message_is_taken_away() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "deleted a moment later");
    let message_id = message.id;
    let mut marker = message.clone();
    let notice_id = scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen).show.len(), 1);
    // The server keeps the place of a deleted message as a marker without its text.
    marker.deleted = true;
    marker.document = None;
    for (held, named) in &mut scene.model.named {
        if *held == message_id {
            *named = Named::Read(Arc::new(marker.clone()));
        }
    }
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, [notice_tag(scene.server_id, notice_id)]);
    // A deleted message that a new notice names is not told.
    let mut gone = scene.message(scene.in_channel(scene.general), 8, "");
    gone.deleted = true;
    gone.document = None;
    scene.notice(NotificationKind::Mention, gone, 2);
    assert!(scene.alerts(&mut seen).show.is_empty());
}

#[test]
fn a_system_notification_is_taken_away_with_its_place_when_the_message_is_not_held() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(
        scene.in_channel(scene.daily),
        7,
        "in a channel that is hidden",
    );
    let notice_id = scene.notice(NotificationKind::Mention, message, 1);
    assert_eq!(scene.alerts(&mut seen).show.len(), 1);
    // The state was replaced: it holds the notice, not its message yet, and not the
    // channel any more.
    scene.model.named.clear();
    let daily = scene.daily;
    for workspace in &mut scene.model.workspaces {
        Arc::make_mut(workspace)
            .channels
            .retain(|channel| channel.channel.id != daily);
    }
    let told = scene.alerts(&mut seen);
    assert_eq!(told.dismiss, [notice_tag(scene.server_id, notice_id)]);
}

#[test]
fn a_notice_that_waits_while_the_bound_is_reached_is_not_shown_later() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let messages: Vec<Message> = (1..=4)
        .map(|number| {
            scene.message(
                scene.in_channel(scene.general),
                number,
                &format!("number {number}"),
            )
        })
        .collect();
    for (minute, message) in messages.iter().enumerate() {
        scene.unread(NotificationKind::Message, message.id, minute as i64 + 1);
    }
    assert_eq!(scene.alerts(&mut seen), Alerts::default());
    // Three messages are read, and the fourth one much later.
    let mut messages = messages.into_iter();
    for message in messages.by_ref().take(3) {
        scene
            .model
            .named
            .push((message.id, Named::Read(Arc::new(message))));
    }
    assert_eq!(scene.alerts(&mut seen).show.len(), MOST_ALERTS);
    let last = messages.next().unwrap();
    scene
        .model
        .named
        .push((last.id, Named::Read(Arc::new(last))));
    let mut later = scene.alert_scene(false, Shown::default());
    later.now = morning() + ALERT_SPAN + Duration::seconds(1);
    assert_eq!(alerts(&mut seen, &later), Alerts::default());
}

#[test]
fn a_shown_system_notification_leads_to_its_target_until_it_is_taken_away() {
    let mut scene = scene();
    let mut seen = Seen::default();
    scene.alerts(&mut seen);
    let message = scene.message(scene.in_channel(scene.general), 7, "press me");
    let message_id = message.id;
    let notice_id = scene.notice(NotificationKind::Mention, message, 1);
    let told = scene.alerts(&mut seen);
    let tag = told.show[0].0.tag.clone();
    let Some(AlertTarget::Notice {
        notice_id: of,
        jump: Some(jump),
        ..
    }) = seen.target(scene.server_id, &tag)
    else {
        panic!("the shown notification has its target");
    };
    assert_eq!((of, jump.message_id), (notice_id, message_id));
    assert_eq!(seen.target(scene.server_id, "notice:other"), None);
    // A notice that was read has no notification and no target.
    scene.model.notices[0].read_at = Some(morning());
    assert_eq!(scene.alerts(&mut seen).dismiss, std::slice::from_ref(&tag));
    assert_eq!(seen.target(scene.server_id, &tag), None);
}
