//! The administration after the remaining requests of the client, in the window against
//! the real server: the removal of a member from one workspace with its reason and its
//! rank rule (ADR 0106, ADR 0309), the notification level of a workspace that is not set
//! (ADR 0412), and a quiet time in a named time zone (ADR 0412, ADR 0798).

use gpui::TestAppContext;
use junto_client::state::admin::view::notifications::{DefaultChoice, NotificationsPage};
use junto_client::state::admin::view::people::{MemberLine, MemberList, NoRemoval};
use junto_client::state::admin::view::records::{AuditPage, AuditRead};
use junto_client::state::admin::view::{Scope, ServerPage};
use junto_client::state::admin::{AskKind, AuditScope};
use junto_client::state::view::ServerStatus;
use junto_client::zones;
use junto_client_core::ServerHandle;
use junto_client_core::api::{ErrorCategory, SettingsScope};
use junto_client_core::live::Notice;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ServerPermission;
use junto_client_core::protocol::discovery::{DailyQuiet, NotificationLevel, NotificationSettings};
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, MessageId, WorkspaceId};
use junto_client_core::protocol::message::{Block, Document, Inline, SendMessage};
use junto_client_core::protocol::roles::CreateServerRole;
use time::OffsetDateTime;

use crate::admin_pages::{
    awaited, click_in_page, default_workspace, idle, invite, joined, open_page, owner_window, sel,
    text_channel, try_click, until_model,
};
use crate::harness::{PATIENCE, Window, World, hold_data_directory, member, named};
use crate::support::logged_in;

/// The client of the server of a window.
fn handle_of(window: &mut Window) -> ServerHandle {
    window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0)
}

fn members_page(window: &mut Window) -> Option<Vec<MemberLine>> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Members(page),
            ..
        }) => match page.members {
            MemberList::Loaded(rows) => Some(rows),
            MemberList::Loading | MemberList::Failed => None,
        },
        _ => None,
    }
}

fn member_row(window: &mut Window, account_id: AccountId) -> Option<MemberLine> {
    members_page(window)?
        .into_iter()
        .find(|row| row.account_id == account_id)
}

fn audit_page(window: &mut Window) -> Option<AuditPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::AuditLog(page),
            ..
        }) => Some(*page),
        _ => None,
    }
}

fn notifications_page(window: &mut Window) -> Option<NotificationsPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Notifications(page),
            ..
        }) => Some(page),
        _ => None,
    }
}

