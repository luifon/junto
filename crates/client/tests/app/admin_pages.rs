//! The pages of the administration in the window against the real server (ADR 0761): the
//! people of a server, the members of a workspace, invites, join requests, the permissions
//! of a channel, access tokens, exports and the audit log.
//!
//! A change of the permissions of an account makes its client read the state of the server
//! again, and a list of a page is on its way for a moment then. A test therefore reads what
//! a page shows in a wait, and clicks an element when it is drawn.

use std::sync::Arc;

use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use junto_client::state::admin::view::overrides::{
    Choice, PermissionLine, Refused, SubjectKind, SubjectLine,
};
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
use junto_client_core::protocol::access_token::{CreateAccessToken, TokenAction};
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
use crate::support::{Device, FaultProxy, TestServer, join, logged_in};

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

#[path = "admin_actions.rs"]
mod actions;

#[path = "admin_exports.rs"]
mod exports_actions;

pub(crate) fn drawn_text(window: &mut Window) -> Vec<junto_client::ui::PaintedText> {
    let _record = junto_client::ui::record_painted_text();
    window.cx.update(|window, _| window.refresh());
    window.settle();
    junto_client::ui::take_painted_text()
}

pub(crate) fn assert_drawn_label(window: &mut Window, label: &str) {
    let text = drawn_text(window);
    assert!(
        text.iter()
            .any(|text| text.text == label || text.label.as_deref() == Some(label)),
        "missing painted label {label}: {text:?}"
    );
}

fn assert_drawn_selection(window: &mut Window, label: &str, selected: bool) {
    let text = drawn_text(window);
    let control = text
        .iter()
        .find(|text| text.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("missing painted control {label}: {text:?}"));
    assert_eq!(
        control.text.as_ref(),
        if selected { "On" } else { "Off" },
        "{label}"
    );
}

fn assert_drawn_setting(window: &mut Window, control: &'static str, title: &str, value: &str) {
    let text = drawn_text(window);
    let control = window.cx.debug_bounds(control).unwrap();
    assert!(
        text.iter().filter(|text| text.text == title).any(|title| {
            text.iter().any(|text| {
                text.text == value
                    && text.bounds.left() == title.bounds.left()
                    && text.bounds.top() >= title.bounds.bottom()
                    && title.bounds.top() <= control.center().y
                    && text.bounds.bottom() >= control.center().y
            })
        }),
        "{title} should show {value}: {text:?}"
    );
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
    owner_window_at(cx, server, &server.address())
}

/// As [`owner_window`], with the window reaching the server at `address`: the address of
/// the server or of a proxy in front of it.
pub(crate) fn owner_window_at(
    cx: &mut TestAppContext,
    server: &TestServer,
    address: &str,
) -> (tempfile::TempDir, Window, ServerHandle) {
    let (directory, mut window) = named(cx, "Ada");
    window.find_server(address);
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
pub(crate) fn grant_server(
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
pub(crate) fn grant_workspace(
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
    let Some(element) = try_reveal(window, selector) else {
        return false;
    };
    window.simulate(|cx| cx.simulate_click(element.center(), Modifiers::none()));
    window.park();
    true
}

/// The bounds of an element of the open page of the administration when it is drawn
/// inside the visible part of the page. An element outside that part is brought nearer
/// with the wheel, as a person does.
pub(crate) fn try_reveal(
    window: &mut Window,
    selector: &'static str,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    window.park();
    let page = window.cx.debug_bounds("admin-page")?;
    let Some(element) = window.cx.debug_bounds(selector) else {
        if window.in_shell(|shell, _, _| shell.admin_page_at_end()) {
            window.in_shell(|shell, _, cx| {
                shell.scroll_admin_page_to_start();
                cx.notify();
            });
        } else {
            window.simulate(|cx| {
                cx.simulate_event(ScrollWheelEvent {
                    position: page.center(),
                    delta: ScrollDelta::Pixels(point(px(0.0), -page.size.height * 0.8)),
                    ..Default::default()
                })
            });
        }
        return None;
    };
    if element.top() >= page.top() && element.bottom() <= page.bottom() {
        return Some(element);
    }
    let distance = if element.top() < page.top() {
        page.top() - element.top()
    } else {
        page.bottom() - element.bottom()
    };
    window.simulate(|cx| {
        cx.simulate_event(ScrollWheelEvent {
            position: page.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), distance)),
            ..Default::default()
        })
    });
    None
}

pub(crate) fn reveal_in_page(
    window: &mut Window,
    selector: &'static str,
) -> gpui::Bounds<gpui::Pixels> {
    awaited(window, &format!("{selector} is in view"), |window| {
        try_reveal(window, selector)
    })
}

/// The bounds of an element of the open page of the administration when it is drawn whole
/// inside the visible part of the page. The page is not scrolled.
pub(crate) fn in_view(
    window: &mut Window,
    selector: &'static str,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let page = window.cx.debug_bounds("admin-page")?;
    window
        .cx
        .debug_bounds(selector)
        .filter(|bounds| bounds.top() >= page.top() && bounds.bottom() <= page.bottom())
}

/// The bounds of `under` for a comparison of its place before and after an answer. The
/// row of the form, with the key `form_row`, is drawn at least in part, and `under` whole,
/// in the visible part of the page: a row above the first row in view moves no row under
/// it, so the comparison would hold whatever the answer does.
#[track_caller]
pub(crate) fn under_form(
    window: &mut Window,
    form_row: &str,
    under: &'static str,
) -> gpui::Bounds<gpui::Pixels> {
    let page = window.cx.debug_bounds("admin-page");
    let row = window.cx.debug_bounds(sel("admin-row", form_row));
    let shown = page
        .zip(row)
        .is_some_and(|(page, row)| page.intersects(&row));
    match (shown, in_view(window, under)) {
        (true, Some(bounds)) => bounds,
        _ => panic!(
            "the row {form_row} of the form and {under} are not both in view: page {page:?}, \
             row {row:?}, under {:?}",
            window.cx.debug_bounds(under),
        ),
    }
}

/// Brings the control `form` of the form and `under` into the visible part of the page at
/// the same time and returns the bounds of `under`, as [`under_form`]. A press on `form`
/// after it then scrolls nothing, so the page moves only by what the answer does.
#[track_caller]
pub(crate) fn reveal_under_form(
    window: &mut Window,
    form: &'static str,
    form_row: &str,
    under: &'static str,
) -> gpui::Bounds<gpui::Pixels> {
    awaited(
        window,
        &format!("{form} and {under} are in view"),
        |window| {
            let control = try_reveal(window, form)?;
            if in_view(window, under).is_some() {
                return Some(());
            }
            let page = window.cx.debug_bounds("admin-page")?;
            // With the control in view, `under` is below it, drawn or not: the page moves up
            // until the bottom of `under` is at the bottom of the page, or by the space above
            // the control when `under` is not drawn.
            let distance = match window.cx.debug_bounds(under) {
                Some(bounds) => {
                    assert!(
                        bounds.top() > control.top()
                            && bounds.bottom() - control.top() <= page.size.height,
                        "{form} at {control:?} and {under} at {bounds:?} do not fit in the page \
                     at {page:?}",
                    );
                    bounds.bottom() - page.bottom()
                }
                None => {
                    assert!(
                        control.top() > page.top(),
                        "{under} is not drawn with {form} at the top of the page at {page:?}",
                    );
                    control.top() - page.top()
                }
            };
            window.simulate(|cx| {
                cx.simulate_event(ScrollWheelEvent {
                    position: page.center(),
                    delta: ScrollDelta::Pixels(point(px(0.0), -distance)),
                    ..Default::default()
                })
            });
            None
        },
    );
    under_form(window, form_row, under)
}

