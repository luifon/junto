//! The pages of the administration in the window against the real server (ADR 0761): the
//! people of a server, the members of a workspace, invites, join requests, the permissions
//! of a channel, access tokens, exports and the audit log.
//!
//! A change of the permissions of an account makes its client read the state of the server
//! again, and a list of a page is on its way for a moment then. A test therefore reads what
//! a page shows in a wait, and clicks an element when it is drawn.

use std::sync::Arc;

use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use junto_client::state::admin::view::overrides::{Choice, PermissionLine, Refused, RoleLine};
use junto_client::state::admin::view::people::{
    AccountLine, AddableLine, Blocked, EXPIRY_DAYS, InviteLine, InviteState, MemberList,
    RequestLine, USES,
};
use junto_client::state::admin::view::records::{
    AUDIT_PAGE, About, Actor, AuditGroup, AuditRead, ExportLine, ExportSave, ExportStatus,
    TOKEN_EXPIRY_DAYS, TokenLine,
};
use junto_client::state::admin::view::{ChannelPageView, Listed, Scope, ServerPage};
use junto_client::state::admin::{AuditFilter, AuditPeriod, AuditScope, Made};
use junto_client::state::view::Members;
use junto_client::text::admin::records::{audit_head, token_action_selector};
use junto_client_core::ServerHandle;
use junto_client_core::address::ServerAddress;
use junto_client_core::api::{
    ApiClient, ErrorCategory, ReqwestTransport, RetryPolicy, ServerApi, server_info,
};
use junto_client_core::flow::access::{AccessAction, AccessState, Notice};
use junto_client_core::live::{LiveView, StreamStatus};
use junto_client_core::model::{AdminPart, PlaceId, ServerModel};
use junto_client_core::protocol::access_token::TokenAction;
use junto_client_core::protocol::account::AccountStatus;
use junto_client_core::protocol::audit::{AuditEvent, AuditFilterQuery};
use junto_client_core::protocol::ids::{
    AccessTokenId, AccountId, ChannelId, ExportId, InviteId, JoinRequestId, WorkspaceId,
};
use junto_client_core::protocol::invite::CreateInvite;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::roles::{CreateServerRole, CreateWorkspaceRole};
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::protocol::structure::{
    ChannelKind, CreateCategory, CreateChannel, UpdateChannel,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use junto_client_core::secret::SecretToken;
use junto_client_core::version::SupportedVersions;
use time::{Duration, OffsetDateTime};

use crate::harness::{PATIENCE, Window, World, member, named};
use crate::messages::document;
use crate::support::{Device, TestServer, join, logged_in};

/// The open page of the administration of the server, when it is the named one.
macro_rules! server_page {
    ($window:expr, $page:ident) => {
        match $window.admin().map(|view| view.scope) {
            Some(Scope::Server {
                page: ServerPage::$page(page),
                ..
            }) => Some(page),
            _ => None,
        }
    };
}

/// The name of the element of one row. gpui finds a drawn element by a name that lives as
/// long as the program, and a test builds few names.
pub(crate) fn sel(prefix: &str, of: impl std::fmt::Display) -> &'static str {
    Box::leak(format!("{prefix}-{of}").into_boxed_str())
}

/// Opens the application on a new data directory and claims the server with its owner
/// code. Returns the window, and the client of the window for the requests that the owner
/// makes through the API.
pub(crate) fn owner_window(
    cx: &mut TestAppContext,
    server: &TestServer,
) -> (tempfile::TempDir, Window, ServerHandle) {
    let (directory, mut window) = named(cx, "Ada");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    let owner = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    (directory, window, owner)
}

/// An invite for one use. No workspace: the default workspace.
pub(crate) fn invite(
    world: &World,
    inviter: &ServerHandle,
    workspace_ids: Vec<WorkspaceId>,
) -> InviteCode {
    let invite = CreateInvite {
        workspace_ids,
        ..CreateInvite::default()
    };
    world
        .run(inviter.api().create_invite(&invite))
        .unwrap()
        .code
}

/// A device without a window that joins the server with an invite code. Its event stream
/// runs, so its state follows the server.
pub(crate) fn joined(
    world: &World,
    server: &TestServer,
    code: &InviteCode,
    name: &str,
) -> (Device, ServerHandle) {
    let device = Device::new();
    let handle = world.run(async {
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(code, name)).await.unwrap();
        logged_in(&handle);
        device.client.start_event_streams().unwrap();
        device.client.select(Some(handle.id()));
        handle
    });
    until_model(world, &handle, "the first state", |_| true);
    (device, handle)
}

/// A device without an account that asks to join the server. Returns the device, its
/// client of the server, and the join request.
fn asks_to_join(
    world: &World,
    server: &TestServer,
    name: &str,
) -> (Device, ServerHandle, JoinRequestId) {
    let device = Device::new();
    let handle = world.run(async {
        let handle = device.add(&server.address()).await;
        let request = AccessAction::RequestAccess {
            name: name.to_owned(),
        };
        handle.dispatch(request).await.unwrap();
        handle
    });
    let AccessState::WaitingForApproval {
        join_request_id: Some(request_id),
        ..
    } = handle.access().state
    else {
        panic!("the device does not wait for an approval");
    };
    (device, handle, request_id)
}

/// Gives an account a new server role with one permission, as the owner does.
fn grant_server(
    world: &World,
    owner: &ServerHandle,
    account_id: AccountId,
    permission: ServerPermission,
) {
    let role = CreateServerRole {
        name: "Helpers".to_owned(),
        permissions: vec![permission],
    };
    let api = owner.api();
    let role = world.run(api.create_server_role(&role)).unwrap();
    world
        .run(api.assign_server_role(account_id, role.id))
        .unwrap();
}

/// Gives a member a new role of a workspace with one permission, as the owner does.
fn grant_workspace(
    world: &World,
    owner: &ServerHandle,
    workspace_id: WorkspaceId,
    account_id: AccountId,
    permission: WorkspacePermission,
) {
    let role = CreateWorkspaceRole {
        name: "Leads".to_owned(),
        permissions: vec![permission],
    };
    let api = owner.api();
    let role = world
        .run(api.create_workspace_role(workspace_id, &role))
        .unwrap();
    world
        .run(api.assign_workspace_role(workspace_id, account_id, role.id))
        .unwrap();
}

pub(crate) fn default_workspace(world: &World, handle: &ServerHandle) -> WorkspaceId {
    world
        .run(handle.api().workspaces())
        .unwrap()
        .into_iter()
        .find(|workspace| workspace.is_default)
        .unwrap()
        .id
}