/// The names of the workspaces that the server book of a window lists for its server.
fn workspace_names(window: &mut Window) -> Vec<String> {
    window
        .view()
        .servers
        .first()
        .map(|server| {
            server
                .workspaces
                .iter()
                .map(|workspace| workspace.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Clicks an action of a row until its question is drawn. After a reload of the state
/// the page reads the accounts and the order of the roles again, and for that moment
/// the action is drawn as held and takes no click.
#[track_caller]
fn ask(window: &mut Window, action: &'static str, question: &'static str) {
    window.until(&format!("{action} asks first"), |window| {
        if window.shows(question) {
            return true;
        }
        try_click(window, action);
        false
    });
}

/// Shows a workspace of the server of the window, as a click on its row does.
fn show_workspace(window: &mut Window, workspace_id: WorkspaceId) {
    let server_id = handle_of(window).id();
    window.state.update(&mut window.cx, |state, cx| {
        state.select_workspace(server_id, workspace_id, cx);
    });
    window.until("the window shows the workspace", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.id == workspace_id)
    });
}

#[gpui::test]
fn an_administrator_removes_a_member_from_one_workspace_and_the_member_keeps_the_server(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_ada_directory, mut window, owner) = owner_window(cx, &server);
    let studio = default_workspace(&world, &owner);
    let lab = world
        .run(owner.api().create_workspace("Lab".to_owned()))
        .unwrap()
        .id;
    text_channel(&world, &owner, lab, "bench");
    let code = invite(&world, &owner, Vec::new());
    let (_carla_directory, mut other) = named(cx, "Carla");
    other.find_server(&server.address());
    other.join_with_code(code.expose());
    other.until_ready();
    let carla = logged_in(&handle_of(&mut other));
    world
        .run(owner.api().add_workspace_member(lab, carla))
        .unwrap();
    other.until("the window of the member lists both workspaces", |window| {
        let names = workspace_names(window);
        names.len() == 2 && names.contains(&"Lab".to_owned())
    });
    show_workspace(&mut other, lab);
    assert!(other.shows_channel("bench"));

    // The members of "Lab" in the window of the administrator.
    show_workspace(&mut window, lab);
    window.open_administration();
    open_page(&mut window, "admin-nav-members", |window| {
        member_row(window, carla).is_some_and(|row| row.remove == Some(Ok(())))
    });

    // The removal asks first, and takes a reason.
    ask(
        &mut window,
        sel("member-remove", carla),
        "member-remove-confirm",
    );
    assert!(window.shows("moderation-reason-field"));
    // A reason that is too long holds the confirmation.
    let long = "a".repeat(1001);
    window.in_shell(|shell, window, cx| shell.type_reason(&long, window, cx));
    window.until("the page states that the reason is too long", |window| {
        window.shows("moderation-reason-invalid")
    });
    click_in_page(&mut window, "member-remove-confirm");
    assert!(member_row(&mut window, carla).is_some());
    assert!(idle(&mut window));
    let reason = "The project of this workspace ended";
    let message = "Thank you for your work on the project.";
    window.in_shell(|shell, window, cx| shell.type_reason(reason, window, cx));
    window.in_shell(|shell, window, cx| shell.type_moderation_message(message, window, cx));
    window.until("the reason can be sent", |window| {
        !window.shows("moderation-reason-invalid")
    });
    click_in_page(&mut window, "member-remove-confirm");
    window.until("the member left the list", |window| {
        members_page(window).is_some_and(|rows| rows.iter().all(|row| row.account_id != carla))
            && idle(window)
    });
    assert!(!window.shows("member-remove-confirm"));

    // The window of the member loses the workspace and keeps the server: the other
    // workspace is shown, and the server is ready.
    other.until("the window of the member lost the workspace", |window| {
        let names = workspace_names(window);
        names.len() == 1 && !names.contains(&"Lab".to_owned())
    });
    assert_eq!(other.status(), Some(ServerStatus::Ready));
    other.until("the member sees the default workspace", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.id == studio)
    });
    assert!(!other.shows_channel("bench"));
    let still = world.run(handle_of(&mut other).api().workspaces());
    assert!(still.is_ok());

    // The window of the member states the removal with the message, and not the reason,
    // until the member puts the notice away (ADR 0120).
    let notice = other.until_value("the notice of the removal", |window| {
        window.view().selected?.moderation.first().cloned()
    });
    assert_eq!(notice.name, "Lab");
    assert_eq!(notice.message.as_deref(), Some(message));
    assert!(notice.dismissable);
    let text = junto_client::text::moderation_notice(&notice);
    assert!(text.contains(message) && !text.contains(reason), "{text}");
    // Another program holds the lock of the data directory: the click returns without
    // waiting for it, and the book is written when the lock is free.
    other.settle();
    let held = hold_data_directory(&other);
    other.press_as_drawn("moderation-seen");
    held.release();
    other.until("the notice is put away", |window| {
        window
            .view()
            .selected
            .is_some_and(|selected| selected.moderation.is_empty())
    });
    // The server book keeps that it was put away.
    assert_eq!(
        handle_of(&mut other).moderation_notices_shown(),
        [notice.id]
    );

    // The audit log of the workspace states the removal with its reason.
    open_page(&mut window, "admin-nav-audit", |window| {
        audit_page(window).is_some()
    });
    window.until("the log of the workspace is chosen", |window| {
        let Some(page) = audit_page(window) else {
            return false;
        };
        if page.scope == Some(AuditScope::Workspace(lab)) {
            return page.state == AuditRead::Loaded;
        }
        window.click_if_shown("audit-scope-1");
        false
    });
    let removal = awaited(&mut window, "the entry of the removal", |window| {
        audit_page(window)?
            .rows
            .into_iter()
            .find(|row| row.sentence.contains("removed") && row.sentence.contains("Carla"))
    });
    assert_eq!(removal.reason.as_deref(), Some(reason));
    assert_eq!(removal.message.as_deref(), Some(message));
    click_in_page(&mut window, sel("audit-row", removal.id));
    assert!(window.shows(sel("audit-reason", removal.id)));
    assert!(window.shows(sel("audit-message", removal.id)));
}