/// The key of the row of the open page of the administration that holds the focused
/// control.
pub(crate) fn focused_row(window: &mut Window) -> Option<String> {
    window.in_shell(|shell, window, cx| shell.admin_focused_row(window, cx))
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
pub(crate) fn open_channel_settings(window: &mut Window, name: &str, channel_id: ChannelId) {
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
    window.admin().is_some_and(|view| view.asks.is_empty())
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

/// The "everyone" role on the page of the permissions, once the rules are read.
fn everyone_role(window: &mut Window) -> Option<SubjectLine> {
    let Scope::Channel(settings) = window.admin()?.scope else {
        return None;
    };
    let settings = *settings;
    let ChannelPageView::Permissions(page) = settings.page else {
        return None;
    };
    page.rules
        .loaded()?
        .subjects
        .iter()
        .find(|line| line.kind == SubjectKind::Everyone)
        .cloned()
}

/// The identifier of the role of a line of the page of the permissions.
fn role_of(line: &SubjectLine) -> junto_client_core::protocol::ids::WorkspaceRoleId {
    match line.subject {
        junto_client_core::protocol::structure::RuleSubject::Role { role_id } => role_id,
        junto_client_core::protocol::structure::RuleSubject::Person { .. } => {
            panic!("the line is of a person")
        }
    }
}

fn permission_line(role: &SubjectLine, permission: WorkspacePermission) -> PermissionLine {
    *role
        .permissions
        .iter()
        .find(|held| held.permission == permission)
        .unwrap()
}

const fn choice_word(choice: Choice) -> &'static str {
    match choice {
        Choice::Allow => "allow",
        Choice::Neutral => "neutral",
        Choice::Deny => "deny",
    }
}

/// Sets a permission of the "everyone" role to a choice with its button, and waits until
/// the page shows the choice. The page sends nothing while the client reads the overrides
/// again after a change of the permissions, so a click that started no request is made
/// again, as a person does.
#[track_caller]
fn choose(window: &mut Window, permission: WorkspacePermission, choice: Choice) -> SubjectLine {
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
    // While the code is shown, the list does not draw the new invite (ADR 0730).
    let row = awaited(&mut window, "the state holds the new invite", |window| {
        invite_line(window, mine).filter(|_| !window.shows("invite-form"))
    });
    assert!(!window.shows(my_row));
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

    // "Done" puts the code away, and the list draws the row of the invite.
    click_in_page(&mut window, "invite-code-done");
    window.until("the code is put away", |window| {
        window.shows(my_row)
            && !window.shows("invite-code")
            && !window.shows("invite-made")
            && server_page!(window, Invites).is_some_and(|page| page.made.is_none())
            && window.admin().is_some_and(|view| view.made.is_empty())
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
fn two_waiting_requests_with_one_name_are_marked_in_the_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, _owner) = owner_window(cx, &server);

    window.open_administration();
    open_page(&mut window, "admin-nav-requests", |window| {
        server_page!(window, JoinRequests).is_some()
    });
    let (_first, _linus, first) = asks_to_join(&world, &server, "Linus");
    awaited(&mut window, "the first request has its row", |window| {
        request_line(window, first).filter(|_| window.shows(sel("request-row", first)))
    });
    assert!(!window.shows("request-same-name"));

    // A second device key with the same name: both rows say so.
    let (_second, _again, second) = asks_to_join(&world, &server, "Linus");
    let marked = awaited(&mut window, "both rows are marked", |window| {
        let both = request_line(window, first).zip(request_line(window, second));
        both.filter(|(one, two)| one.same_name && two.same_name)
    });
    assert_eq!(
        (marked.0.name.as_str(), marked.1.name.as_str()),
        ("Linus", "Linus")
    );
    window.until("the notice is drawn", |window| {
        window.shows("request-same-name")
    });

    // The first is approved: the second keeps its mark, because an account has the name now.
    click_in_page(&mut window, sel("request-approve", first));
    let left = awaited(&mut window, "the first request left the list", |window| {
        request_line(window, second).filter(|_| request_line(window, first).is_none())
    });
    assert!(left.same_name);
    window.until("the notice is still drawn", |window| {
        window.shows("request-same-name")
    });
}

/// The number that a badge draws inside the bounds of the element with this selector.
fn drawn_count(window: &mut Window, selector: &'static str) -> Option<String> {
    let text = drawn_text(window);
    let bounds = window.cx.debug_bounds(selector)?;
    text.into_iter()
        .filter(|text| bounds.contains(&text.bounds.center()))
        .map(|text| text.text.to_string())
        .find(|text| !text.is_empty() && text.chars().all(|letter| letter.is_ascii_digit()))
}

/// Finding 14: the join requests that wait are counted on the server in the server book and
/// beside the page in the navigation, without the page being open first, and an approval
/// lowers both drawn counts (ADR 0761).
#[gpui::test]
fn the_waiting_join_requests_are_counted_on_the_server_and_in_the_navigation(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    assert!(!window.shows("server-waiting-0"));

    let (_first, _linus, first) = asks_to_join(&world, &server, "Linus");
    let (_second, _marta, _) = asks_to_join(&world, &server, "Marta");
    window.until("the server book draws two requests", |window| {
        drawn_count(window, "server-waiting-0").as_deref() == Some("2")
    });

    // A press on the count opens the page of the join requests; its row in the navigation
    // draws the count too.
    window.click("server-waiting-0");
    awaited(
        &mut window,
        "the page of the join requests is open",
        |window| server_page!(window, JoinRequests),
    );
    window.until("the navigation draws two requests", |window| {
        drawn_count(window, "admin-nav-requests").as_deref() == Some("2")
    });

    // An approval lowers both drawn counts.
    click_in_page(&mut window, sel("request-approve", first));
    window.until("both places draw one request", |window| {
        drawn_count(window, "admin-nav-requests").as_deref() == Some("1")
            && drawn_count(window, "server-waiting-0").as_deref() == Some("1")
    });
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
    assert!(everyone.kind == SubjectKind::Everyone && !everyone.has_rule);
    let line = permission_line(&everyone, view_channel);
    assert_eq!((line.current, line.held), (Choice::Neutral, true));
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
    assert!(everyone.has_rule);
    assert_eq!((everyone.allowed(), everyone.denied()), (0, 1));
    until_model(
        &world,
        &member,
        "the channel is hidden from the member",
        |model| model.channel(plans).is_none(),
    );

    // "Default" removes the deny, and the channel returns.
    let everyone = choose(&mut window, view_channel, Choice::Neutral);
    assert!(!everyone.has_rule);
    until_model(
        &world,
        &member,
        "the channel is shown to the member again",
        |model| model.channel(plans).is_some(),
    );
    let stored = world.run(owner.api().channel_rules(plans)).unwrap();
    assert!(stored.rules.is_empty(), "{stored:?}");
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
    assert_eq!((line.current, line.held), (Choice::Neutral, false));
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
    assert!(window.admin().is_none_or(|view| view.asks.is_empty()));

    // "Deny" works, and it reaches the client of the owner.
    world
        .run(admin.handle.live().open_admin(AdminPart::Rules(plans)))
        .unwrap();
    let everyone = choose(&mut window, messages, Choice::Deny);
    until_model(
        &world,
        &admin.handle,
        "the deny in the state of the owner",
        |model| {
            model
                .admin
                .rules(plans)
                .loaded()
                .and_then(|rules| rules.of_role(role_of(&everyone)))
                .is_some_and(|held| held.allow.is_empty() && held.deny == [messages])
        },
    );

    // The member cannot lift the deny: "Default", "Allow" and the removal of the override
    // would give "manage messages", which the member does not hold.
    let line = permission_line(&everyone, messages);
    assert_eq!((line.current, line.held), (Choice::Deny, false));
    assert_eq!(line.refused(Choice::Neutral), Some(Refused::LiftsDeny));
    assert_eq!(line.refused(Choice::Allow), Some(Refused::LiftsDeny));
    assert_eq!(everyone.blocks_removal(), [messages]);
    window.until("the page states the reason of the deny", |window| {
        window.shows("override-manage_messages-neutral") && window.shows("override-ceiling")
    });
    for word in ["neutral", "allow"] {
        click_in_page(&mut window, sel("override-manage_messages", word));
        assert!(
            window.admin().is_none_or(|view| view.asks.is_empty()),
            "{word} sent a request"
        );
    }
    click_in_page(&mut window, sel("override-clear", role_of(&everyone)));
    assert!(window.admin().is_none_or(|view| view.asks.is_empty()));
    let stored = world.run(api.channel_rules(plans)).unwrap().rules;
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (stored[0].subject, stored[0].deny.as_slice()),
        (
            junto_client_core::protocol::structure::RuleSubject::Role {
                role_id: role_of(&everyone)
            },
            [messages].as_slice()
        )
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
    reveal_in_page(&mut window, "token-secret-copy");
    assert!(window.shows("token-secret-copy"));
    window
        .cx
        .write_to_clipboard(gpui::ClipboardItem::new_string(
            "previous clipboard value".to_owned(),
        ));
    click_in_page(&mut window, "token-secret-copy");
    let copied = window
        .cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    assert_eq!(copied, secret);
    assert_drawn_label(&mut window, "Copied");

    // The state holds the token. While the secret is shown, the list does not draw it
    // (ADR 0730).
    let token_row = sel("token-row", token_id);
    let row = awaited(&mut window, "the state holds the token", |window| {
        token_line(window, token_id)
    });
    assert!(!window.shows(token_row));
    assert_eq!(row.name, "setup");
    assert_eq!(row.actions, [TokenAction::ReadAuditLog]);
    assert_eq!(row.created_by, logged_in(&owner));
    assert!(row.own && !row.expired);
    assert!(lasts(row.created, row.expires, 7));

    // A request with the token works on a route of its action, and on no other route.
    let with_token = with_access_token(&world, &server, &copied);
    let log = world
        .run(with_token.server_audit_log(&AuditFilterQuery::default()))
        .unwrap();
    assert!(!log.entries.is_empty());
    let refused = world.run(with_token.workspaces()).unwrap_err();
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));

    // "Done" puts the secret away, and the list draws the row of the token.
    click_in_page(&mut window, "token-secret-done");
    assert!(window.shows(token_row));
    assert!(!window.shows("token-secret") && !window.shows("token-made"));
    assert!(!window.shows("token-secret-copy"));
    assert!(server_page!(window, Tokens).is_some_and(|page| page.made.is_none()));
    assert!(window.admin().is_some_and(|view| view.made.is_empty()));

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
        |window| match window.admin()?.made.as_slice() {
            [Made::Export(export_id)] if window.shows("show-exports") => Some(*export_id),
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
            page.filter.about == Some(junto_client::state::admin::AuditTarget::Account(bruno))
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
    reveal_in_page(&mut window, sel("audit-row", created.id));
    for row in &all.rows {
        for text in [row.sentence.clone(), audit_head(row)] {
            assert!(
                !holds_identifier(&text),
                "an entry shows an identifier: {text}"
            );
        }
    }
}

/// What the open page "Archive and deletion" of a channel states.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ArchivePage {
    /// The messages of the channel are permanent.
    permanent: bool,
    /// The person is the server owner.
    is_owner: bool,
    /// The person holds "manage channel" in the channel.
    manages: bool,
}