pub(crate) fn text_channel(
    world: &World,
    handle: &ServerHandle,
    workspace_id: WorkspaceId,
    name: &str,
) -> ChannelId {
    let channel = CreateChannel {
        name: name.to_owned(),
        kind: ChannelKind::Text,
        category_id: None,
        position: None,
    };
    world
        .run(handle.api().create_channel(workspace_id, &channel))
        .unwrap()
        .id
}

/// Waits until the event stream of a client without a window is in a state.
pub(crate) fn until_stream(
    world: &World,
    handle: &ServerHandle,
    what: &str,
    holds: impl Fn(&LiveView) -> bool,
) {
    let found = world.run(async {
        let mut watch = handle.live().watch();
        tokio::time::timeout(PATIENCE, watch.wait_for(&holds))
            .await
            .is_ok_and(|view| view.is_some())
    });
    assert!(found, "the stream of the client never held this: {what}");
}

/// The state of a client without a window, once it holds what `holds` asks for.
pub(crate) fn until_model(
    world: &World,
    handle: &ServerHandle,
    what: &str,
    holds: impl Fn(&ServerModel) -> bool,
) {
    until_stream(world, handle, what, |view| {
        view.model.as_deref().is_some_and(&holds)
    });
}

/// Waits until the window gives a value, and returns it.
#[track_caller]
pub(crate) fn awaited<T>(
    window: &mut Window,
    what: &str,
    mut read: impl FnMut(&mut Window) -> Option<T>,
) -> T {
    let mut found = None;
    window.until(what, |window| {
        found = read(window);
        found.is_some()
    });
    found.unwrap()
}

/// Clicks an element of the open page of the administration when it is drawn inside the
/// visible part of the page. An element outside that part is brought nearer with the
/// wheel, as a person does, and is not clicked. True when the element was clicked.
pub(crate) fn try_click(window: &mut Window, selector: &'static str) -> bool {
    window.cx.run_until_parked();
    let (Some(page), Some(element)) = (
        window.cx.debug_bounds("admin-page"),
        window.cx.debug_bounds(selector),
    ) else {
        return false;
    };
    if element.top() >= page.top() && element.bottom() <= page.bottom() {
        window
            .cx
            .simulate_click(element.center(), Modifiers::none());
        window.cx.run_until_parked();
        return true;
    }
    let distance = page.center().y - element.center().y;
    window.cx.simulate_event(ScrollWheelEvent {
        position: page.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), distance)),
        ..Default::default()
    });
    false
}

/// Clicks an element of the open page of the administration. The click waits for an
/// element that is not drawn now, and the page is scrolled to an element below its
/// visible part.
#[track_caller]
pub(crate) fn click_in_page(window: &mut Window, selector: &'static str) {
    window.until(&format!("{selector} is drawn and is clicked"), |window| {
        try_click(window, selector)
    });
}

/// Clicks a row of the list of pages until `open` holds for the window.
#[track_caller]
pub(crate) fn open_page(
    window: &mut Window,
    row: &'static str,
    mut open: impl FnMut(&mut Window) -> bool,
) {
    window.until(&format!("the page of {row} is open"), |window| {
        if open(window) {
            return true;
        }
        window.click_if_shown(row);
        false
    });
}

/// Shows a channel of the channel list and opens its settings with the gear of its header.
#[track_caller]
fn open_channel_settings(window: &mut Window, name: &str, channel_id: ChannelId) {
    window.until("the channel is in the channel list", |window| {
        window.shows_channel(name)
    });
    window.click_channel(name);
    window.until("the settings of the channel are open", |window| {
        let open = matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Channel(settings)) if settings.channel_id == channel_id
        );
        if open {
            return true;
        }
        let shown = window
            .view()
            .workspace()
            .and_then(|workspace| workspace.channel.as_ref())
            .is_some_and(|channel| channel.id == channel_id);
        if shown {
            window.click_if_shown("channel-settings");
        }
        false
    });
}

/// True when no request of the open page is on its way and none was refused.
pub(crate) fn idle(window: &mut Window) -> bool {
    window.admin().is_some_and(|view| view.ask.is_none())
}

/// True when the member list of the shown workspace holds a member with this name.
fn knows_member(window: &mut Window, name: &str) -> bool {
    let view = window.view();
    matches!(
        view.workspace().map(|workspace| &workspace.members),
        Some(Members::Loaded(members)) if members.iter().any(|member| member.name == name)
    )
}

pub(crate) fn account(window: &mut Window, account_id: AccountId) -> Option<AccountLine> {
    server_page!(window, People)?
        .accounts
        .loaded()?
        .iter()
        .find(|row| row.id == account_id)
        .cloned()
}

fn invite_line(window: &mut Window, invite_id: InviteId) -> Option<InviteLine> {
    server_page!(window, Invites)?
        .invites
        .loaded()?
        .iter()
        .find(|row| row.id == invite_id)
        .cloned()
}

fn request_line(window: &mut Window, request_id: JoinRequestId) -> Option<RequestLine> {
    server_page!(window, JoinRequests)?
        .requests
        .loaded()?
        .iter()
        .find(|row| row.id == request_id)
        .cloned()
}

fn token_line(window: &mut Window, token_id: AccessTokenId) -> Option<TokenLine> {
    server_page!(window, Tokens)?
        .tokens
        .loaded()?
        .iter()
        .find(|row| row.id == token_id)
        .cloned()
}

fn export_line(window: &mut Window, export_id: ExportId) -> Option<ExportLine> {
    server_page!(window, Exports)?
        .exports
        .loaded()?
        .iter()
        .find(|row| row.id == export_id)
        .cloned()
}

/// True when the settings of a channel show the page of the permissions.
fn shows_permissions(window: &mut Window) -> bool {
    matches!(
        window.admin().map(|view| view.scope),
        Some(Scope::Channel(settings))
            if matches!(settings.page, ChannelPageView::Permissions(_))
    )
}

/// The "everyone" role on the page of the permissions, once the overrides are read.
fn everyone_role(window: &mut Window) -> Option<RoleLine> {
    let Scope::Channel(settings) = window.admin()?.scope else {
        return None;
    };
    let settings = *settings;
    let ChannelPageView::Permissions(page) = settings.page else {
        return None;
    };
    page.roles
        .loaded()?
        .iter()
        .find(|role| role.is_everyone)
        .cloned()
}

fn permission_line(role: &RoleLine, permission: WorkspacePermission) -> PermissionLine {
    *role
        .permissions
        .iter()
        .find(|held| held.permission == permission)
        .unwrap()
}

const fn choice_word(choice: Choice) -> &'static str {
    match choice {
        Choice::Allow => "allow",
        Choice::Default => "default",
        Choice::Deny => "deny",
    }
}