#[gpui::test]
fn an_administrator_sees_why_a_member_above_cannot_be_removed_from_a_workspace_before_trying(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let ada = world.owner(&server);
    let studio = default_workspace(&world, &ada.handle);
    let role = |name: &str| {
        world
            .run(ada.handle.api().create_server_role(&CreateServerRole {
                name: name.to_owned(),
                permissions: vec![ServerPermission::Administrator],
            }))
            .unwrap()
            .id
    };
    // "Chiefs" was created first, so it stands above "Stewards".
    let (chiefs, stewards) = (role("Chiefs"), role("Stewards"));
    let (_bruno_directory, mut window) = member(cx, &world, &server.address(), &ada, "Bruno");
    let bruno = logged_in(&handle_of(&mut window));
    let code = invite(&world, &ada.handle, Vec::new());
    let (_dora_device, dora_handle) = joined(&world, &server, &code, "Dora");
    let dora = logged_in(&dora_handle);
    let code = invite(&world, &ada.handle, Vec::new());
    let (_elio_device, elio_handle) = joined(&world, &server, &code, "Elio");
    let elio = logged_in(&elio_handle);
    for (account_id, role_id) in [(bruno, stewards), (dora, chiefs)] {
        world
            .run(ada.handle.api().assign_server_role(account_id, role_id))
            .unwrap();
    }

    window.until("the window of the administrator has the page", |window| {
        window.view().selected.is_some()
    });
    window.open_administration();
    open_page(&mut window, "admin-nav-members", |window| {
        member_row(window, dora).is_some_and(|row| row.remove == Some(Err(NoRemoval::Outranked)))
    });
    // The member above states the rule in its row, and its action takes no click.
    assert!(window.shows(sel("member-outranked", dora)));
    click_in_page(&mut window, sel("member-remove", dora));
    assert!(!window.shows("member-remove-confirm"));
    // The server decides the same for a request that is sent all the same.
    let api = handle_of(&mut window).api();
    let refused = world
        .run(api.remove_workspace_member(
            studio,
            dora,
            &junto_client_core::api::Statement::default(),
        ))
        .unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    // The owner and the own account.
    let own = member_row(&mut window, bruno).unwrap();
    assert_eq!(own.remove, Some(Err(NoRemoval::OwnAccount)));
    let owner = member_row(&mut window, logged_in(&ada.handle)).unwrap();
    assert_eq!(owner.remove, Some(Err(NoRemoval::Owner)));

    // A member below is removed.
    let below = member_row(&mut window, elio).unwrap();
    assert_eq!(below.remove, Some(Ok(())));
    assert!(!window.shows(sel("member-outranked", elio)));
    ask(
        &mut window,
        sel("member-remove", elio),
        "member-remove-confirm",
    );
    click_in_page(&mut window, "member-remove-confirm");
    window.until("the member below left the list", |window| {
        member_row(window, elio).is_none() && idle(window)
    });
    until_model(
        &world,
        &elio_handle,
        "the member lost the workspace",
        |model| model.workspace(studio).is_none(),
    );

    // The member above loses its role: the row offers the removal without a new read
    // by the person.
    world
        .run(ada.handle.api().unassign_server_role(dora, chiefs))
        .unwrap();
    window.until("the removal of the member is offered", |window| {
        member_row(window, dora).is_some_and(|row| row.remove == Some(Ok(())))
            && !window.shows(sel("member-outranked", dora))
    });
}