fn archive_page_of(window: &mut Window, channel_id: ChannelId) -> Option<ArchivePage> {
    match window.admin()?.scope {
        Scope::Channel(settings) if settings.channel_id == channel_id => match settings.page {
            ChannelPageView::Archive(page) => Some(ArchivePage {
                permanent: page.permanent,
                is_owner: page.is_owner,
                manages: settings.may_manage,
            }),
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

    // The member may delete a channel without permanent messages: the question opens. The
    // role reaches the window through the event stream, at a time that the test does not
    // know, and the action is disabled until then (ADR 0730).
    open_channel_settings(&mut window, "notes", notes);
    open_page(&mut window, "channel-nav-archive", |window| {
        let managed = ArchivePage {
            permanent: false,
            is_owner: false,
            manages: true,
        };
        archive_page_of(window, notes) == Some(managed)
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
        let permanent = ArchivePage {
            permanent: true,
            is_owner: false,
            manages: true,
        };
        archive_page_of(window, plans) == Some(permanent)
    });
    click_in_page(&mut window, "channel-delete");
    window.settle();
    assert!(!window.shows("channel-delete-confirm"));
    assert!(window.shows("channel-delete"));
}

/// Tab reaches "Open" and "Settings" of each archived channel for a member who may not
/// restore one: the rows hold these controls whatever the permission (ADR 0761).
#[gpui::test]
fn tab_reaches_each_archived_channel_of_a_member_who_may_not_restore(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let default = default_workspace(&world, &admin.handle);
    for name in ["notes", "plans", "drafts"] {
        let channel = text_channel(&world, &admin.handle, default, name);
        world
            .run(admin.handle.api().archive_channel(channel))
            .unwrap();
    }
    window.open_administration();
    open_page(&mut window, "admin-nav-archive", |window| {
        server_page!(window, Archive)
            .is_some_and(|page| page.channels.loaded().is_some_and(|rows| rows.len() == 3))
    });
    let page = server_page!(&mut window, Archive).unwrap();
    assert!(!page.may_manage);
    let rows = page.channels.loaded().unwrap().clone();
    let key = |at: usize| Some(format!("archived-{}", rows[at].id));

    // "Open" of the first row, then its "Settings", then "Open" and "Settings" of the
    // second row. "Restore" is disabled and takes no focus.
    window.in_shell(|shell, window, cx| shell.focus_admin_page(window, cx));
    window.until("the focus is in the first row", |window| {
        focused_row(window) == key(0)
    });
    for row in [0, 1, 1] {
        window.press("tab");
        window.until("the focus is in the next row", |window| {
            focused_row(window) == key(row)
        });
    }
    window.press("enter");
    window.until("the settings of the second channel are open", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Channel(settings)) if settings.channel_id == rows[1].id
        )
    });
}

/// The settings of a channel that a member opens without "manage channel" show the page
/// "Archive and deletion", the only one. When the role arrives, the pages of the channel
/// are offered, and the page that is shown stays (ADR 0761).
#[gpui::test]
fn a_role_that_arrives_keeps_the_shown_page_of_the_channel_settings(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.view().person.account_id.unwrap();
    let default = default_workspace(&world, &admin.handle);
    let notes = text_channel(&world, &admin.handle, default, "notes");

    // The gear asks for the overview, which the member may not use.
    open_channel_settings(&mut window, "notes", notes);
    let unmanaged = ArchivePage {
        permanent: false,
        is_owner: false,
        manages: false,
    };
    window.until("the page of the archive is shown", |window| {
        archive_page_of(window, notes) == Some(unmanaged)
    });

    grant_workspace(
        &world,
        &admin.handle,
        default,
        own,
        WorkspacePermission::ManageChannel,
    );
    window.until("the overview is offered", |window| {
        window.shows("channel-nav-overview")
    });
    let managed = ArchivePage {
        permanent: false,
        is_owner: false,
        manages: true,
    };
    assert_eq!(archive_page_of(&mut window, notes), Some(managed));
}

/// A page of the administration that the person may not use any more gives way to the page
/// of the workspaces. When the permission returns, the page of the workspaces stays (ADR 0761).
#[gpui::test]
fn a_permission_that_returns_keeps_the_shown_page_of_the_administration(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.view().person.account_id.unwrap();
    let api = admin.handle.api();
    let role = CreateServerRole {
        name: "Helpers".to_owned(),
        permissions: vec![ServerPermission::CreateInvites],
    };
    let role = world.run(api.create_server_role(&role)).unwrap().id;
    world.run(api.assign_server_role(own, role)).unwrap();

    window.open_administration();
    open_page(&mut window, "admin-nav-invites", |window| {
        server_page!(window, Invites).is_some()
    });
    world.run(api.unassign_server_role(own, role)).unwrap();
    window.until("the page of the workspaces is shown", |window| {
        server_page!(window, Workspaces).is_some() && !window.shows("admin-nav-invites")
    });

    world.run(api.assign_server_role(own, role)).unwrap();
    window.until("the page of the invites is offered again", |window| {
        window.shows("admin-nav-invites")
    });
    assert!(server_page!(window, Workspaces).is_some());
}

/// The answers to the questions before the ban and before the removal of an account close
/// each question, and the row of the account under it keeps its place (ADR 0730). The row
/// of an account stays in the list after both, marked "Banned" or "Removed".
#[gpui::test]
fn an_answer_to_a_question_about_an_account_moves_no_row_under_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let ada = logged_in(&owner);
    let _devices: Vec<Device> = ["Bruno", "Carla", "Dario"]
        .into_iter()
        .map(|name| joined(&world, &server, &invite(&world, &owner, Vec::new()), name).0)
        .collect();
    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        server_page!(window, People).is_some()
    });
    let order = awaited(&mut window, "the page lists the four accounts", |window| {
        let rows = server_page!(window, People)?.accounts.loaded()?.clone();
        (rows.len() == 4).then(|| rows.iter().map(|row| row.id).collect::<Vec<_>>())
    });
    // The first two accounts of the list that are not the owner: each has a row under it.
    let asked: Vec<(AccountId, AccountId)> = order
        .iter()
        .enumerate()
        .filter(|(_, id)| **id != ada)
        .take(2)
        .map(|(at, id)| (*id, order[at + 1]))
        .collect();

    let questions = [
        ("account-ban", "PUT", "ban", AccountStatus::Banned),
        ("account-remove", "POST", "removal", AccountStatus::Removed),
    ];
    for ((action, method, end, status), (asked, next)) in questions.into_iter().zip(asked) {
        let under = sel("account-row", next);
        click_in_page(&mut window, sel(action, asked));
        // The confirmation is brought into the visible part of the page first, so that
        // its click scrolls nothing.
        window.until(
            "the question is open and its confirmation is visible",
            |window| try_reveal(window, "account-confirm").is_some(),
        );
        let row = sel("account-row", asked);
        let form_row = format!("account-{asked}");
        let before = reveal_under_form(&mut window, "account-confirm", &form_row, under);
        let (row_before, page_before) = (
            window.cx.debug_bounds(row).unwrap(),
            window.cx.debug_bounds("admin-page").unwrap(),
        );

        // The server does the request, and its answer waits at the proxy.
        proxy.hold_answer_ending(method, &format!("/accounts/{asked}/{end}"));
        click_in_page(&mut window, "account-confirm");
        window.until(
            "the answer waits and the row has the new status",
            |window| {
                proxy.answers_waiting() == 1
                    && account(window, asked).is_some_and(|row| row.status == status)
            },
        );
        proxy.release();
        window.until("the question is closed", |window| {
            idle(window) && !window.shows("account-confirm")
        });
        assert_eq!(
            under_form(&mut window, &form_row, under),
            before,
            "the answer to {action} moved the row under the question; the row asked about \
             was at {row_before:?} and is at {:?}, the page was at {page_before:?} and is at \
             {:?}",
            window.cx.debug_bounds(row),
            window.cx.debug_bounds("admin-page"),
        );
    }
}