/// Sets a permission of the "everyone" role to a choice with its button, and waits until
/// the page shows the choice. The page sends nothing while the client reads the overrides
/// again after a change of the permissions, so a click that started no request is made
/// again, as a person does.
#[track_caller]
fn choose(window: &mut Window, permission: WorkspacePermission, choice: Choice) -> RoleLine {
    let button = sel(
        &format!("override-{}", permission.name()),
        choice_word(choice),
    );
    let what = format!("the page shows {choice:?} for {}", permission.name());
    awaited(window, &what, |window| {
        let role = everyone_role(window)?;
        if !idle(window) {
            return None;
        }
        if permission_line(&role, permission).current == choice {
            return Some(role);
        }
        try_click(window, button);
        None
    })
}

/// True when a text holds an identifier: a part of it reads as a UUID.
fn holds_identifier(text: &str) -> bool {
    text.split(|character: char| !(character.is_ascii_hexdigit() || character == '-'))
        .any(|part| uuid::Uuid::parse_str(part).is_ok())
}

/// True when the time from `created` to `expires` is this number of days, within an hour.
fn lasts(created: OffsetDateTime, expires: OffsetDateTime, days: i64) -> bool {
    (expires - created - Duration::days(days)).abs() < Duration::hours(1)
}

/// The operations of the API with an access token as the bearer of each request.
fn with_access_token(world: &World, server: &TestServer, secret: &str) -> ServerApi {
    world.run(async {
        let transport = ReqwestTransport::new().unwrap();
        let address = ServerAddress::parse(&server.address()).unwrap();
        let policy = RetryPolicy::default();
        let info = server_info(&transport, &address, &policy).await.unwrap();
        let version = SupportedVersions::THIS_BUILD
            .negotiate(&info.api_versions)
            .unwrap();
        ApiClient::new(
            Arc::new(transport),
            address,
            info.server_id,
            version,
            policy,
        )
        .with_token(SecretToken::new(secret.to_owned()))
    })
}

#[gpui::test]
fn the_owner_bans_unbans_and_removes_a_member_on_the_people_page(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let (ada, bruno) = (logged_in(&owner), logged_in(&member));
    let on_server = |account_id: AccountId| {
        world
            .run(owner.api().accounts())
            .unwrap()
            .into_iter()
            .find(|held| held.id == account_id)
            .map(|held| held.status)
    };
    let (row_of_ada, row_of_bruno) = (sel("account-row", ada), sel("account-row", bruno));

    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        server_page!(window, People).is_some()
    });
    let (own, other) = awaited(
        &mut window,
        "the page lists the owner and the member",
        |window| Some((account(window, ada)?, account(window, bruno)?)),
    );
    assert!(own.is_owner && own.is_self);
    assert_eq!(own.status, AccountStatus::Active);
    assert_eq!(
        (own.remove, own.ban, own.unban),
        (
            Err(Blocked::Owner),
            Err(Blocked::Owner),
            Err(Blocked::Owner)
        )
    );
    assert_eq!(
        (
            other.name.as_str(),
            other.status,
            other.is_owner,
            other.is_self
        ),
        ("Bruno", AccountStatus::Active, false, false)
    );
    assert_eq!(
        (other.remove, other.ban, other.unban),
        (Ok(()), Ok(()), Err(Blocked::NotBanned))
    );

    // The row of the owner draws its removal and its ban disabled: a click asks nothing.
    for action in ["account-remove", "account-ban"] {
        click_in_page(&mut window, sel(action, ada));
        assert!(
            window.shows(row_of_ada) && !window.shows("account-confirm"),
            "{action} asks about the server owner"
        );
    }
    assert!(!window.shows(sel("account-unban", ada)));

    // The ban of the member asks first, and nothing is sent before the answer.
    let unban = sel("account-unban", bruno);
    click_in_page(&mut window, sel("account-ban", bruno));
    assert!(window.shows("account-confirm") && window.shows("account-cancel"));
    assert_eq!(on_server(bruno), Some(AccountStatus::Active));
    // The question takes a reason, which the audit entry of the ban keeps, and a message
    // to the person, which the notice of the ban gives the person (ADR 0120).
    assert!(window.shows("moderation-reason-field"));
    assert!(window.shows("moderation-message-field"));
    let reason = "Advertisements in every channel";
    let message = "You can ask for an unban by mail.";
    window.in_shell(|shell, window, cx| shell.type_reason(&format!("  {reason} "), window, cx));
    // A control character in the message holds the confirmation.
    window.in_shell(|shell, window, cx| {
        shell.type_moderation_message("ring \u{7}", window, cx);
    });
    assert!(window.shows("moderation-message-invalid"));
    click_in_page(&mut window, "account-confirm");
    assert_eq!(on_server(bruno), Some(AccountStatus::Active));
    window.in_shell(|shell, window, cx| shell.type_moderation_message(message, window, cx));
    click_in_page(&mut window, "account-confirm");
    // The server states that the owner may unban the account, after the read of the
    // accounts that the ban asked for (ADR 0311).
    let banned = awaited(&mut window, "the row shows the ban", |window| {
        account(window, bruno).filter(|row| {
            row.status == AccountStatus::Banned && window.shows(unban) && row.unban.is_ok()
        })
    });
    assert_eq!(
        (banned.remove, banned.ban, banned.unban),
        (Ok(()), Err(Blocked::Banned), Ok(()))
    );
    assert!(!window.shows(sel("account-ban", bruno)) && !window.shows("account-confirm"));
    let entry_of = |action: &str| {
        let query = AuditFilterQuery {
            action: Some(action.to_owned()),
            ..AuditFilterQuery::default()
        };
        let log = world.run(owner.api().server_audit_log(&query)).unwrap();
        assert_eq!(log.entries.len(), 1, "{action}");
        log.entries.into_iter().next().unwrap().event
    };
    assert_eq!(
        entry_of("account_banned"),
        AuditEvent::AccountBanned {
            recipient_message: Some(message.to_owned()),
            reason: Some(reason.to_owned()),
            account_id: bruno,
            cancelled_invite_ids: Vec::new(),
        }
    );

    // The ban ends the session of the member.
    until_stream(
        &world,
        &member,
        "the stream of the banned member is off",
        |view| view.status == StreamStatus::Off,
    );
    assert!(member.live().view().model.is_none());
    assert!(matches!(member.access().state, AccessState::Banned { .. }));

    // The filter "Banned" shows the banned account and no other.
    assert_eq!(
        server_page!(window, People).map(|page| page.banned),
        Some(1)
    );
    click_in_page(&mut window, "people-filter-1");
    assert!(window.shows(row_of_bruno) && !window.shows(row_of_ada));

    // The unban makes the account active again, and the filter then shows no account.
    click_in_page(&mut window, unban);
    awaited(&mut window, "the member is active again", |window| {
        account(window, bruno).filter(|row| row.status == AccountStatus::Active)
    });
    window.until("the filter shows no account", |window| {
        window.shows("accounts-none") && !window.shows(row_of_bruno)
    });
    assert_eq!(
        server_page!(window, People).map(|page| page.banned),
        Some(0)
    );
    click_in_page(&mut window, "people-filter-0");
    assert!(window.shows(row_of_ada) && window.shows(row_of_bruno));
    assert_eq!(on_server(bruno), Some(AccountStatus::Active));

    // The removal asks too, and the row of a removed account has no action.
    click_in_page(&mut window, sel("account-remove", bruno));
    assert!(window.shows("account-confirm"));
    assert_eq!(on_server(bruno), Some(AccountStatus::Active));
    // The reason of the ban belonged to its question: this question starts without one.
    assert!(window.shows("moderation-reason-field"));
    click_in_page(&mut window, "account-confirm");
    let removed = awaited(&mut window, "the row shows the removal", |window| {
        account(window, bruno)
            .filter(|row| row.status == AccountStatus::Removed && !window.shows("account-confirm"))
    });
    assert_eq!(
        (removed.remove, removed.ban, removed.unban),
        (
            Err(Blocked::Removed),
            Err(Blocked::Removed),
            Err(Blocked::Removed)
        )
    );
    assert!(window.shows(row_of_bruno));
    for action in ["account-remove", "account-ban", "account-unban"] {
        assert!(
            !window.shows(sel(action, bruno)),
            "{action} is drawn for a removed account"
        );
    }
    assert_eq!(on_server(bruno), Some(AccountStatus::Removed));
    assert_eq!(
        entry_of("account_removed"),
        AuditEvent::AccountRemoved {
            recipient_message: None,
            reason: None,
            account_id: bruno,
        }
    );
}