#[gpui::test]
fn an_administrator_sets_the_notification_level_of_a_workspace_and_removes_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_ada_directory, mut window, owner) = owner_window(cx, &server);
    let studio = default_workspace(&world, &owner);
    let code = invite(&world, &owner, Vec::new());
    let (_bruno_device, bruno) = joined(&world, &server, &code, "Bruno");
    let level = |handle: &ServerHandle| {
        world
            .run(handle.api().notification_default(studio))
            .unwrap()
    };

    window.open_administration();
    open_page(&mut window, "admin-nav-notifications", |window| {
        notifications_page(window).is_some()
    });
    // A workspace sets no level until an administrator stores one.
    let page = notifications_page(&mut window).unwrap();
    assert_eq!(page.choice, DefaultChoice::NotSet);
    assert_eq!(level(&owner), None);
    assert!(window.shows("notification-default-stated"));

    // "All messages" is the second choice of the control.
    click_in_page(&mut window, "notification-default-1");
    // The reply can end the request before the state holds the event of the change. Until
    // the event is applied, the control shows the sent level and takes no choice, so the
    // next click waits for the page to take a choice again.
    window.until(
        "the page shows the stored level and takes a choice",
        |window| {
            notifications_page(window).is_some_and(|page| {
                page.choice == DefaultChoice::Level(NotificationLevel::All) && !page.changing
            }) && idle(window)
        },
    );
    assert_eq!(level(&owner), Some(NotificationLevel::All));
    until_model(&world, &bruno, "the member holds the level", |model| {
        model
            .workspace(studio)
            .is_some_and(|workspace| workspace.notification_default == Some(NotificationLevel::All))
    });

    // "Not set" removes the level: it is not the level "mentions only".
    click_in_page(&mut window, "notification-default-0");
    window.until("the page shows that no level is set", |window| {
        notifications_page(window).is_some_and(|page| page.choice == DefaultChoice::NotSet)
            && idle(window)
    });
    assert_eq!(level(&owner), None);
    until_model(&world, &bruno, "the member holds no level", |model| {
        model
            .workspace(studio)
            .is_some_and(|workspace| workspace.notification_default.is_none())
    });
    assert!(
        window
            .admin()
            .is_some_and(|view| view.refused(AskKind::NotificationDefault(studio)).is_none())
    );

    // The server gives the change to administrators only.
    let refused = world.run(bruno.api().remove_notification_default(studio));
    assert_eq!(
        refused.unwrap_err().category(),
        Some(ErrorCategory::NotAllowed)
    );
}