/// The answer to the form of a new invite closes the form and shows the code in its place,
/// and the row of an invite under the form keeps its place (ADR 0730). The row of the new
/// invite, which the state of the server brings before or after the answer, is drawn at the
/// top of the list when the person puts the code away.
#[gpui::test]
fn an_answer_to_the_form_of_a_new_invite_moves_no_control_under_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let older = world
        .run(owner.api().create_invite(&CreateInvite::default()))
        .unwrap()
        .invite
        .id;
    let under = sel("invite-revoke", older);
    window.open_administration();
    open_page(&mut window, "admin-nav-invites", |window| {
        server_page!(window, Invites).is_some()
    });
    window.until("the invite has its row", |window| window.shows(under));
    let invites = |window: &mut Window| {
        server_page!(window, Invites)
            .and_then(|page| page.invites.loaded().cloned())
            .unwrap_or_default()
    };
    for round in 0..2 {
        click_in_page(&mut window, "invite-new");
        window.until("the form of a new invite is open", |window| {
            try_reveal(window, "invite-create").is_some()
        });
        let before = reveal_under_form(&mut window, "invite-create", "invites-header", under);
        let listed = invites(&mut window).len();
        // In the first round the state of the server brings the new invite while the
        // answer waits at the proxy. In the second, the answer is not held.
        if round == 0 {
            proxy.hold_answer_ending("POST", "/invites");
        }
        click_in_page(&mut window, "invite-create");
        if round == 0 {
            window.until(
                "the answer waits and the state has the new invite",
                |window| proxy.answers_waiting() == 1 && invites(window).len() == listed + 1,
            );
            assert_eq!(under_form(&mut window, "invites-header", under), before);
            proxy.release();
        }
        window.until(
            "the code is shown, the form is closed and the state has the new invite",
            |window| {
                idle(window)
                    && window.shows("invite-code")
                    && !window.shows("invite-form")
                    && invites(window).len() == listed + 1
            },
        );
        let made = server_page!(window, Invites)
            .and_then(|page| page.made.as_ref().map(|made| made.invite_id))
            .unwrap();
        assert_eq!(
            under_form(&mut window, "invites-header", under),
            before,
            "round {round}: the new invite moved the row under the form"
        );
        assert!(!window.shows(sel("invite-row", made)));

        // The person puts the code away: the new invite is at the top of the list.
        click_in_page(&mut window, "invite-code-done");
        window.until("the new invite has its row", |window| {
            !window.shows("invite-code") && window.shows(sel("invite-row", made))
        });
        reveal_in_page(&mut window, under);
        let new_row = window.cx.debug_bounds(sel("invite-row", made)).unwrap();
        assert!(new_row.origin.y < window.cx.debug_bounds(under).unwrap().origin.y);
    }
}

/// The answer to the form of a new access token closes the form and shows the secret in
/// its place, and the row of a token under the form keeps its place (ADR 0730).
#[gpui::test]
fn an_answer_to_the_form_of_a_new_access_token_moves_no_control_under_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    let token = CreateAccessToken {
        name: "older".to_owned(),
        actions: vec![TokenAction::ReadStructure],
        expires_at: OffsetDateTime::now_utc() + Duration::days(7),
    };
    let older = world
        .run(owner.api().create_access_token(&token))
        .unwrap()
        .access_token
        .id;
    let under = sel("token-revoke", older);
    window.open_administration();
    open_page(&mut window, "admin-nav-tokens", |window| {
        server_page!(window, Tokens).is_some()
    });
    window.until("the token has its row", |window| window.shows(under));
    let tokens = |window: &mut Window| {
        server_page!(window, Tokens)
            .and_then(|page| page.tokens.loaded().cloned())
            .unwrap_or_default()
    };
    for round in 0..2 {
        click_in_page(&mut window, "token-new");
        window.until("the form of a new token is open", |window| {
            try_reveal(window, "token-create").is_some()
        });
        window.in_shell(|shell, window, cx| shell.type_token_name("setup", window, cx));
        window.settle();
        let before = reveal_under_form(&mut window, "token-create", "tokens-header", under);
        let listed = tokens(&mut window).len();
        // In the first round the state of the server brings the new token while the
        // answer waits at the proxy. In the second, the answer is not held.
        if round == 0 {
            proxy.hold_answer_ending("POST", "/access-tokens");
        }
        click_in_page(&mut window, "token-create");
        if round == 0 {
            window.until(
                "the answer waits and the state has the new token",
                |window| proxy.answers_waiting() == 1 && tokens(window).len() == listed + 1,
            );
            assert_eq!(under_form(&mut window, "tokens-header", under), before);
            proxy.release();
        }
        window.until(
            "the secret is shown, the form is closed and the state has the new token",
            |window| {
                idle(window)
                    && window.shows("token-secret")
                    && !window.shows("token-form")
                    && tokens(window).len() == listed + 1
            },
        );
        let made = server_page!(window, Tokens)
            .and_then(|page| page.made.as_ref().map(|made| made.access_token_id))
            .unwrap();
        assert_eq!(
            under_form(&mut window, "tokens-header", under),
            before,
            "round {round}: the new token moved the row under the form"
        );
        assert!(!window.shows(sel("token-row", made)));

        // The person puts the secret away: the new token is at the top of the list.
        click_in_page(&mut window, "token-secret-done");
        window.until("the new token has its row", |window| {
            !window.shows("token-secret") && window.shows(sel("token-row", made))
        });
        reveal_in_page(&mut window, under);
        let new_row = window.cx.debug_bounds(sel("token-row", made)).unwrap();
        assert!(new_row.origin.y < window.cx.debug_bounds(under).unwrap().origin.y);
    }
}

/// A page whose form adds a row at the top of its list, whose newest rows are first: the
/// invites or the access tokens (ADR 0730).
#[derive(Clone, Copy)]
struct NewRowPage {
    nav: &'static str,
    /// The key of the row of the list that holds the form.
    header: &'static str,
    new: &'static str,
    form: &'static str,
    create: &'static str,
    cancel: &'static str,
    /// The code or the secret that the answer carries, drawn in the row of the form.
    result: &'static str,
    done: &'static str,
    row: &'static str,
    revoke: &'static str,
    /// The sentence that the list has no row.
    none: &'static str,
    /// The place of that sentence while the list holds only the new row.
    none_space: &'static str,
    /// The end of the path of the request that makes a row.
    request: &'static str,
}

const INVITES: NewRowPage = NewRowPage {
    nav: "admin-nav-invites",
    header: "invites-header",
    new: "invite-new",
    form: "invite-form",
    create: "invite-create",
    cancel: "invite-cancel",
    result: "invite-code",
    done: "invite-code-done",
    row: "invite-row",
    revoke: "invite-revoke",
    none: "invites-none",
    none_space: "invites-none-space",
    request: "/invites",
};

const TOKENS: NewRowPage = NewRowPage {
    nav: "admin-nav-tokens",
    header: "tokens-header",
    new: "token-new",
    form: "token-form",
    create: "token-create",
    cancel: "token-cancel",
    result: "token-secret",
    done: "token-secret-done",
    row: "token-row",
    revoke: "token-revoke",
    none: "no-tokens",
    none_space: "no-tokens-space",
    request: "/access-tokens",
};

impl NewRowPage {
    /// The identifiers of the rows of the list in the state, when it is read.
    fn rows(self, window: &mut Window) -> Option<Vec<String>> {
        if self.nav == INVITES.nav {
            server_page!(window, Invites).and_then(|page| {
                let rows = page.invites.loaded()?;
                Some(rows.iter().map(|row| row.id.to_string()).collect())
            })
        } else {
            server_page!(window, Tokens).and_then(|page| {
                let rows = page.tokens.loaded()?;
                Some(rows.iter().map(|row| row.id.to_string()).collect())
            })
        }
    }

    /// The row that the answer made, while its code or secret is shown.
    fn made(self, window: &mut Window) -> Option<String> {
        if self.nav == INVITES.nav {
            server_page!(window, Invites)
                .and_then(|page| page.made.map(|made| made.invite_id.to_string()))
        } else {
            server_page!(window, Tokens)
                .and_then(|page| page.made.map(|made| made.access_token_id.to_string()))
        }
    }

    /// Makes a row through the API, as another client of the owner does.
    fn make(self, world: &World, owner: &ServerHandle) -> String {
        if self.nav == INVITES.nav {
            world
                .run(owner.api().create_invite(&CreateInvite::default()))
                .unwrap()
                .invite
                .id
                .to_string()
        } else {
            let token = CreateAccessToken {
                name: "older".to_owned(),
                actions: vec![TokenAction::ReadStructure],
                expires_at: OffsetDateTime::now_utc() + Duration::days(7),
            };
            world
                .run(owner.api().create_access_token(&token))
                .unwrap()
                .access_token
                .id
                .to_string()
        }
    }

    #[track_caller]
    fn open(self, window: &mut Window) {
        open_page(window, self.nav, |window| self.rows(window).is_some());
    }

    #[track_caller]
    fn open_form(self, window: &mut Window) {
        click_in_page(window, self.new);
        window.until("the form is open", |window| {
            try_reveal(window, self.create).is_some()
        });
        if self.nav == TOKENS.nav {
            window.in_shell(|shell, window, cx| shell.type_token_name("setup", window, cx));
            window.settle();
        }
    }

    /// Sends the open form and waits for the code or secret, with the new row in the state.
    /// Returns the new row.
    #[track_caller]
    fn create(self, window: &mut Window, listed: usize) -> String {
        click_in_page(window, self.create);
        window.until(
            "the result is shown, the form is closed and the state has the new row",
            |window| {
                idle(window)
                    && window.shows(self.result)
                    && !window.shows(self.form)
                    && self
                        .rows(window)
                        .is_some_and(|rows| rows.len() == listed + 1)
            },
        );
        self.made(window).unwrap()
    }
}