#[gpui::test]
fn the_members_page_lists_the_members_and_offers_the_active_accounts_that_are_no_members(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let (ada, bruno) = (logged_in(&owner), logged_in(&member));
    let default = default_workspace(&world, &owner);
    let api = owner.api();
    let nobody = Some(Listed::Loaded(Vec::<AddableLine>::new()));

    // Both accounts are members of the shown workspace: the page offers no account.
    window.open_administration();
    open_page(&mut window, "admin-nav-members", |window| {
        server_page!(window, Members).is_some()
    });
    let page = awaited(
        &mut window,
        "the page lists both members and no account to add",
        |window| {
            server_page!(window, Members).filter(|page| {
                matches!(&page.members, MemberList::Loaded(rows) if rows.len() == 2)
                    && page.addable == nobody
            })
        },
    );
    assert_eq!(page.workspace_id, default);
    let MemberList::Loaded(rows) = &page.members else {
        panic!("the members are read");
    };
    let listed: Vec<(AccountId, &str, bool)> = rows
        .iter()
        .map(|row| (row.account_id, row.name.as_str(), row.is_self))
        .collect();
    assert_eq!(listed, [(ada, "Ada", true), (bruno, "Bruno", false)]);
    assert!(window.shows(sel("member-row", ada)) && window.shows(sel("member-row", bruno)));
    assert!(!window.shows(sel("member-addable", bruno)) && !window.shows(sel("member-add", bruno)));

    // A second workspace: the window keeps the workspace that it shows.
    let lab = world.run(api.create_workspace("Lab".to_owned())).unwrap();
    let shown = awaited(
        &mut window,
        "the server book lists the new workspace",
        |window| {
            let view = window.view();
            let listed = view
                .servers
                .first()?
                .workspaces
                .iter()
                .any(|workspace| workspace.id == lab.id);
            view.workspace()
                .filter(|_| listed)
                .map(|workspace| workspace.id)
        },
    );
    assert_eq!(shown, default);

    // An account that joined the second workspace only is no member of the shown one. The
    // page offers the active accounts of the server that are no members.
    let code = invite(&world, &owner, vec![lab.id]);
    let (_third, third) = joined(&world, &server, &code, "Dora");
    let dora = logged_in(&third);
    let offered = Some(Listed::Loaded(vec![AddableLine {
        account_id: dora,
        name: "Dora".to_owned(),
    }]));
    let (addable, add) = (sel("member-addable", dora), sel("member-add", dora));
    let row_of_dora = sel("member-row", dora);
    let page = awaited(
        &mut window,
        "the page offers the account that is no member",
        |window| {
            server_page!(window, Members).filter(|page| {
                page.addable == offered && window.shows(addable) && window.shows(add)
            })
        },
    );
    assert_eq!(page.workspace_id, default);
    assert!(matches!(&page.members, MemberList::Loaded(rows) if rows.len() == 2));
    assert!(!window.shows(row_of_dora));
    assert!(!window.shows(sel("member-addable", ada)));
    assert!(!window.shows(sel("member-addable", bruno)));

    // A banned account is not offered, and it is offered again after its unban.
    world.run(api.ban_account(dora)).unwrap();
    awaited(&mut window, "a banned account is not offered", |window| {
        server_page!(window, Members).filter(|page| page.addable == nobody)
    });
    world.run(api.unban_account(dora)).unwrap();
    awaited(&mut window, "the account is offered again", |window| {
        server_page!(window, Members).filter(|page| page.addable == offered)
    });

    // "Add to workspace" makes the account a member.
    click_in_page(&mut window, add);
    awaited(&mut window, "the added account is a member", |window| {
        server_page!(window, Members).filter(|page| {
            matches!(
                &page.members,
                MemberList::Loaded(rows) if rows.iter().any(|row| row.account_id == dora)
            ) && page.addable == nobody
                && window.shows(row_of_dora)
        })
    });
    assert!(!window.shows(addable));
    let members = world.run(api.workspace_members(default)).unwrap();
    assert!(members.iter().any(|held| held.account_id == dora));
}

