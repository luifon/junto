//! The remaining requests of the client in the client core against the real server: the
//! removal of a member from one workspace with its reason and its message (ADR 0106,
//! ADR 0120), the filters of the
//! audit log (ADR 0305), one permission of an override (ADR 0304), the name of the creator
//! of an invite (ADR 0111), the logo of the server (ADR 0101), the position of a search
//! result (ADR 0413), and a quiet time by the name of a time zone (ADR 0412).

use junto_client_core::api::{ErrorCategory, SearchQuery, SettingsScope, Statement};
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::{Notice, StreamStatus};
use junto_client_core::model::{AdminPart, LogoState, PlaceId, ServerModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::account::ModerationNoticeKind;
use junto_client_core::protocol::audit::{AuditEvent, AuditFilterQuery};
use junto_client_core::protocol::discovery::{DailyQuiet, NotificationSettings};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::invite::CreateInvite;
use junto_client_core::protocol::message::{Block, Document, Inline};
use junto_client_core::protocol::structure::{ChannelOverride, OverrideState, SetChannelOverride};
use time::OffsetDateTime;

use crate::messages::{both_open, document, message_when, messages_when};
use crate::stream::{Pair, events, live, model_when, pair, text_channel, told_until, when};
use crate::support::{TestServer, logged_in};

/// An owner and a member on one server, each with a running event stream.
async fn scene(name: &str) -> Pair {
    let server = TestServer::start(name).await;
    let address = server.address();
    let scene = pair(server, &address).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    scene
}

fn action(name: &str) -> AuditFilterQuery {
    AuditFilterQuery {
        action: Some(name.to_owned()),
        ..AuditFilterQuery::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_removed_from_one_workspace_loses_it_and_keeps_the_server() {
    let scene = scene("Workspace removal").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let (owner_id, member_id) = (logged_in(owner), logged_in(member));
    let second = owner
        .api()
        .create_workspace("Second".to_owned())
        .await
        .unwrap();
    let lobby = text_channel(owner, second.id, "lobby").await;
    owner
        .api()
        .add_workspace_member(second.id, member_id)
        .await
        .unwrap();
    model_when(member, "the second workspace with its channel", |model| {
        model
            .workspace(second.id)
            .is_some_and(|workspace| workspace.channel(lobby.id).is_some())
    })
    .await;
    let place = PlaceId::Channel(lobby.id);
    member.live().open_messages(place).await.unwrap();
    owner.live().load_members(second.id).await.unwrap();
    model_when(owner, "the two members", |model| {
        model
            .workspace(second.id)
            .and_then(|workspace| workspace.members.loaded())
            .is_some_and(|members| members.len() == 2)
    })
    .await;

    // A member is no administrator, and an account does not remove itself.
    let refused = member
        .api()
        .remove_workspace_member(second.id, owner_id, &Statement::default())
        .await
        .unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    let own = owner
        .api()
        .remove_workspace_member(second.id, owner_id, &Statement::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            own.code(),
            Some(ErrorCode::OwnAccountTarget | ErrorCode::OwnerProtected)
        ),
        "{own}"
    );

    let mut told = member.live().notices();
    let reason = "The project of this workspace ended";
    let message = "Thank you for your work on the project.";
    owner
        .api()
        .remove_workspace_member(
            second.id,
            member_id,
            &Statement {
                reason: Some(reason.to_owned()),
                message: Some(message.to_owned()),
            },
        )
        .await
        .unwrap();

    // The member loses the workspace with its channel and its messages, and keeps its
    // session, its stream and the other workspace.
    let model = model_when(member, "the state without the second workspace", |model| {
        model.workspace(second.id).is_none()
    })
    .await;
    assert!(model.channel(lobby.id).is_none());
    assert!(model.timeline(place).is_none());
    assert!(!model.account.workspace_ids.contains(&second.id));
    // The new state holds the private notice of the removal with the message and without
    // the reason (ADR 0120).
    let notice = model
        .moderation_notices
        .iter()
        .find(|notice| notice.workspace_id == Some(second.id))
        .expect("the notice of the removal");
    assert_eq!(notice.kind, ModerationNoticeKind::WorkspaceRemoved);
    assert_eq!(notice.name, "Second");
    assert_eq!(notice.message.as_deref(), Some(message));
    let page = member.api().moderation_notices(None).await.unwrap();
    assert_eq!(page.notices.first().map(|held| held.id), Some(notice.id));
    assert!(model.workspace(scene.workspace_id).is_some());
    when(member, "the stream of the member delivers", |view| {
        view.status == StreamStatus::Live
    })
    .await;
    assert!(matches!(
        member.access().state,
        AccessState::LoggedIn { .. }
    ));
    // The state of the member is replaced after `permissions_changed`. The event of the
    // removal itself can stand after that event, inside the new state, so the member is
    // not always told it by itself. The reason is in no event.
    let notices = told_until(&mut told, "the new state", |notices| {
        notices
            .iter()
            .any(|notice| matches!(notice, Notice::Reloaded { .. }))
    })
    .await;
    let changed = events(&notices)
        .into_iter()
        .any(|(_, event)| matches!(event, Event::PermissionsChanged { .. }));
    assert!(changed, "{notices:?}");
    assert!(!format!("{notices:?}").contains("project of this workspace"));
    let refused = member.api().workspace_members(second.id).await.unwrap_err();
    assert!(refused.category().is_some());

    // The member list of the administrator follows the event.
    model_when(owner, "the member left the list", |model| {
        model
            .workspace(second.id)
            .and_then(|workspace| workspace.members.loaded())
            .is_some_and(|members| members.iter().all(|held| held.account_id != member_id))
    })
    .await;

    // The audit log of the workspace states the removal with its reason. The filters
    // are applied by the server.
    let log = owner
        .api()
        .workspace_audit_log(second.id, &action("workspace_member_removed"))
        .await
        .unwrap();
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].actor_id, Some(owner_id));
    assert_eq!(
        log.entries[0].event,
        AuditEvent::WorkspaceMemberRemoved {
            recipient_message: Some(message.to_owned()),
            account_id: member_id,
            reason: Some(reason.to_owned()),
        }
    );
    let about_member = AuditFilterQuery {
        target_id: Some(member_id.as_uuid()),
        ..AuditFilterQuery::default()
    };
    let log = owner
        .api()
        .workspace_audit_log(second.id, &about_member)
        .await
        .unwrap();
    assert!(!log.entries.is_empty());
    assert!(
        log.entries
            .iter()
            .any(|entry| matches!(entry.event, AuditEvent::WorkspaceMemberRemoved { .. }))
    );
    assert!(
        log.entries
            .iter()
            .all(|entry| !matches!(entry.event, AuditEvent::ChannelCreated { .. }))
    );
    for nothing in [
        AuditFilterQuery {
            actor_id: Some(member_id),
            ..AuditFilterQuery::default()
        },
        action("account_banned"),
        AuditFilterQuery {
            since: Some(OffsetDateTime::now_utc() + time::Duration::days(1)),
            ..AuditFilterQuery::default()
        },
        AuditFilterQuery {
            until: Some(OffsetDateTime::now_utc() - time::Duration::days(1)),
            ..action("workspace_member_removed")
        },
    ] {
        let log = owner
            .api()
            .workspace_audit_log(second.id, &nothing)
            .await
            .unwrap();
        assert!(log.entries.is_empty(), "{nothing:?}: {:?}", log.entries);
    }
    let recent = AuditFilterQuery {
        since: Some(OffsetDateTime::now_utc() - time::Duration::hours(1)),
        ..action("workspace_member_removed")
    };
    let log = owner
        .api()
        .workspace_audit_log(second.id, &recent)
        .await
        .unwrap();
    assert_eq!(log.entries.len(), 1);

    // A second removal finds no member.
    let gone = owner
        .api()
        .remove_workspace_member(second.id, member_id, &Statement::default())
        .await
        .unwrap_err();
    assert_eq!(gone.category(), Some(ErrorCategory::NotFound));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ban_with_a_reason_states_it_in_the_audit_log_of_the_server() {
    let scene = scene("Ban reason").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let member_id = logged_in(member);
    // A reason that the server does not take changes nothing.
    let long = "a".repeat(1001);
    let refused = owner
        .api()
        .ban_account_stating(member_id, &Statement::reason(&long))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest));
    assert_eq!(member.live().view().status, StreamStatus::Live);

    owner
        .api()
        .ban_account_stating(
            member_id,
            &Statement {
                reason: Some("  Advertisements in every channel ".to_owned()),
                message: Some("You can ask for an unban by mail.".to_owned()),
            },
        )
        .await
        .unwrap();
    when(member, "off", |view| view.status == StreamStatus::Off).await;
    // The banned device reads its notice with a proof of its key and without a session,
    // with the message and without the reason (ADR 0120).
    let page = member.moderation_notices().await.unwrap();
    let notice = page.notices.first().expect("the notice of the ban");
    assert_eq!(notice.kind, ModerationNoticeKind::Banned);
    assert_eq!(
        notice.message.as_deref(),
        Some("You can ask for an unban by mail.")
    );
    assert!(!format!("{page:?}").contains("Advertisements"));
    let log = owner
        .api()
        .server_audit_log(&action("account_banned"))
        .await
        .unwrap();
    assert_eq!(log.entries.len(), 1);
    assert!(matches!(
        &log.entries[0].event,
        AuditEvent::AccountBanned { reason: Some(reason), account_id, .. }
            if reason == "Advertisements in every channel" && *account_id == member_id
    ));
    owner.api().unban_account(member_id).await.unwrap();

    // A removal without a reason states none.
    owner.api().remove_account(member_id).await.unwrap();
    let log = owner
        .api()
        .server_audit_log(&action("account_removed"))
        .await
        .unwrap();
    assert_eq!(log.entries.len(), 1);
    assert!(matches!(
        &log.entries[0].event,
        AuditEvent::AccountRemoved { reason: None, account_id, .. } if *account_id == member_id
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn one_permission_of_an_override_changes_and_the_other_permissions_stay() {
    use WorkspacePermission::{LockRoom, MentionEveryone, MuteMembers};
    let scene = scene("Override change").await;
    let owner = &scene.owner;
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let everyone = owner
        .api()
        .workspace_roles(scene.workspace_id)
        .await
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap();
    let held = |model: &ServerModel| -> Option<ChannelOverride> {
        let overrides = model.admin.overrides(general.id).loaded()?;
        overrides
            .iter()
            .find(|held| held.role_id == everyone.id)
            .cloned()
    };
    model_when(owner, "the state holds the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    owner
        .live()
        .open_admin(AdminPart::Overrides(general.id))
        .await
        .unwrap();
    owner
        .api()
        .set_channel_override(
            general.id,
            everyone.id,
            &SetChannelOverride {
                allow: vec![MentionEveryone],
                deny: vec![LockRoom],
            },
        )
        .await
        .unwrap();

    // One request names one permission; the two others stay as they are.
    let changed = owner
        .api()
        .change_channel_override(general.id, everyone.id, MuteMembers, OverrideState::Deny)
        .await
        .unwrap();
    assert_eq!(changed.allow, [MentionEveryone]);
    let mut denied = changed.deny.clone();
    denied.sort();
    let mut expected = vec![LockRoom, MuteMembers];
    expected.sort();
    assert_eq!(denied, expected);
    model_when(owner, "the collection follows the event", |model| {
        held(model).is_some_and(|held| held.deny.contains(&MuteMembers))
    })
    .await;

    let changed = owner
        .api()
        .change_channel_override(
            general.id,
            everyone.id,
            MentionEveryone,
            OverrideState::Inherit,
        )
        .await
        .unwrap();
    assert!(changed.allow.is_empty());
    assert_eq!(changed.deny.len(), 2);
    let changed = owner
        .api()
        .change_channel_override(general.id, everyone.id, MuteMembers, OverrideState::Allow)
        .await
        .unwrap();
    assert_eq!(changed.allow, [MuteMembers]);
    assert_eq!(changed.deny, [LockRoom]);

    // The last permissions that leave the override remove it.
    for permission in [MuteMembers, LockRoom] {
        owner
            .api()
            .change_channel_override(general.id, everyone.id, permission, OverrideState::Inherit)
            .await
            .unwrap();
    }
    model_when(owner, "no override of the role is left", |model| {
        model.admin.overrides(general.id).loaded().is_some() && held(model).is_none()
    })
    .await;
    assert!(
        owner
            .api()
            .channel_overrides(general.id)
            .await
            .unwrap()
            .is_empty()
    );

    // A member holds no "manage channel".
    let refused = scene
        .member
        .api()
        .change_channel_override(general.id, everyone.id, MuteMembers, OverrideState::Deny)
        .await
        .unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invite_names_its_creator_and_loses_a_workspace_with_the_removal_of_its_creator() {
    let scene = scene("Invite creator").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let member_id = logged_in(member);
    let second = owner
        .api()
        .create_workspace("Second".to_owned())
        .await
        .unwrap();
    owner
        .api()
        .add_workspace_member(second.id, member_id)
        .await
        .unwrap();
    // The member may create invites with a role of the server.
    let role = owner
        .api()
        .create_server_role(&junto_client_core::protocol::roles::CreateServerRole {
            name: "Hosts".to_owned(),
            permissions: vec![junto_client_core::protocol::ServerPermission::CreateInvites],
        })
        .await
        .unwrap();
    owner
        .api()
        .assign_server_role(member_id, role.id)
        .await
        .unwrap();
    model_when(member, "the member may create invites", |model| {
        model
            .account
            .server_permissions
            .contains(&junto_client_core::protocol::ServerPermission::CreateInvites)
            && model.workspace(second.id).is_some()
    })
    .await;
    let created = member
        .api()
        .create_invite(&CreateInvite {
            workspace_ids: vec![scene.workspace_id, second.id],
            ..CreateInvite::default()
        })
        .await
        .unwrap();
    let invite_id = created.invite.id;

    owner.live().open_admin(AdminPart::Invites).await.unwrap();
    let model = model_when(owner, "the invite of the member", |model| {
        model
            .admin
            .invites
            .loaded()
            .is_some_and(|invites| invites.iter().any(|held| held.id == invite_id))
    })
    .await;
    let invites = model.admin.invites.loaded().unwrap();
    for held in invites {
        let name = if held.created_by == member_id {
            "Grace"
        } else {
            "Ada"
        };
        assert_eq!(held.created_by_name.as_deref(), Some(name));
    }

    // The removal of the creator from one workspace takes that workspace from its
    // invite; the list of the administrator follows `invite_changed`.
    owner
        .api()
        .remove_workspace_member(second.id, member_id, &Statement::default())
        .await
        .unwrap();
    model_when(owner, "the invite lost the workspace", |model| {
        model.admin.invites.loaded().is_some_and(|invites| {
            invites.iter().any(|held| {
                held.id == invite_id
                    && held.workspace_ids == [scene.workspace_id]
                    && held.revocation.is_none()
            })
        })
    })
    .await;
    let log = owner
        .api()
        .server_audit_log(&action("invite_workspaces_changed"))
        .await
        .unwrap();
    assert_eq!(log.entries.len(), 1);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::InviteWorkspacesChanged {
            invite_id,
            invite_created_by: member_id,
            removed_workspace_id: second.id,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_logo_of_the_server_is_set_read_without_a_session_and_told_to_a_member() {
    let scene = scene("Logo").await;
    let (owner, member) = (&scene.owner, &scene.member);
    assert_eq!(owner.api().server_logo().await.unwrap(), None);
    assert_eq!(member.logo().await.unwrap(), None);
    let model = member.live().view().model.unwrap();
    assert_eq!(model.logo, LogoState::NotStated);

    // Only an administrator sets the logo.
    let image = super::png(64, 64, 32);
    let refused = member.api().set_server_logo(image.clone()).await;
    assert_eq!(
        refused.unwrap_err().category(),
        Some(ErrorCategory::NotAllowed)
    );
    let logo = owner.api().set_server_logo(image.clone()).await.unwrap();
    assert_eq!((logo.width, logo.height), (64, 64));
    assert_eq!(logo.bytes as usize, image.len());

    // The member is told the description, and reads the image without its session.
    let model = model_when(member, "the logo is stated", |model| {
        model.logo == LogoState::Stated(Some(logo.clone()))
    })
    .await;
    assert!(model.workspace(scene.workspace_id).is_some());
    let read = member.logo().await.unwrap().unwrap();
    assert_eq!(read.bytes, image);
    assert_eq!(read.hash, logo.hash);
    assert_eq!(
        member.api().server_logo().await.unwrap().unwrap().hash,
        logo.hash
    );

    // The same image again records nothing: the audit log holds one entry.
    owner.api().set_server_logo(image).await.unwrap();
    let log = owner
        .api()
        .server_audit_log(&action("server_logo_changed"))
        .await
        .unwrap();
    assert_eq!(log.entries.len(), 1);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::ServerLogoChanged {
            logo: Some(logo.clone())
        }
    );

    // The public information of the server names the path, so a device that was not
    // connected reads the logo at its next contact.
    let later = scene.second.restarted();
    let handle = later.server(member.id()).unwrap();
    handle.connect().await.unwrap();
    assert!(handle.entry().logo_path.is_some());
    assert_eq!(handle.logo().await.unwrap().unwrap().hash, logo.hash);

    owner.api().remove_server_logo().await.unwrap();
    model_when(member, "the removal is stated", |model| {
        model.logo == LogoState::Stated(None)
    })
    .await;
    assert_eq!(member.logo().await.unwrap(), None);
    let refused = member.api().remove_server_logo().await.unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_result_states_its_position_and_the_messages_around_it_are_read() {
    let scene = scene("Search position").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    both_open(&scene, place).await;
    owner
        .live()
        .send_message(place, document("the obsidian plan"), None)
        .unwrap();
    let wanted = message_when(member, place, "the obsidian plan").await;
    // More messages than one page holds follow the wanted one. The client sends them one
    // after the other, so the wait bounds each of them and not their sum.
    let later: Vec<String> = (1..=70).map(|number| format!("later {number}")).collect();
    for text in &later {
        owner
            .live()
            .send_message(place, document(text), None)
            .unwrap();
    }
    messages_when(owner, place, &later).await;

    // A client that opens the channel now holds its end, without the wanted message.
    member.live().release_messages(place);
    model_when(member, "the channel is closed", |model| {
        model.timeline(place).is_none()
    })
    .await;
    member.live().open_messages(place).await.unwrap();
    let model = model_when(member, "the end of the channel", |model| {
        model
            .timeline(place)
            .is_some_and(|timeline| timeline.loaded && timeline.message(wanted.id).is_none())
    })
    .await;
    assert!(model.timeline(place).unwrap().more_before);

    // The result names the message, its place and its position.
    let hits = member
        .api()
        .search(&SearchQuery {
            text: "obsidian".to_owned(),
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(hits.results.len(), 1);
    let hit = &hits.results[0];
    assert_eq!(hit.message_id, Some(wanted.id));
    assert_eq!(hit.place_id, general.id.as_uuid());
    assert_eq!(hit.position, Some(wanted.position));
    assert_eq!(hit.thread_root, None);
    assert_eq!(hit.start_ms, None);

    // The jump: the messages around the position replace what is loaded.
    member
        .live()
        .read_messages_around(place, hit.position.unwrap());
    let model = model_when(member, "the messages around the result", |model| {
        model
            .timeline(place)
            .is_some_and(|timeline| timeline.message(wanted.id).is_some())
    })
    .await;
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.messages.iter().any(|held| held.id == wanted.id));
    assert!(!timeline.more_before);
}

/// A text that names an account.
fn mention(text: &str, account_id: junto_client_core::protocol::ids::AccountId) -> Document {
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

/// The settings of an account with this daily quiet time and nothing else.
fn quiet(daily_quiet: Option<DailyQuiet>) -> NotificationSettings {
    NotificationSettings {
        level: None,
        mute: false,
        muted_until: None,
        role_mentions: None,
        everyone_mentions: None,
        do_not_disturb: false,
        daily_quiet,
        hide_text: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_time_in_a_named_zone_holds_the_event_of_a_notice_back() {
    let scene = scene("Quiet zone").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let member_id = logged_in(member);
    let general = text_channel(owner, scene.workspace_id, "general").await;
    let place = PlaceId::Channel(general.id);
    both_open(&scene, place).await;

    // `Etc/GMT+3` is three hours behind UTC through the whole year. A window of two
    // hours around the present time on its clock does not hold the present time on the
    // clock of UTC, so the test fails for a server that reads the window in UTC.
    let zone = "Etc/GMT+3";
    let now = OffsetDateTime::now_utc()
        .checked_to_offset(time::UtcOffset::from_hms(-3, 0, 0).unwrap())
        .unwrap();
    let minute = u16::from(now.hour()) * 60 + u16::from(now.minute());
    let around = DailyQuiet {
        start_minute: (minute + 1440 - 60) % 1440,
        end_minute: (minute + 60) % 1440,
        time_zone: zone.to_owned(),
    };
    let stored = member
        .api()
        .set_notification_settings(SettingsScope::Account, &quiet(Some(around.clone())))
        .await
        .unwrap()
        .value;
    assert_eq!(stored.daily_quiet, Some(around.clone()));
    model_when(member, "the state holds the quiet time", |model| {
        model
            .notification_settings(SettingsScope::Account)
            .is_some_and(|settings| settings.daily_quiet.as_ref() == Some(&around))
    })
    .await;
    // The rule of the client agrees with the server when the caller gives the clock of
    // the zone, and a zone that the caller does not know gives no quiet time.
    let model = member.live().view().model.unwrap();
    assert!(model.is_quiet(|name| (name == zone).then_some(minute)));
    assert!(!model.is_quiet(|_| None));

    let mut told = member.live().notices();
    owner
        .live()
        .send_message(place, mention("during the quiet time ", member_id), None)
        .unwrap();
    message_when(member, place, "during the quiet time ").await;

    // The quiet time ends: the next mention is told with its notice.
    member
        .api()
        .set_notification_settings(SettingsScope::Account, &quiet(None))
        .await
        .unwrap();
    model_when(member, "the state holds no quiet time", |model| {
        model
            .notification_settings(SettingsScope::Account)
            .is_none_or(|settings| settings.daily_quiet.is_none())
    })
    .await;
    owner
        .live()
        .send_message(place, mention("after the quiet time ", member_id), None)
        .unwrap();
    let after = message_when(member, place, "after the quiet time ").await;
    let alerts = |notices: &[Notice]| -> Vec<junto_client_core::protocol::ids::MessageId> {
        events(notices)
            .into_iter()
            .filter_map(|(_, event)| match event {
                Event::MessageMentioned { message_id, .. } => Some(*message_id),
                _ => None,
            })
            .collect()
    };
    let notices = told_until(&mut told, "the mention after the quiet time", |notices| {
        alerts(notices).contains(&after.id)
    })
    .await;
    // The events of one account arrive in their order: the mention of the quiet time
    // would stand before this one, and it was not delivered.
    assert_eq!(alerts(&notices), [after.id]);

    // A name that is no zone of the database is refused.
    let refused = member
        .api()
        .set_notification_settings(
            SettingsScope::Account,
            &quiet(Some(DailyQuiet {
                time_zone: "XYZ3".to_owned(),
                ..around
            })),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest));
}