/// The first invite of a server and the first access token: while the code or the secret
/// of the new row is shown in the row of the form, the list states no absence of rows, and
/// it keeps the place of that sentence until the row joins it (ADR 0730).
#[gpui::test]
fn the_list_states_no_absence_of_rows_under_the_result_of_its_first_row(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.open_administration();
    for page in [INVITES, TOKENS] {
        page.open(&mut window);
        window.until("the list states that it has no row", |window| {
            window.shows(page.none)
        });
        page.open_form(&mut window);
        let none = window.cx.debug_bounds(page.none).unwrap();
        let made = page.create(&mut window, 0);
        assert!(
            !window.shows(page.none),
            "{}: the list states that it has no row under the result of its first row",
            page.nav
        );
        assert_eq!(window.cx.debug_bounds(page.none_space), Some(none));
        assert!(!window.shows(sel(page.row, &made)));

        click_in_page(&mut window, page.done);
        window.until("the new row is in the list", |window| {
            !window.shows(page.result) && window.shows(sel(page.row, &made))
        });
        assert!(!window.shows(page.none_space));
    }
}

/// The answer arrives first, the person puts the code or the secret away, and the state of
/// the server brings the new row only afterwards: the row waits for the next release of the
/// space of the form, and the row under the form keeps its place (ADR 0730).
#[gpui::test]
fn a_new_row_that_the_state_brings_after_its_result_is_put_away_moves_no_row_under_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    window.open_administration();
    for page in [INVITES, TOKENS] {
        let older = page.make(&world, &owner);
        let under = sel(page.revoke, &older);
        page.open(&mut window);
        window.until("the older row is drawn", |window| window.shows(under));
        page.open_form(&mut window);
        let listed = page.rows(&mut window).unwrap().len();
        proxy.hold_events();
        click_in_page(&mut window, page.create);
        window.until("the result is shown and the form is closed", |window| {
            idle(window) && window.shows(page.result) && !window.shows(page.form)
        });
        let made = page.made(&mut window).unwrap();
        assert_eq!(page.rows(&mut window).unwrap().len(), listed);

        click_in_page(&mut window, page.done);
        window.until("the result is put away", |window| {
            !window.shows(page.result)
        });
        let before = reveal_under_form(&mut window, page.new, page.header, under);
        proxy.release();
        window.until("the state has the new row", |window| {
            page.rows(window).unwrap_or_default().contains(&made)
        });
        // The wait read the state of the client core: a frame is drawn with that state
        // before the window is measured (ADR 0730).
        window.settle();
        assert_eq!(
            under_form(&mut window, page.header, under),
            before,
            "{}: the new row that the state brought moved the row under the form",
            page.nav
        );
        assert!(!window.shows(sel(page.row, &made)));

        // The person opens the form: the row joins the list.
        page.open_form(&mut window);
        reveal_in_page(&mut window, sel(page.row, &made));
        click_in_page(&mut window, page.cancel);
    }
}

/// Every release of the space of the form other than the Done button puts the new row into
/// the list: the form opened again, a change of the page, and the layer closed and opened
/// again (ADR 0730).
#[gpui::test]
fn the_new_row_joins_the_list_at_every_release_of_the_space_of_its_form(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.open_administration();
    for page in [INVITES, TOKENS] {
        page.open(&mut window);
        for release in ["the form opened again", "a page change", "the layer closed"] {
            page.open_form(&mut window);
            let listed = page.rows(&mut window).unwrap().len();
            let made = page.create(&mut window, listed);
            assert!(!window.shows(sel(page.row, &made)));
            match release {
                "the form opened again" => {
                    page.open_form(&mut window);
                    window.settle();
                    assert!(window.shows(page.form));
                    click_in_page(&mut window, page.cancel);
                }
                "a page change" => {
                    open_page(&mut window, "admin-nav-people", |window| {
                        server_page!(window, People).is_some()
                    });
                    page.open(&mut window);
                }
                _ => {
                    window.click("admin-close");
                    window.until("the layer is closed", |window| !window.shows("admin"));
                    window.open_administration();
                    page.open(&mut window);
                }
            }
            window.until(&format!("after {release} the new row is drawn"), |window| {
                window.shows(sel(page.row, &made))
            });
        }
    }
}

/// While the request of the form is on its way, the form stays: Cancel, the action of a row
/// under it and the page that is shown in the list of pages do nothing. The answer then
/// shows its result in the place of the form, and the row under the form keeps its place
/// (ADR 0730).
#[gpui::test]
fn the_form_of_a_new_row_stays_while_its_request_is_sent(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    window.open_administration();
    for page in [INVITES, TOKENS] {
        let older = page.make(&world, &owner);
        let under = sel(page.revoke, &older);
        page.open(&mut window);
        window.until("the older row is drawn", |window| window.shows(under));
        page.open_form(&mut window);
        let listed = page.rows(&mut window).unwrap().len();
        proxy.hold_answer_ending("POST", page.request);
        click_in_page(&mut window, page.create);
        window.until("the answer waits", |_| proxy.answers_waiting() == 1);
        let before = reveal_under_form(&mut window, page.create, page.header, under);

        for (selector, what) in [
            (page.cancel, "Cancel"),
            (under, "the action of the row under the form"),
            (page.nav, "the page that is shown"),
        ] {
            window.click(selector);
            window.settle();
            assert!(
                window.shows(page.form),
                "{}: {what} put the form away while its request was sent",
                page.nav
            );
        }
        proxy.release();
        window.until(
            "the result is shown and the state has the new row",
            |window| {
                idle(window)
                    && window.shows(page.result)
                    && page
                        .rows(window)
                        .is_some_and(|rows| rows.len() == listed + 1)
            },
        );
        window.settle();
        assert_eq!(under_form(&mut window, page.header, under), before);
        click_in_page(&mut window, page.done);
        window.until("the result is put away", |window| {
            !window.shows(page.result)
        });
    }
}

/// The person makes a second row while the first one waits for the state: the first row
/// stays out of the list while the second request is sent and after its answer, and both
/// join the list at the next release at which the state holds them (ADR 0730).
#[gpui::test]
fn a_second_new_row_keeps_the_first_one_out_of_the_list_until_a_release(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    window.open_administration();
    for page in [INVITES, TOKENS] {
        let older = page.make(&world, &owner);
        let under = sel(page.revoke, &older);
        page.open(&mut window);
        window.until("the older row is drawn", |window| window.shows(under));
        page.open_form(&mut window);
        proxy.hold_events();
        click_in_page(&mut window, page.create);
        window.until("the first result is shown", |window| {
            idle(window) && window.shows(page.result) && !window.shows(page.form)
        });
        let first = page.made(&mut window).unwrap();

        // The person opens the form again, a release while the state does not hold the
        // first row, and sends it.
        page.open_form(&mut window);
        let before = reveal_under_form(&mut window, page.create, page.header, under);
        click_in_page(&mut window, page.create);
        window.until("the second result is shown", |window| {
            idle(window)
                && !window.shows(page.form)
                && page.made(window).is_some_and(|made| made != first)
        });
        let second = page.made(&mut window).unwrap();
        proxy.release();
        window.until("the state has both new rows", |window| {
            page.rows(window)
                .is_some_and(|rows| rows.contains(&first) && rows.contains(&second))
        });
        window.settle();
        assert_eq!(
            under_form(&mut window, page.header, under),
            before,
            "{}: the first new row moved the row under the form",
            page.nav
        );
        assert!(!window.shows(sel(page.row, &first)));
        assert!(!window.shows(sel(page.row, &second)));

        click_in_page(&mut window, page.done);
        window.until("both new rows are in the list", |window| {
            window.shows(sel(page.row, &first)) && window.shows(sel(page.row, &second))
        });
    }
}

/// An administrator loses the permission while the request of a new access token is sent:
/// the form goes, the answer still settles the new row, and the person's next release
/// draws it (ADR 0730).
#[gpui::test]
fn a_token_form_that_a_lost_permission_puts_away_holds_its_row_until_a_release(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let own = window.view().person.account_id.unwrap();
    let api = admin.handle.api();
    let role = CreateServerRole {
        name: "Administrators".to_owned(),
        permissions: vec![ServerPermission::Administrator],
    };
    let role = world.run(api.create_server_role(&role)).unwrap();
    world.run(api.assign_server_role(own, role.id)).unwrap();
    window.open_administration();
    TOKENS.open(&mut window);
    window.until("the account may create a token", |window| {
        server_page!(window, Tokens).is_some_and(|page| page.may_create)
    });
    TOKENS.open_form(&mut window);
    proxy.hold_answer_ending("POST", TOKENS.request);
    click_in_page(&mut window, TOKENS.create);
    window.until("the answer waits", |_| proxy.answers_waiting() == 1);

    world.run(api.unassign_server_role(own, role.id)).unwrap();
    window.until("the form is put away", |window| {
        !window.shows(TOKENS.form)
            && server_page!(window, Tokens).is_some_and(|page| !page.may_create)
    });
    proxy.release();
    window.until("the secret is shown", |window| {
        idle(window) && window.shows(TOKENS.result)
    });
    let made = TOKENS.made(&mut window).unwrap();
    window.until("the state has the new token", |window| {
        TOKENS.rows(window).unwrap_or_default().contains(&made)
    });
    window.settle();
    assert!(!window.shows(sel(TOKENS.row, &made)));

    click_in_page(&mut window, TOKENS.done);
    window.until("the new token is in the list", |window| {
        window.shows(sel(TOKENS.row, &made))
    });
}