#[gpui::test]
fn a_member_with_the_permission_follows_the_invites_and_creates_one_with_the_form(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.view().person.account_id.unwrap();
    let api = admin.handle.api();

    // Without the permission the list of pages has no page of the invites.
    window.open_administration();
    window.until("the list of pages has no page of the invites", |window| {
        window.shows("admin-nav-workspaces") && !window.shows("admin-nav-invites")
    });
    grant_server(&world, &admin.handle, own, ServerPermission::CreateInvites);
    open_page(&mut window, "admin-nav-invites", |window| {
        server_page!(window, Invites).is_some()
    });
    awaited(&mut window, "the invites are read", |window| {
        server_page!(window, Invites)?.invites.loaded().map(|_| ())
    });

    // An invite of the owner appears, and its revocation reaches its row.
    let theirs = world
        .run(api.create_invite(&CreateInvite::default()))
        .unwrap()
        .invite
        .id;
    let (their_row, their_revoke) = (sel("invite-row", theirs), sel("invite-revoke", theirs));
    let row = awaited(
        &mut window,
        "the invite of the owner has its row",
        |window| {
            invite_line(window, theirs)
                .filter(|_| window.shows(their_row) && window.shows(their_revoke))
        },
    );
    assert_eq!(
        (row.state, row.may_revoke, row.uses, row.max_uses),
        (InviteState::Usable, true, 0, 1)
    );
    world.run(api.revoke_invite(theirs)).unwrap();
    let row = awaited(
        &mut window,
        "the row shows the invite as revoked",
        |window| {
            invite_line(window, theirs).filter(|row| {
                row.state == InviteState::Revoked
                    && window.shows(their_row)
                    && !window.shows(their_revoke)
            })
        },
    );
    assert!(!row.may_revoke);

    // The window creates an invite for one day and five uses with the controls of the
    // form. The code is shown once.
    let day = EXPIRY_DAYS.iter().position(|days| *days == 1).unwrap();
    let five = USES.iter().position(|uses| *uses == 5).unwrap();
    click_in_page(&mut window, "invite-new");
    window.until("the form of a new invite is open", |window| {
        window.shows("invite-form")
    });
    click_in_page(&mut window, sel("invite-expiry", day));
    click_in_page(&mut window, sel("invite-uses", five));
    click_in_page(&mut window, "invite-create");
    let made = awaited(
        &mut window,
        "the code of the new invite is shown",
        |window| {
            server_page!(window, Invites)?
                .made
                .filter(|_| window.shows("invite-code"))
        },
    );
    let (mine, code) = (made.invite_id, made.code.expose().to_owned());
    let my_row = sel("invite-row", mine);
    let row = awaited(&mut window, "the new invite has its row", |window| {
        invite_line(window, mine).filter(|_| window.shows(my_row) && !window.shows("invite-form"))
    });
    assert_eq!(
        (row.created_by, row.uses, row.max_uses, row.state),
        (own, 0, 5, InviteState::Usable)
    );
    assert!(lasts(row.created, row.expires, 1));

    // A third device joins with the code that the window showed: the row counts the use.
    let (_third, _dora) = joined(&world, &server, &InviteCode::new(code), "Dora");
    let row = awaited(&mut window, "the row counts the use", |window| {
        invite_line(window, mine).filter(|row| row.uses == 1)
    });
    assert_eq!((row.max_uses, row.state), (5, InviteState::Usable));

    // "Done" puts the code away. The row of the invite stays.
    click_in_page(&mut window, "invite-code-done");
    window.until("the code is put away", |window| {
        window.shows(my_row)
            && !window.shows("invite-code")
            && !window.shows("invite-made")
            && server_page!(window, Invites).is_some_and(|page| page.made.is_none())
            && window.admin().is_some_and(|view| view.made.is_none())
    });
}

#[gpui::test]
fn the_owner_approves_one_join_request_and_denies_another_in_the_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let default = default_workspace(&world, &owner);

    window.open_administration();
    open_page(&mut window, "admin-nav-requests", |window| {
        server_page!(window, JoinRequests).is_some()
    });
    window.until("no join request waits", |window| {
        window.shows("requests-none")
    });

    // A request appears, and the owner approves it in the window.
    let (_first, linus, first) = asks_to_join(&world, &server, "Linus");
    let first_row = sel("request-row", first);
    let row = awaited(&mut window, "the request has its row", |window| {
        request_line(window, first).filter(|_| window.shows(first_row))
    });
    assert_eq!(row.name, "Linus");
    click_in_page(&mut window, sel("request-approve", first));
    window.until("the approved request left the list", |window| {
        window.shows("requests-none") && !window.shows(first_row)
    });
    window.until("the device that asked is a member", |_| {
        if matches!(linus.access().state, AccessState::WaitingForApproval { .. }) {
            let _ = world.run(linus.dispatch(AccessAction::Check));
        }
        matches!(linus.access().state, AccessState::LoggedIn { .. })
    });
    let account_id = logged_in(&linus);
    let members = world.run(owner.api().workspace_members(default)).unwrap();
    assert!(
        members
            .iter()
            .any(|held| held.account_id == account_id && held.name == "Linus")
    );

    // A second request is denied: it leaves the list, and no account is created.
    let (_second, marta, second) = asks_to_join(&world, &server, "Marta");
    let second_row = sel("request-row", second);
    awaited(&mut window, "the second request has its row", |window| {
        request_line(window, second).filter(|_| window.shows(second_row))
    });
    click_in_page(&mut window, sel("request-deny", second));
    window.until("the denied request left the list", |window| {
        window.shows("requests-none") && !window.shows(second_row)
    });
    window.until("the second device is told the denial", |_| {
        if matches!(marta.access().state, AccessState::WaitingForApproval { .. }) {
            let _ = world.run(marta.dispatch(AccessAction::Check));
        }
        matches!(
            marta.access().state,
            AccessState::NoAccount {
                notice: Some(Notice::RequestDenied)
            }
        )
    });
    let accounts = world.run(owner.api().accounts()).unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(accounts.iter().all(|held| held.name != "Marta"));
}

#[gpui::test]
fn the_owner_hides_a_channel_from_everyone_and_shows_it_again_on_the_permissions_page(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let default = default_workspace(&world, &owner);
    let plans = text_channel(&world, &owner, default, "plans");
    let view_channel = WorkspacePermission::ViewChannel;
    until_model(&world, &member, "the member holds the channel", |model| {
        model.channel(plans).is_some()
    });

    // The gear of the channel opens its settings. The page of the permissions shows the
    // "everyone" role first, with its permissions.
    open_channel_settings(&mut window, "plans", plans);
    open_page(&mut window, "channel-nav-permissions", shows_permissions);
    let everyone = awaited(
        &mut window,
        "the overrides of the channel are read",
        everyone_role,
    );
    assert!(everyone.is_everyone && !everyone.has_override);
    let line = permission_line(&everyone, view_channel);
    assert_eq!((line.current, line.held), (Choice::Default, true));
    // The owner may make every choice: no row states a reason.
    assert!(
        everyone
            .permissions
            .iter()
            .all(|held| held.reason().is_none())
    );
    window.until("the page states no reason", |window| {
        window.shows("override-view_channel-deny") && !window.shows("override-ceiling")
    });

    // "Deny" for "view channel": the member has the channel no longer.
    let everyone = choose(&mut window, view_channel, Choice::Deny);
    assert!(everyone.has_override);
    assert_eq!((everyone.allowed(), everyone.denied()), (0, 1));
    until_model(
        &world,
        &member,
        "the channel is hidden from the member",
        |model| model.channel(plans).is_none(),
    );

    // "Default" removes the deny, and the channel returns.
    let everyone = choose(&mut window, view_channel, Choice::Default);
    assert!(!everyone.has_override);
    until_model(
        &world,
        &member,
        "the channel is shown to the member again",
        |model| model.channel(plans).is_some(),
    );
    let stored = world.run(owner.api().channel_overrides(plans)).unwrap();
    assert!(stored.is_empty(), "{stored:?}");
}