/// A text that names an account.
fn mention(text: &str, account_id: AccountId) -> SendMessage {
    SendMessage {
        document: Document {
            blocks: vec![Block::Paragraph {
                content: vec![
                    Inline::Text {
                        text: text.to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Account { account_id },
                ],
            }],
        },
        reply_to: None,
        attachments: Vec::new(),
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

/// The messages that the events of mentions name, out of what a reader of notices was
/// told until the mention of `last` arrived.
fn mentions_until(
    world: &World,
    told: &mut junto_client_core::live::Notices,
    last: MessageId,
) -> Vec<MessageId> {
    world.run(async {
        let mut found = Vec::new();
        let reading = async {
            while !found.contains(&last) {
                match told.next().await {
                    Some(Ok(Notice::Event { event, .. })) => {
                        if let Event::MessageMentioned { message_id, .. } = event.as_ref() {
                            found.push(*message_id);
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        };
        let ended = tokio::time::timeout(PATIENCE, reading).await.is_ok();
        assert!(ended, "the mention after the quiet time was not told");
        found
    })
}

/// The clock of each named zone, as this build computes it, is the clock by which the
/// server holds a notice back. The zones have a whole hour, a half hour and a quarter of
/// an hour from UTC, and one of them is the zone of the system.
#[gpui::test]
fn a_quiet_time_in_a_named_zone_holds_a_notice_back_by_the_clock_that_the_client_computes(
    _cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let ada = world.owner(&server);
    let studio = default_workspace(&world, &ada.handle);
    let general = text_channel(&world, &ada.handle, studio, "general");
    let code = invite(&world, &ada.handle, Vec::new());
    let (_bruno_device, bruno) = joined(&world, &server, &code, "Bruno");
    let bruno_id = logged_in(&bruno);
    let place = PlaceId::Channel(general);
    until_model(&world, &bruno, "the member sees the channel", |model| {
        model.channel(general).is_some()
    });

    #[allow(clippy::disallowed_methods)]
    let system = zones::system();
    let mut told = bruno.live().notices();
    for zone in [
        system.as_str(),
        "America/Sao_Paulo",
        "Asia/Kolkata",
        "Asia/Kathmandu",
        "Europe/Lisbon",
    ] {
        // A window of twenty minutes around the present minute on the clock of the zone.
        let minute = zones::minute_in(zone, OffsetDateTime::now_utc()).unwrap();
        let around = DailyQuiet {
            start_minute: (minute + 1440 - 10) % 1440,
            end_minute: (minute + 10) % 1440,
            time_zone: zone.to_owned(),
        };
        let stored =
            world
                .run(bruno.api().set_notification_settings(
                    SettingsScope::Account,
                    &quiet(Some(around.clone())),
                ))
                .unwrap()
                .value;
        assert_eq!(stored.daily_quiet.as_ref(), Some(&around), "{zone}");
        until_model(&world, &bruno, "the state holds the quiet time", |model| {
            model
                .notification_settings(SettingsScope::Account)
                .is_some_and(|settings| settings.daily_quiet.as_ref() == Some(&around))
        });
        // The rule of the client with the clock of this build agrees with the server.
        let model = bruno.live().view().model.unwrap();
        assert!(
            model.is_quiet(|name| zones::minute_in(name, OffsetDateTime::now_utc())),
            "{zone}"
        );
        let held_back = world
            .run(ada.handle.api().send_message(
                place,
                &mention(&format!("during the quiet time in {zone} "), bruno_id),
            ))
            .unwrap();

        // A window that ended ten minutes ago on the same clock is no quiet time.
        let before = DailyQuiet {
            start_minute: (minute + 1440 - 30) % 1440,
            end_minute: (minute + 1440 - 10) % 1440,
            time_zone: zone.to_owned(),
        };
        world
            .run(
                bruno.api().set_notification_settings(
                    SettingsScope::Account,
                    &quiet(Some(before.clone())),
                ),
            )
            .unwrap();
        until_model(
            &world,
            &bruno,
            "the state holds the earlier window",
            |model| {
                model
                    .notification_settings(SettingsScope::Account)
                    .is_some_and(|settings| settings.daily_quiet.as_ref() == Some(&before))
            },
        );
        let model = bruno.live().view().model.unwrap();
        assert!(
            !model.is_quiet(|name| zones::minute_in(name, OffsetDateTime::now_utc())),
            "{zone}"
        );
        let told_of = world
            .run(ada.handle.api().send_message(
                place,
                &mention(&format!("after the quiet time in {zone} "), bruno_id),
            ))
            .unwrap();
        // The events of one account arrive in their order: a mention of the quiet time
        // would stand before this one.
        let mentions = mentions_until(&world, &mut told, told_of.id);
        assert_eq!(mentions, [told_of.id], "{zone}");
        assert!(!mentions.contains(&held_back.id));
    }
}