/// The person puts the result of the first row away while the request of a second row is
/// sent, and the state holds the first row then: the first row joins the list at that
/// press, and the answer of the second request moves no row under the form (ADR 0730).
#[gpui::test]
fn the_first_row_joins_the_list_at_its_done_while_a_second_request_is_sent(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) = owner_window_at(cx, &server, &proxy.address());
    window.open_administration();
    for page in [INVITES, TOKENS] {
        let older = page.make(&world, &owner);
        let under = sel(page.revoke, &older);
        page.open(&mut window);
        window.until("the older row is drawn", |window| window.shows(under));
        page.open_form(&mut window);
        proxy.hold_events();
        click_in_page(&mut window, page.create);
        window.until("the first result is shown", |window| {
            idle(window) && window.shows(page.result) && !window.shows(page.form)
        });
        let first = page.made(&mut window).unwrap();

        // The second request waits at the proxy; the state brings the first row.
        page.open_form(&mut window);
        proxy.hold_answer_ending("POST", page.request);
        click_in_page(&mut window, page.create);
        window.until("the second answer waits", |_| proxy.answers_waiting() == 1);
        proxy.release_events();
        window.until("the state has the first row", |window| {
            page.rows(window).unwrap_or_default().contains(&first)
        });
        window.settle();
        assert!(!window.shows(sel(page.row, &first)));

        // Done of the first result: the first row joins the list at this press.
        click_in_page(&mut window, page.done);
        reveal_in_page(&mut window, sel(page.row, &first));
        let before = reveal_under_form(&mut window, page.create, page.header, under);
        proxy.release();
        window.until(
            "the second result is shown and the state has the second row",
            |window| {
                idle(window)
                    && window.shows(page.result)
                    && page.made(window).is_some_and(|made| {
                        made != first && page.rows(window).unwrap_or_default().contains(&made)
                    })
            },
        );
        window.settle();
        let second = page.made(&mut window).unwrap();
        assert_eq!(
            under_form(&mut window, page.header, under),
            before,
            "{}: the answer of the second request moved the row under the form",
            page.nav
        );
        assert!(!window.shows(sel(page.row, &second)));
        click_in_page(&mut window, page.done);
        reveal_in_page(&mut window, sel(page.row, &second));
    }
}

/// The person opens the form while the list is read and makes a row: the read list holds
/// the new row, which waits for a release after it was made (ADR 0730).
#[gpui::test]
fn a_form_opened_before_the_list_is_read_holds_its_new_row_until_a_release(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, _owner) = owner_window_at(cx, &server, &proxy.address());
    window.open_administration();
    for page in [INVITES, TOKENS] {
        proxy.hold_request("GET", page.request);
        open_page(&mut window, page.nav, |window| window.shows(page.new));
        window.settle();
        assert!(
            page.rows(&mut window).is_none(),
            "{}: the list was read",
            page.nav
        );
        page.open_form(&mut window);
        click_in_page(&mut window, page.create);
        window.until("the result is shown", |window| {
            idle(window) && window.shows(page.result) && !window.shows(page.form)
        });
        let made = page.made(&mut window).unwrap();
        proxy.release();
        window.until("the list is read with the new row", |window| {
            page.rows(window).unwrap_or_default().contains(&made)
        });
        window.settle();
        assert!(
            !window.shows(sel(page.row, &made)),
            "{}: the new row was drawn before a release after it was made",
            page.nav
        );
        click_in_page(&mut window, page.done);
        window.until("the new row is in the list", |window| {
            window.shows(sel(page.row, &made))
        });
    }
}

/// Writes `count` entries about the server into its audit log in one statement, as the
/// server writes them: that `account` was unbanned, the newest at `newest` and each one
/// second before the one after it. A test needs more entries, or older ones, than its
/// requests would make.
fn write_audit_entries(
    world: &World,
    server: &TestServer,
    account: AccountId,
    count: i64,
    newest: OffsetDateTime,
) {
    use sqlx::ConnectOptions;
    world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url().parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query(
            "insert into audit_entries (id, occurred_at, action, event)
             select gen_random_uuid(), $2 - make_interval(secs => n), 'account_unbanned',
                    jsonb_build_object('action', 'account_unbanned', 'account_id', $1::text)
             from generate_series(0, $3 - 1) as n",
        )
        .bind(account.as_uuid())
        .bind(newest)
        .bind(count)
        .execute(&mut database)
        .await
        .unwrap();
    });
}

/// The number of entries about the server that its audit log holds.
fn server_entries(world: &World, server: &TestServer) -> usize {
    use sqlx::ConnectOptions;
    world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url().parse().unwrap();
        let mut database = options.connect().await.unwrap();
        let count: i64 =
            sqlx::query_scalar("select count(*) from audit_entries where workspace_id is null")
                .fetch_one(&mut database)
                .await
                .unwrap();
        usize::try_from(count).unwrap()
    })
}

/// The audit log of the server is read page by page with its cursor past 2,000 entries:
/// every entry once, the newest first. The cost of the pages is measured apart from the
/// cost of drawing them (ADR 0761).
#[test]
fn more_than_two_thousand_audit_entries_are_read_page_by_page_once_each_in_order() {
    let world = World::new();
    let server = world.server("Audit pages");
    let owner = world.owner(&server);
    let started = std::time::Instant::now();
    write_audit_entries(
        &world,
        &server,
        logged_in(&owner.handle),
        2_060,
        OffsetDateTime::now_utc(),
    );
    println!("inserting 2060 audit entries: {:?}", started.elapsed());
    let expected = server_entries(&world, &server);
    let started = std::time::Instant::now();
    let mut query = AuditFilterQuery {
        limit: Some(50),
        ..Default::default()
    };
    let mut ids = Vec::new();
    let mut longest = std::time::Duration::ZERO;
    loop {
        let request = std::time::Instant::now();
        let page = world
            .run(owner.handle.api().server_audit_log(&query))
            .unwrap();
        longest = longest.max(request.elapsed());
        assert!(page.entries.len() <= 50);
        ids.extend(page.entries.iter().map(|entry| entry.id));
        query.before = page.next_before;
        if query.before.is_none() {
            break;
        }
    }
    println!(
        "reading {} audit entries: {:?}, longest page {:?}",
        ids.len(),
        started.elapsed(),
        longest
    );
    assert_eq!(ids.len(), expected);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), expected);
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
}

#[gpui::test]
fn the_people_page_lays_out_only_the_accounts_in_view(cx: &mut TestAppContext) {
    use sqlx::ConnectOptions;
    let world = World::new();
    let server = world.server("People list");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    // A member whose name sorts near the start of the list, and who renames itself later.
    let code = invite(&world, &owner, Vec::new());
    let device = Device::new();
    let mover = world.run(async {
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(&code, "Member 0002b")).await.unwrap();
        logged_in(&handle);
        handle
    });
    world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url().parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query(
            "insert into accounts (id, name, status, created_at)
            select gen_random_uuid(), 'Member ' || lpad(n::text, 4, '0'), 'active', now()
            from generate_series(1, 2000) as n",
        )
        .execute(&mut database)
        .await
        .unwrap();
    });
    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        server_page!(window, People).is_some_and(|page| {
            page.accounts
                .loaded()
                .is_some_and(|rows| rows.len() == 2002)
        })
    });
    let rows = server_page!(&mut window, People)
        .unwrap()
        .accounts
        .loaded()
        .unwrap()
        .clone();
    let count = |window: &mut Window| window.in_shell(|shell, _, _| shell.admin_rows_laid_out());
    assert!(count(&mut window) < 100);

    // The scroll bar covers every account: a row not laid out has an estimated height,
    // also after a change of the width of the window.
    let extent = |window: &mut Window| window.in_shell(|shell, _, _| shell.admin_page_extent());
    let estimated = px(2000.0 * 30.0);
    assert!(extent(&mut window) > estimated, "{:?}", extent(&mut window));
    window.simulate(|cx| cx.simulate_resize(gpui::size(px(720.0), px(600.0))));
    window.settle();
    assert!(extent(&mut window) > estimated, "{:?}", extent(&mut window));

    // A row that moves from above the view moves no row in view: the member renames
    // itself and its row goes to the end of the list.
    window.in_shell(|shell, _, cx| {
        shell.scroll_admin_page_to(0.5);
        cx.notify();
    });
    window.settle();
    let seen = rows
        .iter()
        .map(|row| sel("account-row", row.id))
        .find(|selector| in_view(&mut window, selector).is_some())
        .unwrap();
    let before = in_view(&mut window, seen);
    world
        .run(mover.api().update_profile("Member 1999b"))
        .unwrap();
    window.until("the renamed member is near the end", |window| {
        server_page!(window, People).is_some_and(|page| {
            page.accounts.loaded().is_some_and(|rows| {
                rows.iter()
                    .rev()
                    .nth(1)
                    .is_some_and(|row| row.name == "Member 1999b")
            })
        })
    });
    window.settle();
    assert_eq!(in_view(&mut window, seen), before);

    let last = sel("account-row", rows.last().unwrap().id);
    assert!(!window.shows(last));
    window.in_shell(|shell, _, cx| {
        shell.scroll_admin_page_to_end();
        cx.notify();
    });
    window.until("the last account is drawn", |window| window.shows(last));
    assert!(count(&mut window) < 100);
}