#[gpui::test]
fn a_member_who_manages_a_channel_cannot_allow_or_restore_what_the_member_does_not_hold(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.view().person.account_id.unwrap();
    let api = admin.handle.api();
    // The client of the owner follows the server.
    world.run(async {
        admin.device.client.start_event_streams().unwrap();
        admin.device.client.select(Some(admin.handle.id()));
    });
    until_model(
        &world,
        &admin.handle,
        "the first state of the owner",
        |_| true,
    );
    let default = default_workspace(&world, &admin.handle);
    let plans = text_channel(&world, &admin.handle, default, "plans");
    let messages = WorkspacePermission::ManageMessages;
    until_model(
        &world,
        &admin.handle,
        "the owner holds the channel",
        |model| model.channel(plans).is_some(),
    );

    // A member without "manage channel" has no page of the permissions.
    open_channel_settings(&mut window, "plans", plans);
    window.until("the settings have no page of the permissions", |window| {
        window.shows("channel-nav-archive") && !window.shows("channel-nav-permissions")
    });

    // A role with "manage channel" and without "manage messages" gives the page.
    grant_workspace(
        &world,
        &admin.handle,
        default,
        own,
        WorkspacePermission::ManageChannel,
    );
    open_page(&mut window, "channel-nav-permissions", shows_permissions);
    let everyone = awaited(
        &mut window,
        "the overrides of the channel are read",
        everyone_role,
    );
    let line = permission_line(&everyone, messages);
    assert_eq!((line.current, line.held), (Choice::Default, false));
    assert_eq!(line.refused(Choice::Allow), Some(Refused::Allows));
    assert_eq!(line.refused(Choice::Deny), None);
    assert_eq!(line.reason(), Some(Refused::Allows));
    assert!(permission_line(&everyone, WorkspacePermission::ManageChannel).held);
    assert!(permission_line(&everyone, WorkspacePermission::ViewChannel).held);

    // "Allow" is disabled and the row states the reason: a click sends nothing.
    window.until("the page states the reason", |window| {
        window.shows("override-manage_messages-allow") && window.shows("override-ceiling")
    });
    click_in_page(&mut window, "override-manage_messages-allow");
    assert!(window.admin().is_none_or(|view| view.ask.is_none()));

    // "Deny" works, and it reaches the client of the owner.
    world
        .run(admin.handle.live().open_admin(AdminPart::Overrides(plans)))
        .unwrap();
    let everyone = choose(&mut window, messages, Choice::Deny);
    until_model(
        &world,
        &admin.handle,
        "the deny in the state of the owner",
        |model| {
            model
                .admin
                .overrides(plans)
                .loaded()
                .is_some_and(|overrides| {
                    overrides.iter().any(|held| {
                        held.role_id == everyone.id
                            && held.allow.is_empty()
                            && held.deny == [messages]
                    })
                })
        },
    );

    // The member cannot lift the deny: "Default", "Allow" and the removal of the override
    // would give "manage messages", which the member does not hold.
    let line = permission_line(&everyone, messages);
    assert_eq!((line.current, line.held), (Choice::Deny, false));
    assert_eq!(line.refused(Choice::Default), Some(Refused::LiftsDeny));
    assert_eq!(line.refused(Choice::Allow), Some(Refused::LiftsDeny));
    assert_eq!(everyone.blocks_removal(), [messages]);
    window.until("the page states the reason of the deny", |window| {
        window.shows("override-manage_messages-default") && window.shows("override-ceiling")
    });
    for word in ["default", "allow"] {
        click_in_page(&mut window, sel("override-manage_messages", word));
        assert!(
            window.admin().is_none_or(|view| view.ask.is_none()),
            "{word} sent a request"
        );
    }
    click_in_page(&mut window, sel("override-clear", everyone.id));
    assert!(window.admin().is_none_or(|view| view.ask.is_none()));
    let stored = world.run(api.channel_overrides(plans)).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (stored[0].role_id, stored[0].deny.as_slice()),
        (everyone.id, [messages].as_slice())
    );
}

#[gpui::test]
fn the_owner_creates_an_access_token_uses_it_and_revokes_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);

    window.open_administration();
    open_page(&mut window, "admin-nav-tokens", |window| {
        server_page!(window, Tokens).is_some()
    });
    window.until("the server lists no access token", |window| {
        window.shows("no-tokens")
    });

    // The form: a name, the one action "read the audit log", and seven days. The form
    // opens with "read the structure" on.
    let week = TOKEN_EXPIRY_DAYS
        .iter()
        .position(|days| *days == 7)
        .unwrap();
    click_in_page(&mut window, "token-new");
    assert!(window.shows("token-form"));
    window.in_shell(|shell, window, cx| shell.type_token_name("setup", window, cx));
    click_in_page(
        &mut window,
        token_action_selector(TokenAction::ReadAuditLog),
    );
    click_in_page(
        &mut window,
        token_action_selector(TokenAction::ReadStructure),
    );
    click_in_page(&mut window, sel("token-expiry", week));
    click_in_page(&mut window, "token-create");

    // The secret is shown once.
    let made = awaited(
        &mut window,
        "the secret of the new token is shown",
        |window| {
            server_page!(window, Tokens)?
                .made
                .filter(|_| window.shows("token-secret"))
        },
    );
    let (token_id, secret) = (made.access_token_id, made.token.expose().to_owned());
    assert!(
        secret.starts_with("jat_"),
        "the secret has the prefix of an access token"
    );
    assert!(!window.shows("token-form"));

    // The list holds the token.
    let token_row = sel("token-row", token_id);
    let row = awaited(&mut window, "the list holds the token", |window| {
        token_line(window, token_id).filter(|_| window.shows(token_row))
    });
    assert_eq!(row.name, "setup");
    assert_eq!(row.actions, [TokenAction::ReadAuditLog]);
    assert_eq!(row.created_by, logged_in(&owner));
    assert!(row.own && !row.expired);
    assert!(lasts(row.created, row.expires, 7));

    // A request with the token works on a route of its action, and on no other route.
    let with_token = with_access_token(&world, &server, &secret);
    let log = world
        .run(with_token.server_audit_log(&AuditFilterQuery::default()))
        .unwrap();
    assert!(!log.entries.is_empty());
    let refused = world.run(with_token.workspaces()).unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));

    // "Done" puts the secret away. The row of the token stays.
    click_in_page(&mut window, "token-secret-done");
    assert!(window.shows(token_row));
    assert!(!window.shows("token-secret") && !window.shows("token-made"));
    assert!(server_page!(window, Tokens).is_some_and(|page| page.made.is_none()));
    assert!(window.admin().is_some_and(|view| view.made.is_none()));

    // The revocation removes the row, and the server refuses the token.
    click_in_page(&mut window, sel("token-revoke", token_id));
    window.until("the revoked token left the list", |window| {
        window.shows("no-tokens") && !window.shows(token_row)
    });
    let ended = world
        .run(with_token.server_audit_log(&AuditFilterQuery::default()))
        .unwrap_err();
    assert_eq!(ended.category(), Some(ErrorCategory::SessionEnded));
}