/// The window reads the profiles of the accounts whose rows the list lays out, and those of
/// the rows that a scroll brings into view, and not the profiles of the other accounts of
/// a long list. A scroll back to rows whose profiles are read sends no request and draws
/// the window once (ADR 0783).
#[gpui::test]
fn the_people_page_reads_the_profiles_of_the_rows_that_it_lays_out(cx: &mut TestAppContext) {
    use sqlx::ConnectOptions;
    let world = World::new();
    let server = world.server("People profiles");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let device = Device::new();
    world.run(async {
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(&code, "Member 0002b")).await.unwrap();
        logged_in(&handle);
    });
    world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url().parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query(
            "insert into accounts (id, name, status, created_at)
            select gen_random_uuid(), 'Member ' || lpad(n::text, 4, '0'), 'active', now()
            from generate_series(1, 2000) as n",
        )
        .execute(&mut database)
        .await
        .unwrap();
    });
    let state = window.state.clone();
    let requests =
        |window: &mut Window| state.read_with(&window.cx, |state, _| state.people_requests());
    let before = requests(&mut window);
    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        server_page!(window, People).is_some_and(|page| {
            page.accounts
                .loaded()
                .is_some_and(|rows| rows.len() == 2002)
        })
    });
    window.settle();
    let server_id = window.view().selected.as_ref().unwrap().id;
    let wanted = |window: &mut Window| {
        state.read_with(&window.cx, |state, _| state.people_wanted(server_id))
    };
    let read = |window: &mut Window| {
        window.until("every profile request has its answer", |window| {
            state.read_with(&window.cx, |state, _| state.people_on_way(server_id)) == 0
        });
    };
    read(&mut window);
    let laid_out = window.in_shell(|shell, _, _| shell.admin_rows_laid_out());
    let first = wanted(&mut window);
    assert!(!first.is_empty());
    assert!(
        first.len() <= laid_out,
        "{} profiles wanted for {laid_out} rows laid out",
        first.len()
    );
    let opened = requests(&mut window) - before;
    assert!(
        opened <= u64::try_from(laid_out).unwrap(),
        "{opened} profile requests for {laid_out} rows laid out"
    );

    // A row far below the first 512 accounts comes into view: its profile is wanted.
    window.in_shell(|shell, _, cx| {
        shell.scroll_admin_page_to(0.75);
        cx.notify();
    });
    window.settle();
    let rows = server_page!(&mut window, People)
        .unwrap()
        .accounts
        .loaded()
        .unwrap()
        .clone();
    let seen = rows
        .iter()
        .position(|row| in_view(&mut window, sel("account-row", row.id)).is_some())
        .unwrap();
    assert!(seen > 512, "the first row in view is row {seen}");
    let after = wanted(&mut window);
    assert!(after.contains(&rows[seen].id), "{seen}");
    // The first account of the page is drawn only by the page.
    let top = rows
        .iter()
        .find(|row| row.name == "Member 0001")
        .unwrap()
        .id;
    assert!(first.contains(&top));
    assert!(!after.contains(&top));

    // Back at the top, the profiles are held: no request, and one drawing for the scroll.
    read(&mut window);
    let draws = |window: &mut Window| window.in_shell(|shell, _, _| shell.draws().0);
    let (drawn, asked) = (draws(&mut window), requests(&mut window));
    window.in_shell(|shell, _, cx| {
        shell.scroll_admin_page_to(0.0);
        cx.notify();
    });
    window.settle();
    assert_eq!(wanted(&mut window), first);
    assert_eq!(requests(&mut window), asked);
    assert_eq!(draws(&mut window) - drawn, 1);
}

/// While the administration is open, the rows that its list lays out are read before the
/// people of the main view, which the layer covers: the 601 members of the shown workspace
/// do not take every place of the 512 profiles (ADR 0783).
#[gpui::test]
fn the_people_page_reads_its_rows_before_the_members_of_the_shown_workspace(
    cx: &mut TestAppContext,
) {
    use sqlx::ConnectOptions;
    let world = World::new();
    let server = world.server("People first");
    // The profiles get no answer: the test reads which profiles the window wants, and each
    // answer would derive the main view of 601 members and the page of 2,001 rows again.
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    proxy.hold_request("GET", "/profile");
    world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url().parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query(
            "insert into accounts (id, name, status, created_at)
            select gen_random_uuid(), 'Member ' || lpad(n::text, 4, '0'), 'active', now()
            from generate_series(1, 2000) as n",
        )
        .execute(&mut database)
        .await
        .unwrap();
        sqlx::query(
            "insert into workspace_members (workspace_id, account_id, joined_at)
            select state.default_workspace_id, accounts.id, now()
            from accounts, server_state as state
            where accounts.name <= 'Member 0600'",
        )
        .execute(&mut database)
        .await
        .unwrap();
    });
    let (_directory, mut window, _owner) = owner_window_at(cx, &server, &proxy.address());
    window.until(
        "the main view names the members of the workspace",
        |window| {
            matches!(
                window.view().workspace().map(|workspace| &workspace.members),
                Some(Members::Loaded(members)) if members.len() == 601
            )
        },
    );
    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        server_page!(window, People).is_some_and(|page| {
            page.accounts
                .loaded()
                .is_some_and(|rows| rows.len() == 2001)
        })
    });
    window.in_shell(|shell, _, cx| {
        shell.scroll_admin_page_to(0.75);
        cx.notify();
    });
    window.settle();
    let rows = server_page!(&mut window, People)
        .unwrap()
        .accounts
        .loaded()
        .unwrap()
        .clone();
    let seen = rows
        .iter()
        .position(|row| in_view(&mut window, sel("account-row", row.id)).is_some())
        .unwrap();
    assert!(
        rows[seen].name.as_str() > "Member 0600",
        "{}",
        rows[seen].name
    );
    let server_id = window.view().selected.as_ref().unwrap().id;
    let wanted = window
        .state
        .read_with(&window.cx, |state, _| state.people_wanted(server_id));
    assert!(wanted.contains(&rows[seen].id), "row {seen}");
}

/// Presses "Load older" of the audit log, one page at a time, until `done` holds for the
/// loaded page.
fn load_older_until(
    window: &mut Window,
    done: impl Fn(&junto_client::state::admin::view::records::AuditPage) -> bool,
) -> junto_client::state::admin::view::records::AuditPage {
    loop {
        let page = awaited(window, "the page is read", |window| {
            server_page!(window, AuditLog).filter(|page| page.state != AuditRead::Reading)
        });
        assert_eq!(
            page.state,
            AuditRead::Loaded,
            "after {} entries",
            page.loaded
        );
        if done(&page) {
            return *page;
        }
        assert!(page.older, "no older entry after {} entries", page.loaded);
        let loaded = page.loaded;
        click_in_page(window, "audit-older");
        window.until(&format!("the page after {loaded} entries"), |window| {
            server_page!(window, AuditLog).is_some_and(|page| {
                page.state != AuditRead::Reading && page.loaded != loaded
                    || matches!(page.state, AuditRead::Failed { .. })
            })
        });
    }
}

#[gpui::test]
fn the_audit_log_goes_on_after_the_most_entries_that_the_page_keeps(cx: &mut TestAppContext) {
    use junto_client::state::admin::view::records::MOST_AUDIT_ENTRIES;
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    write_audit_entries(
        &world,
        &server,
        logged_in(&owner),
        2_060,
        OffsetDateTime::now_utc(),
    );
    let total = server_entries(&world, &server);
    assert!(total > MOST_AUDIT_ENTRIES);

    window.open_administration();
    open_page(&mut window, "admin-nav-audit", |window| {
        server_page!(window, AuditLog).is_some_and(|page| page.state == AuditRead::Loaded)
    });
    // "Load older" adds pages up to the most entries that the page keeps. Each page is
    // one step of the test.
    let full = load_older_until(&mut window, |page| page.at_bound);
    assert_eq!(full.loaded, MOST_AUDIT_ENTRIES);
    assert!(full.continues && !full.older && full.passed == 0);
    let laid_out = window.in_shell(|shell, _, _| shell.admin_rows_laid_out());
    assert!(laid_out < 100, "the audit page laid out {laid_out} rows");

    // The keyboard: the entries hold no control, so Tab goes from the filters to the
    // first control of the footer, "Continue", and Shift+Tab goes back.
    window.in_shell(|shell, window, cx| shell.focus_admin_page(window, cx));
    let focused = |window: &mut Window, row: &str| focused_row(window).as_deref() == Some(row);
    window.until("the focus is in the filters", |window| {
        focused(window, "audit-header")
    });
    // Shift+Tab leaves the list for the list of pages. With entries in view and no
    // control drawn, Tab enters the list again at the filters.
    window.press("shift-tab");
    window.until("the focus left the list", |window| {
        focused_row(window).is_none()
    });
    window.in_shell(|shell, _, cx| {
        shell.scroll_admin_page_to(0.5);
        cx.notify();
    });
    window.settle();
    assert!(in_view(&mut window, "audit-period").is_none());
    window.press("tab");
    window.until("Tab enters the list at the filters", |window| {
        focused(window, "audit-header")
    });
    for _ in 0..80 {
        if focused(&mut window, "audit-footer") {
            break;
        }
        window.press("tab");
        window.until("the focus is in the filters or the footer", |window| {
            focused(window, "audit-header") || focused(window, "audit-footer")
        });
    }
    let reached = (
        focused_row(&mut window),
        in_view(&mut window, "audit-continue"),
        window.cx.debug_bounds("audit-continue"),
        window.cx.debug_bounds("admin-page"),
    );
    assert!(
        reached.0.as_deref() == Some("audit-footer") && reached.1.is_some(),
        "Tab reaches the footer past 2000 rows: {reached:?}"
    );
    window.press("shift-tab");
    window.until("Shift+Tab returns to the filters", |window| {
        focused(window, "audit-header") && in_view(window, "audit-period").is_some()
    });
    window.press("tab");
    window.until("Tab returns to the footer", |window| {
        focused(window, "audit-footer") && in_view(window, "audit-continue").is_some()
    });
    assert!(window.shows("audit-bound") && !window.shows("audit-older"));
    let first_ids: Vec<_> = full.rows.iter().map(|row| row.id).collect();

    // "Continue" shows the next entries of the same list in place of the loaded ones. It
    // holds the focus and is pressed with the keyboard.
    window.press("enter");
    let next = awaited(&mut window, "the next entries are read", |window| {
        server_page!(window, AuditLog)
            .filter(|page| page.state == AuditRead::Loaded && page.passed > 0)
    });
    assert_eq!(next.passed, MOST_AUDIT_ENTRIES);
    assert!(!next.at_bound && next.loaded > 0);
    assert!(next.rows.iter().all(|row| !first_ids.contains(&row.id)));
    // The view starts at the first of the next entries.
    window.settle();
    assert!(in_view(&mut window, sel("admin-row-audit", next.rows[0].id)).is_some());
    // The rest of the list is read with "Load older": every entry of the log was shown.
    let end = load_older_until(&mut window, |page| !page.older && !page.continues);
    assert_eq!(end.passed + end.loaded, total);
    reveal_in_page(&mut window, "audit-count");

    // "Read again" returns to the newest entries.
    click_in_page(&mut window, "audit-reload");
    let newest = awaited(&mut window, "the newest entries again", |window| {
        server_page!(window, AuditLog)
            .filter(|page| page.state == AuditRead::Loaded && page.passed == 0)
    });
    assert_eq!(
        newest.rows.first().map(|row| row.id),
        first_ids.first().copied()
    );
    window.settle();
    assert!(in_view(&mut window, sel("admin-row-audit", newest.rows[0].id)).is_some());
}