#[gpui::test]
fn the_owner_exports_a_channel_saves_the_file_and_removes_the_export(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let default = default_workspace(&world, &owner);
    let plans = text_channel(&world, &owner, default, "plans");
    let message = SendMessage {
        document: document("kept in the export"),
        reply_to: None,
        attachments: Vec::new(),
    };
    world
        .run(owner.api().send_message(PlaceId::Channel(plans), &message))
        .unwrap();

    // The files that the window saves go to a directory of this test.
    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell.state().clone().update(cx, |state, _| {
            state.set_download_directory(target);
        });
    });

    // The page "Archive and deletion" of the settings of the channel asks for the export.
    open_channel_settings(&mut window, "plans", plans);
    open_page(&mut window, "channel-nav-archive", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Channel(settings))
                if matches!(settings.page, ChannelPageView::Archive(_))
        )
    });
    click_in_page(&mut window, "channel-export");
    let export_id = awaited(
        &mut window,
        "the export was asked for",
        |window| match window.admin()?.made {
            Some(Made::Export(export_id)) if window.shows("show-exports") => Some(export_id),
            _ => None,
        },
    );
    click_in_page(&mut window, "show-exports");

    // The export becomes ready on the page of the exports.
    let (export_row, saved_line) = (sel("export-row", export_id), sel("export-saved", export_id));
    let row = awaited(&mut window, "the export is ready", |window| {
        export_line(window, export_id).filter(|row| {
            row.status == ExportStatus::Ready && row.can_save && window.shows(export_row)
        })
    });
    let bytes = row.bytes.unwrap();
    assert!(bytes > 0);
    assert_eq!(row.save, None);

    // "Save" writes the file into the download directory, and the page states its path.
    click_in_page(&mut window, sel("export-save", export_id));
    let saved = awaited(
        &mut window,
        "the page states where the file was saved",
        |window| match export_line(window, export_id)?.save {
            Some(ExportSave::Saved(path)) if window.shows(saved_line) => Some(path),
            _ => None,
        },
    );
    assert_eq!(saved.parent(), Some(downloads.path()));
    let name = saved.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("junto-export-plans-") && name.ends_with(".zip"),
        "{name}"
    );
    let file = std::fs::read(&saved).unwrap();
    assert_eq!(file.len() as u64, bytes);
    assert!(file.starts_with(b"PK"));
    // The unfinished file of the save is gone.
    assert_eq!(std::fs::read_dir(downloads.path()).unwrap().count(), 1);

    // The removal asks first. It removes the export on the server, and the saved file
    // stays on this device.
    click_in_page(&mut window, sel("export-remove", export_id));
    assert!(window.shows("export-confirm") && window.shows("export-cancel"));
    assert!(world.run(owner.api().export(export_id)).is_ok());
    click_in_page(&mut window, "export-confirm");
    window.until("the removed export left the page", |window| {
        window.shows("no-exports") && !window.shows(export_row)
    });
    let gone = world.run(owner.api().export(export_id)).unwrap_err();
    assert_eq!(gone.category(), Some(ErrorCategory::NotFound));
    assert!(saved.exists());
}