#[gpui::test]
fn an_administrator_reads_the_log_of_a_deleted_workspace_from_the_entry_of_its_deletion(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let lab = world
        .run(owner.api().create_workspace("Lab".to_owned()))
        .unwrap()
        .id;
    let bench = text_channel(&world, &owner, lab, "bench");
    world.run(owner.api().delete_workspace(lab)).unwrap();

    window.open_administration();
    open_page(&mut window, "admin-nav-audit", |window| {
        server_page!(window, AuditLog).is_some_and(|page| page.state == AuditRead::Loaded)
    });
    let deletion = awaited(&mut window, "the entry of the deletion", |window| {
        server_page!(window, AuditLog)?
            .rows
            .into_iter()
            .find(|row| row.deleted_workspace.is_some())
    });
    assert_eq!(deletion.deleted_workspace, Some((lab, "Lab".to_owned())));
    click_in_page(&mut window, sel("audit-deleted-log", lab));
    let page = awaited(&mut window, "the log of the deleted workspace", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.scope == Some(AuditScope::DeletedWorkspace(lab))
                && page.state == AuditRead::Loaded
                && page.loaded > 0
        })
    });
    let scopes: Vec<AuditScope> = page.scopes.iter().map(|choice| choice.scope).collect();
    assert!(scopes.contains(&AuditScope::Server));
    assert_eq!(scopes.last(), Some(&AuditScope::DeletedWorkspace(lab)));
    assert_eq!(
        page.scopes
            .last()
            .and_then(|choice| choice.workspace.clone()),
        Some("Lab".to_owned())
    );
    // The entries of the workspace stay: the creation of its channel is among them.
    let created = page
        .rows
        .iter()
        .find(|row| row.sentence.contains("created") && row.channel.is_some())
        .unwrap();
    assert_eq!(created.channel, Some(About::Unknown));
    reveal_in_page(&mut window, sel("audit-row", created.id));
    let held = world
        .run(
            owner
                .api()
                .workspace_audit_log(lab, &AuditFilterQuery::default()),
        )
        .unwrap();
    assert_eq!(page.loaded, held.entries.len());
    assert!(
        held.entries
            .iter()
            .any(|entry| entry.channel_id == Some(bench))
    );

    // The scope of the server is chosen again with the control of the scopes.
    click_in_page(&mut window, "audit-scope-0");
    awaited(&mut window, "the log of the server again", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.scope == Some(AuditScope::Server) && page.state == AuditRead::Loaded
        })
    });
}

#[gpui::test]
fn the_audit_log_reads_a_range_of_dates_a_channel_and_names_the_access_token_of_an_action(
    cx: &mut TestAppContext,
) {
    use junto_client::state::admin::AuditTarget;
    use junto_client::state::admin::view::records::TokenRef;
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let default = default_workspace(&world, &owner);
    let plans = text_channel(&world, &owner, default, "plans");
    text_channel(&world, &owner, default, "notes");
    // One entry of a past year.
    let past = time::Date::from_calendar_date(2020, time::Month::March, 5)
        .unwrap()
        .with_hms(12, 0, 0)
        .unwrap()
        .assume_utc();
    write_audit_entries(&world, &server, logged_in(&owner), 1, past);
    // Two tokens with one name, each of which creates a channel.
    let mut tokens = Vec::new();
    for name in ["built", "rebuilt"] {
        let token = world
            .run(owner.api().create_access_token(&CreateAccessToken {
                name: "deploy".to_owned(),
                actions: vec![TokenAction::ManageStructure],
                expires_at: OffsetDateTime::now_utc() + Duration::days(7),
            }))
            .unwrap();
        let with_token = with_access_token(&world, &server, token.token.expose());
        world
            .run(with_token.create_channel(
                default,
                &CreateChannel {
                    name: name.to_owned(),
                    kind: ChannelKind::Text,
                    category_id: None,
                    position: None,
                },
            ))
            .unwrap();
        tokens.push((name, token.access_token.id));
    }

    window.open_administration();
    open_page(&mut window, "admin-nav-audit", |window| {
        server_page!(window, AuditLog).is_some_and(|page| page.state == AuditRead::Loaded)
    });

    // A range of dates: the last date before the first is not applied.
    click_in_page(&mut window, "audit-period-4");
    assert!(window.shows("audit-dates"));
    window.in_shell(|shell, window, cx| {
        shell.type_audit_dates("2020-03-09", "2020-03-01", window, cx);
    });
    click_in_page(&mut window, "audit-dates-apply");
    assert!(window.shows("audit-dates-invalid"));
    window.in_shell(|shell, window, cx| {
        shell.type_audit_dates("2020-03-05", "2020-03-05", window, cx);
    });
    window.press("enter");
    let range = awaited(&mut window, "the entries of the date", |window| {
        server_page!(window, AuditLog).filter(|page| {
            matches!(page.filter.period, AuditPeriod::Dates { .. })
                && page.state == AuditRead::Loaded
        })
    });
    assert_eq!(range.loaded, 1);
    assert!(window.shows("audit-dates-shown") && !window.shows("audit-dates"));
    click_in_page(&mut window, "audit-clear");

    // The log of the workspace, with the entries about one channel.
    click_in_page(&mut window, "audit-scope-1");
    let whole = awaited(&mut window, "the log of the workspace", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.scope == Some(AuditScope::Workspace(default))
                && page.state == AuditRead::Loaded
                && page.loaded > 0
        })
    });
    let about_notes = Some(About::Named("# notes".to_owned()));
    assert!(whole.rows.iter().any(|row| row.channel == about_notes));
    click_in_page(&mut window, "audit-about");
    let kinds = awaited(&mut window, "the kinds of things", |window| {
        let page = server_page!(window, AuditLog)?;
        window.shows("audit-about-kind").then(|| {
            page.targets
                .iter()
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>()
        })
    });
    let channels = kinds
        .iter()
        .position(|kind| *kind == junto_client::state::admin::view::records::TargetKind::Channels)
        .unwrap();
    click_in_page(&mut window, sel("audit-about-kind", channels));
    click_in_page(&mut window, sel("audit-about", plans.as_uuid()));
    let about = awaited(&mut window, "the entries about the channel", |window| {
        server_page!(window, AuditLog).filter(|page| {
            page.filter.about == Some(AuditTarget::Channel(plans))
                && page.state == AuditRead::Loaded
                && page.loaded > 0
        })
    });
    assert!(
        about
            .rows
            .iter()
            .all(|row| row.channel == Some(About::Named("# plans".to_owned())))
    );
    click_in_page(&mut window, "audit-clear");

    // The entry of each channel names its token by name and by identifier: the two
    // tokens have one name.
    for (name, token_id) in tokens {
        let row = awaited(&mut window, "the entry of the token", |window| {
            server_page!(window, AuditLog)?
                .rows
                .into_iter()
                .find(|row| row.token.is_some() && row.sentence.contains(&format!("“{name}”")))
        });
        let token = row.token.clone().unwrap();
        assert_eq!(
            token,
            TokenRef {
                id: token_id,
                name: Some("deploy".to_owned()),
            }
        );
        assert!(window.shows(sel("audit-token", row.id)));
        assert!(
            junto_client::text::admin::records::audit_token(&token).contains(&token_id.to_string())
        );
        assert!(audit_head(&row).contains("Ada with an access token"));
    }
}