#[gpui::test]
fn the_audit_log_names_what_was_done_filters_by_group_and_loads_older_entries(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let default = default_workspace(&world, &owner);
    let api = owner.api();
    // The log names an account whose name the window holds: here with the member list of
    // the shown workspace.
    window.until("the window lists the member", |window| {
        knows_member(window, "Bruno")
    });

    // A channel, then 60 categories, and the ban of the member.
    text_channel(&world, &owner, default, "plans");
    for number in 1..=60 {
        let category = CreateCategory {
            name: format!("Category {number:02}"),
            position: None,
        };
        world.run(api.create_category(default, &category)).unwrap();
    }
    let reason = "Sent the same advertisement to every channel";
    world
        .run(api.ban_account_stating(bruno, &junto_client_core::api::Statement::reason(reason)))
        .unwrap();

    // The page shows the log of the server first: the ban, with the name of the member.
    window.open_administration();
    open_page(&mut window, "admin-nav-audit", |window| {
        server_page!(window, AuditLog).is_some()
    });
    let page = awaited(
        &mut window,
        "the log of the server names the banned member",
        |window| {
            server_page!(window, AuditLog).filter(|page| {
                page.scope == Some(AuditScope::Server)
                    && page.state == AuditRead::Loaded
                    && page.rows.iter().any(|row| {
                        row.sentence.contains("Bruno") && row.sentence.contains("banned")
                    })
            })
        },
    );
    let scopes: Vec<AuditScope> = page.scopes.iter().map(|choice| choice.scope).collect();
    assert_eq!(scopes, [AuditScope::Server, AuditScope::Workspace(default)]);
    assert_eq!(
        (page.rows.len(), page.filter, page.older),
        (page.loaded, AuditFilter::default(), false)
    );
    let banned = page
        .rows
        .iter()
        .find(|row| row.sentence.contains("banned"))
        .unwrap();
    assert_eq!(banned.group, AuditGroup::Members);
    assert!(matches!(
        &banned.actor,
        Actor::Account { name: Some(name), .. } if name == "Ada"
    ));
    // The entry of the ban states the reason that the administrator gave.
    assert_eq!(banned.reason.as_deref(), Some(reason));
    assert!(window.shows(sel("audit-reason", banned.id)));
    let banned_row = sel("audit-row", banned.id);
    assert!(window.shows(banned_row));
    for row in &page.rows {
        for text in [row.sentence.clone(), audit_head(row)] {
            assert!(
                !holds_identifier(&text),
                "an entry shows an identifier: {text}"
            );
        }
    }

    // The filter by action: the server returns the entries of that action only, so the
    // page holds fewer entries than before, and each of them is a ban.
    let other = page
        .rows
        .iter()
        .find(|row| row.group != AuditGroup::Members)
        .unwrap();
    let other_row = sel("audit-row", other.id);
    click_in_page(&mut window, "audit-action");
    window.until("the actions of the first group are offered", |window| {
        window.shows("audit-action-account_banned")
    });
    click_in_page(&mut window, "audit-action-account_banned");
    let filtered = awaited(&mut window, "the server returned the bans", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.filter.action == Some("account_banned")
                && page.state == AuditRead::Loaded
                && window.shows(banned_row)
                && !window.shows(other_row)
        })
    });
    assert!(filtered.loaded < page.loaded);
    assert_eq!(filtered.rows.len(), filtered.loaded);
    assert!(
        filtered
            .rows
            .iter()
            .all(|row| row.sentence.contains("banned"))
    );
    assert!(window.shows("audit-filtered") && window.shows("audit-count"));
    assert!(!window.shows("audit-actions"));

    // The filter by the person concerned names the banned account; the filter by the
    // person who acted with an account that did nothing leaves no entry.
    click_in_page(&mut window, "audit-about");
    click_in_page(&mut window, sel("audit-about", bruno));
    awaited(&mut window, "the entries about the member", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.filter.about == Some(bruno)
                && page.state == AuditRead::Loaded
                && page.rows.iter().any(|row| row.id == banned.id)
        })
    });
    click_in_page(&mut window, "audit-actor");
    click_in_page(&mut window, sel("audit-actor", bruno));
    awaited(&mut window, "no entry passes the filters", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.filter.actor == Some(bruno)
                && page.state == AuditRead::Loaded
                && page.loaded == 0
                && window.shows("audit-none-pass")
        })
    });
    let total = page.loaded;
    click_in_page(&mut window, "audit-clear");
    awaited(&mut window, "the page shows every action again", |window| {
        server_page!(window, AuditLog).filter(|page| {
            !page.filter.is_set()
                && page.state == AuditRead::Loaded
                && page.loaded == total
                && window.shows(other_row)
        })
    });

    // A period that ends before the first entry of the server holds nothing; the last
    // 24 hours hold every entry of this test.
    click_in_page(&mut window, "audit-period-1");
    let recent = awaited(&mut window, "the entries of the last 24 hours", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.filter.period == AuditPeriod::Day && page.state == AuditRead::Loaded
        })
    });
    assert_eq!(recent.loaded, total);
    click_in_page(&mut window, "audit-period-0");
    awaited(&mut window, "the page shows every time again", |window| {
        server_page!(window, AuditLog)
            .filter(|page| !page.filter.is_set() && page.state == AuditRead::Loaded)
    });

    // The log of the workspace holds more entries than one read gives: the newest 50 are
    // categories.
    let most = usize::try_from(AUDIT_PAGE).unwrap();
    click_in_page(&mut window, "audit-scope-1");
    let first = awaited(
        &mut window,
        "the first entries of the log of the workspace are read",
        |window| {
            server_page!(window, AuditLog).filter(|page| {
                page.scope == Some(AuditScope::Workspace(default))
                    && page.state == AuditRead::Loaded
                    && page.loaded > 0
            })
        },
    );
    assert_eq!((first.loaded, first.rows.len()), (most, most));
    assert!(first.older && !first.at_bound);
    assert!(
        first
            .rows
            .iter()
            .all(|row| row.sentence.contains("Category"))
    );

    // "Load older" reads the rest, with the entry of the channel.
    click_in_page(&mut window, "audit-older");
    let all = awaited(&mut window, "the older entries are read", |window| {
        server_page!(window, AuditLog)
            .filter(|page| page.state == AuditRead::Loaded && page.loaded > most)
    });
    assert!(!all.older && !window.shows("audit-older"));
    assert_eq!(all.rows.len(), all.loaded);
    let categories = all
        .rows
        .iter()
        .filter(|row| row.sentence.contains("Category"))
        .count();
    assert_eq!(categories, 60);
    let created = all
        .rows
        .iter()
        .find(|row| row.sentence.contains("plans"))
        .unwrap();
    assert!(created.sentence.contains("created"), "{}", created.sentence);
    assert_eq!(created.group, AuditGroup::Structure);
    assert_eq!(created.channel, Some(About::Named("# plans".to_owned())));
    assert!(window.shows(sel("audit-row", created.id)));
    for row in &all.rows {
        for text in [row.sentence.clone(), audit_head(row)] {
            assert!(
                !holds_identifier(&text),
                "an entry shows an identifier: {text}"
            );
        }
    }
}

/// Whether the messages of a channel are permanent and whether the person is the owner, as
/// the open page "Archive and deletion" of the channel states them.
fn archive_page_of(window: &mut Window, channel_id: ChannelId) -> Option<(bool, bool)> {
    match window.admin()?.scope {
        Scope::Channel(settings) if settings.channel_id == channel_id => match settings.page {
            ChannelPageView::Archive(page) => Some((page.permanent, page.is_owner)),
            _ => None,
        },
        _ => None,
    }
}

#[gpui::test]
fn a_member_who_manages_channels_deletes_none_with_permanent_messages(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.view().person.account_id.unwrap();
    let default = default_workspace(&world, &admin.handle);
    let notes = text_channel(&world, &admin.handle, default, "notes");
    let plans = text_channel(&world, &admin.handle, default, "plans");
    grant_workspace(
        &world,
        &admin.handle,
        default,
        own,
        WorkspacePermission::ManageChannel,
    );
    let permanent = UpdateChannel {
        permanent_messages: Some(true),
        ..Default::default()
    };
    world
        .run(admin.handle.api().update_channel(plans, &permanent))
        .unwrap();

    // The member may delete a channel without permanent messages: the question opens.
    open_channel_settings(&mut window, "notes", notes);
    open_page(&mut window, "channel-nav-archive", |window| {
        archive_page_of(window, notes) == Some((false, false))
    });
    click_in_page(&mut window, "channel-delete");
    window.until("the question of the deletion", |window| {
        window.shows("channel-delete-confirm")
    });
    window.click("channel-delete-cancel");
    window.until("the question is closed", |window| {
        !window.shows("channel-delete-confirm")
    });
    window.click("admin-close");
    window.until("the settings are closed", |window| !window.shows("admin"));

    // The deletion of a channel with permanent messages is the owner's: the action is
    // drawn, and a click opens no question.
    open_channel_settings(&mut window, "plans", plans);
    open_page(&mut window, "channel-nav-archive", |window| {
        archive_page_of(window, plans) == Some((true, false))
    });
    click_in_page(&mut window, "channel-delete");
    window.settle();
    assert!(!window.shows("channel-delete-confirm"));
    assert!(window.shows("channel-delete"));
}
