//! The pages of the people of a server (ADR 0761): the accounts with removal and ban
//! (ADR 0113), the members of the shown workspace (ADR 0115), the invites with the form of
//! a new one, and the join requests (ADR 0111).

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Role, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
};
use gpui_base::input::InputState;
use junto_client_core::api::Statement;
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::account::AccountStatus;
use junto_client_core::protocol::ids::{AccountId, InviteId, WorkspaceId};
use junto_client_core::protocol::invite::CreateInvite;
use time::OffsetDateTime;

use super::{
    Question, group_label, line, line_text, named, page_title, refusal, row_action, sentence,
};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::state::admin::view::people::{
    AccountLine, Blocked, DEFAULT_EXPIRY_DAYS, DEFAULT_USES, EXPIRY_DAYS, Grant, InviteLine,
    InvitesPage, MemberLine, MemberList, MembersPage, NewCode, NoRemoval, PeoplePage, RequestsPage,
    USES, expiry, typed_reason, typed_uses,
};
use crate::state::admin::view::{AdminView, Listed, Scope, ServerPage};
use crate::state::admin::{AdminPage, AskKind, AskState};
use crate::text;
use crate::text::admin::people as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Chip, Field, FieldEvent, FormSpace, Notice, Segmented, StyleExt, Toggle,
};

/// The width of the control that chooses which accounts are shown, in design pixels.
const FILTER_WIDTH: f32 = 260.0;
/// The width of the label before a control of the form of a new invite.
const LABEL_WIDTH: f32 = 130.0;
/// The width of the field for a typed number of uses.
const USES_WIDTH: f32 = 120.0;

/// Which accounts the page of the people shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Banned,
}

/// What a page of the people shows in place of the actions of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    /// The question before the removal of an account.
    RemoveAccount(AccountId),
    /// The question before the ban of an account.
    BanAccount(AccountId),
    /// The question before the removal of a member from the shown workspace.
    RemoveMember(AccountId),
    /// The form of a new invite.
    NewInvite,
}

/// The number of uses of a new invite: one of the offered numbers, or the typed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Uses {
    Offered(u32),
    Typed,
}

/// The request that a choice of workspaces belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantFor {
    Invite,
    Approval,
}

/// What the pages of the people hold of the window itself.
pub(crate) struct Ui {
    /// The typed number of uses of a new invite.
    pub(crate) uses: Entity<InputState>,
    /// The typed reason of the ban or of the removal that the open question asks about.
    pub(crate) reason: Entity<InputState>,
    /// The typed message to the person of that ban or removal (ADR 0120).
    pub(crate) message: Entity<InputState>,
    pub(crate) open: Option<Open>,
    pub(crate) filter: Filter,
    /// The lifetime of a new invite in days.
    pub(crate) invite_days: u32,
    pub(crate) invite_uses: Uses,
    /// The workspaces that a new invite grants. None: the default workspace.
    pub(crate) invite_workspaces: Vec<WorkspaceId>,
    /// The workspaces that an approval grants. None: the default workspace.
    pub(crate) approval_workspaces: Vec<WorkspaceId>,
    /// Why the form of a new invite was not sent.
    pub(crate) invalid: Option<&'static str>,
    /// The request that the open question or form was sent as.
    pub(crate) sent: Option<AskKind>,
    /// The space of the rows with a question or the form of a new invite (ADR 0730).
    pub(crate) space: FormSpace<Open>,
    /// The invite whose new code was copied.
    pub(crate) copied: Option<InviteId>,
}

impl Ui {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let uses = Field::state(txt::USES_PLACEHOLDER, window, cx);
        let reason = Field::state(txt::REASON_PLACEHOLDER, window, cx);
        let message = Field::state(txt::MESSAGE_PLACEHOLDER, window, cx);
        fn redraw(_: &mut Shell, event: FieldEvent, _: &mut Window, cx: &mut Context<Shell>) {
            match event {
                FieldEvent::Changed => cx.notify(),
                FieldEvent::Submitted => {}
            }
        }
        let subscriptions = vec![
            Field::observe(
                &uses,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => {
                        this.admin.people.invalid = None;
                        cx.notify();
                    }
                    FieldEvent::Submitted => this.submit_invite_form(window, cx),
                },
            ),
            // The question draws whether the typed texts can be sent.
            Field::observe(&reason, window, cx, redraw),
            Field::observe(&message, window, cx, redraw),
        ];
        let ui = Self {
            uses,
            reason,
            message,
            open: None,
            filter: Filter::All,
            invite_days: DEFAULT_EXPIRY_DAYS,
            invite_uses: Uses::Offered(DEFAULT_USES),
            invite_workspaces: Vec::new(),
            approval_workspaces: Vec::new(),
            invalid: None,
            sent: None,
            space: FormSpace::default(),
            copied: None,
        };
        (ui, subscriptions)
    }
}

/// A row with what stands under it: the statement of a refusal, a reason, the roles of
/// the account. `selector` names the row for a test.
fn framed(
    selector: String,
    row: gpui::Div,
    under: impl IntoIterator<Item = AnyElement>,
) -> AnyElement {
    div()
        .id(SharedString::from(selector.clone()))
        .flex()
        .flex_col()
        .flex_none()
        .child(named(selector, row).flex_1())
        .children(under)
        .into_any_element()
}

/// The controls of a form with their label before them.
fn labelled(label: &'static str, controls: Vec<AnyElement>, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(10.0))
        .child(
            div()
                .flex_none()
                .w(u(LABEL_WIDTH))
                .typeface(theme.types.control, theme)
                .ink(theme.colors.ink_2)
                .child(label),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(u(8.0))
                .children(controls),
        )
}

/// The marks of an account beside its name.
fn account_marks(row: &AccountLine) -> Vec<&'static str> {
    let mut marks = Vec::new();
    if row.is_owner {
        marks.push(txt::OWNER);
    }
    if row.is_self {
        marks.push(txt::YOU);
    }
    match row.status {
        AccountStatus::Active => {}
        AccountStatus::Banned => marks.push(txt::BANNED),
        AccountStatus::Removed => marks.push(txt::REMOVED),
    }
    marks
}

/// Why a request with this number of chosen workspaces cannot be sent.
fn grant_reason(grant: &Grant, chosen: usize) -> Option<&'static str> {
    if grant.workspaces.is_empty() {
        Some(txt::NO_WORKSPACE_TO_GIVE)
    } else {
        grant.fault(chosen).map(txt::grant_fault)
    }
}

impl Shell {
    /// The questions, the form and the choices of the pages of the people belong to the
    /// page that was shown.
    /// Shows the banned accounts only on the page of the people, or every account.
    pub fn show_banned_only(&mut self, banned: bool, cx: &mut Context<Self>) {
        self.admin.people.filter = if banned { Filter::Banned } else { Filter::All };
        cx.notify();
    }

    /// Sets the workspaces that the open approval of a join request grants.
    pub fn choose_approval_workspaces(
        &mut self,
        workspace_ids: Vec<WorkspaceId>,
        cx: &mut Context<Self>,
    ) {
        self.admin.people.approval_workspaces = workspace_ids;
        cx.notify();
    }

    pub(crate) fn reset_people_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.admin.people;
        ui.open = None;
        ui.filter = Filter::All;
        ui.approval_workspaces.clear();
        ui.invalid = None;
        ui.sent = None;
        ui.space.release();
        ui.copied = None;
        self.reset_invite_form(window, cx);
        self.type_reason("", window, cx);
        self.type_moderation_message("", window, cx);
    }

    /// Puts text into the field of the reason of the open question, as typing does.
    pub fn type_reason(&mut self, reason: &str, window: &mut Window, cx: &mut Context<Self>) {
        let reason = reason.to_owned();
        self.admin
            .people
            .reason
            .update(cx, |field, cx| field.set_value(reason, window, cx));
        cx.notify();
    }

    /// Puts text into the field of the message to the person of the open question, as
    /// typing does.
    pub fn type_moderation_message(
        &mut self,
        message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = message.to_owned();
        self.admin
            .people
            .message
            .update(cx, |field, cx| field.set_value(message, window, cx));
        cx.notify();
    }

    /// What the open question sends: the reason and the message to the person, or why a
    /// typed text cannot be sent.
    fn statement_typed(&self, cx: &Context<Self>) -> Result<Statement, &'static str> {
        let people = &self.admin.people;
        let reason = typed_reason(&people.reason.read(cx).value()).map_err(txt::reason_fault)?;
        let message = typed_reason(&people.message.read(cx).value()).map_err(txt::message_fault)?;
        Ok(Statement { reason, message })
    }

    /// The fields of the reason and of the message to the person under an open question,
    /// with what each text is for and why a typed text cannot be sent.
    fn render_reason(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let people = &self.admin.people;
        let reason_fault = typed_reason(&people.reason.read(cx).value())
            .err()
            .map(txt::reason_fault);
        let message_fault = typed_reason(&people.message.read(cx).value())
            .err()
            .map(txt::message_fault);
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(u(4.0))
            .mt(u(6.0))
            .mb(u(6.0))
            .debug_selector(|| "moderation-reason".to_owned())
            .child(
                Field::new(&self.admin.people.reason, txt::REASON)
                    .selector("moderation-reason-field"),
            )
            .child(sentence(txt::REASON_HELP, theme))
            .child(
                crate::ui::notice_slot("moderation-reason-invalid-slot").children(
                    reason_fault
                        .map(|fault| Notice::error(fault).selector("moderation-reason-invalid")),
                ),
            )
            .child(
                div().mt(u(6.0)).child(
                    Field::new(&self.admin.people.message, txt::MESSAGE)
                        .selector("moderation-message-field"),
                ),
            )
            .child(sentence(txt::MESSAGE_HELP, theme))
            .child(
                crate::ui::notice_slot("moderation-message-invalid-slot").children(
                    message_fault
                        .map(|fault| Notice::error(fault).selector("moderation-message-invalid")),
                ),
            )
            .into_any_element()
    }

    fn reset_invite_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.admin.people;
        ui.invite_days = DEFAULT_EXPIRY_DAYS;
        ui.invite_uses = Uses::Offered(DEFAULT_USES);
        ui.invite_workspaces.clear();
        let field = ui.uses.clone();
        field.update(cx, |field, cx| field.set_value("", window, cx));
    }

    /// Shows a question in a row or the form of a new invite, or the actions of every
    /// row again.
    pub fn set_people_open(
        &mut self,
        open: Option<Open>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.admin.people.open = open;
        self.admin.people.invalid = None;
        self.admin.people.sent = None;
        self.admin.people.space.release();
        if open == Some(Open::NewInvite) {
            self.reset_invite_form(window, cx);
        }
        // A reason and a message belong to the question that they were typed for.
        self.type_reason("", window, cx);
        self.type_moderation_message("", window, cx);
        self.state
            .update(cx, |state, cx| state.dismiss_admin_ask(cx));
        cx.notify();
    }

    /// The question or the form that was sent goes when the server did the request. A
    /// refusal keeps it, with the reason.
    fn settle_people(&mut self, view: &AdminView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(what) = self.admin.people.sent else {
            return;
        };
        match view.asked(what) {
            Some(AskState::Sending) => {}
            Some(AskState::Refused(_)) => self.admin.people.sent = None,
            None => {
                self.admin.people.sent = None;
                // The answer arrives while the person can be pointing at a control under
                // the row: the row keeps the height of its question or form.
                if let Some(open) = self.admin.people.open.take() {
                    self.admin.people.space.keep(open);
                }
                self.admin.people.invalid = None;
                self.reset_invite_form(window, cx);
            }
        }
    }

    /// A change of the state puts the open question away: its row keeps its height, as
    /// after an answer.
    fn put_away_people_open(&mut self) {
        if let Some(open) = self.admin.people.open.take() {
            self.admin.people.space.keep(open);
        }
        self.admin.people.sent = None;
    }

    /// Puts text into the field for the number of uses of a new invite, as choosing
    /// "Other" and typing does.
    pub fn type_invite_uses(&mut self, uses: &str, window: &mut Window, cx: &mut Context<Self>) {
        let uses = uses.to_owned();
        self.admin.people.invite_uses = Uses::Typed;
        self.admin
            .people
            .uses
            .update(cx, |field, cx| field.set_value(uses, window, cx));
        cx.notify();
    }

    /// Sends the form of a new invite.
    fn submit_invite_form(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.admin.people.open != Some(Open::NewInvite) {
            return;
        }
        let Some(view) = self.state.read(cx).admin_view() else {
            return;
        };
        let Scope::Server {
            page: ServerPage::Invites(page),
            ..
        } = &view.scope
        else {
            return;
        };
        if view.sending(AskKind::CreateInvite) {
            return;
        }
        let ui = &self.admin.people;
        let max_uses = match ui.invite_uses {
            Uses::Offered(uses) => Some(uses),
            Uses::Typed => typed_uses(&ui.uses.read(cx).value()),
        };
        let workspace_ids = page.grant.kept(&ui.invite_workspaces);
        let days = ui.invite_days;
        let Some(max_uses) = max_uses else {
            self.admin.people.invalid = Some(txt::USES_RANGE);
            cx.notify();
            return;
        };
        if let Some(reason) = grant_reason(&page.grant, workspace_ids.len()) {
            self.admin.people.invalid = Some(reason);
            cx.notify();
            return;
        }
        let invite = CreateInvite {
            expires_at: expiry(days, OffsetDateTime::now_utc()),
            max_uses: Some(max_uses),
            workspace_ids,
        };
        let server_id = view.server_id;
        self.admin.people.invalid = None;
        self.admin.people.sent = Some(AskKind::CreateInvite);
        self.state
            .update(cx, |state, cx| state.create_invite(server_id, invite, cx));
        cx.notify();
    }

    /// A question in the row of an account, with its two answers. The confirmation
    /// carries the typed reason and message; a text that cannot be sent holds the
    /// confirmation.
    fn render_account_question(
        &mut self,
        question: Question,
        theme: &Theme,
        cx: &mut Context<Self>,
        confirmed: impl Fn(&mut Self, Statement, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Div {
        let invalid = self.statement_typed(cx).is_err();
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(u(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink)
                    .child(question.sentence),
            )
            .child(
                Button::new("people-cancel")
                    .label(text::CANCEL)
                    .selector(question.cancel_selector)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_people_open(None, window, cx);
                    })),
            )
            .child(
                Button::new("people-confirm")
                    .kind(ButtonKind::Primary)
                    .label(question.confirm)
                    .selector(question.confirm_selector)
                    .disabled(question.sending || invalid)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Ok(statement) = this.statement_typed(cx) {
                            confirmed(this, statement, window, cx);
                        }
                    })),
            )
    }

    fn render_account_row(
        &mut self,
        view: &AdminView,
        row: &AccountLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let account_id = row.id;
        // A question whose action is not possible any more is put away for good: it does
        // not return when the action becomes possible again.
        // While the order of the roles is read again the question stays, and its
        // answer waits.
        let gone = |action: Result<(), Blocked>| {
            action.is_err_and(|reason| reason != Blocked::OrderNotRead)
        };
        let reading = |action: Result<(), Blocked>| action == Err(Blocked::OrderNotRead);
        let stale = match self.admin.people.open {
            Some(Open::RemoveAccount(id)) => id == account_id && gone(row.remove),
            Some(Open::BanAccount(id)) => id == account_id && gone(row.ban),
            _ => false,
        };
        if stale {
            // A change of the state closes the question: the row keeps its height, as
            // after an answer.
            self.put_away_people_open();
        }
        let content: AnyElement = match self.admin.people.open {
            // A question stays only while its action is possible: the order of the roles
            // or the status of the account can change while it is open.
            Some(Open::RemoveAccount(id)) if id == account_id && !gone(row.remove) => self
                .render_account_question(
                    Question {
                        sentence: txt::remove_question(&row.name).into(),
                        confirm: txt::REMOVE,
                        confirm_selector: "account-confirm",
                        cancel_selector: "account-cancel",
                        sending: view.sending(AskKind::RemoveAccount(account_id))
                            || reading(row.remove),
                    },
                    theme,
                    cx,
                    move |this, reason, _, cx| {
                        this.admin.people.sent = Some(AskKind::RemoveAccount(account_id));
                        this.state.update(cx, |state, cx| {
                            state.remove_account(server_id, account_id, reason, cx);
                        });
                    },
                )
                .into_any_element(),
            Some(Open::BanAccount(id)) if id == account_id && !gone(row.ban) => self
                .render_account_question(
                    Question {
                        sentence: txt::ban_question(&row.name).into(),
                        confirm: txt::BAN,
                        confirm_selector: "account-confirm",
                        cancel_selector: "account-cancel",
                        sending: view.sending(AskKind::BanAccount(account_id)) || reading(row.ban),
                    },
                    theme,
                    cx,
                    move |this, reason, _, cx| {
                        this.admin.people.sent = Some(AskKind::BanAccount(account_id));
                        this.state.update(cx, |state, cx| {
                            state.ban_account(server_id, account_id, reason, cx);
                        });
                    },
                )
                .into_any_element(),
            _ => {
                let mut content = div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        crate::shell::people::avatar_of(
                            &self.faces(server_id),
                            account_id,
                            &row.name,
                            false,
                            26.0,
                        )
                        .radius(2.0),
                    )
                    .child(line_text(
                        row.name.clone(),
                        Some(txt::account_detail(&row.roles, row.created).into()),
                        theme,
                    ));
                for mark in account_marks(row) {
                    content = content.child(Chip::new(mark));
                }
                if row.assignable.is_some() {
                    let shown = self.admin.roles.account == Some(account_id);
                    content = content.child(row_action(
                        ("account-roles", account_id.as_uuid().as_u128() as u64),
                        if shown {
                            text::admin::roles::HIDE_ROLES
                        } else {
                            text::admin::roles::ROLES
                        },
                        format!("account-roles-toggle-{account_id}"),
                        None,
                        cx.listener(move |this, _, _, cx| {
                            this.show_account_roles((!shown).then_some(account_id), cx);
                        }),
                    ));
                }
                // A removed account takes no action: its state is the mark of the row.
                if row.status != AccountStatus::Removed {
                    content = content.child(row_action(
                        ("account-remove", account_id.as_uuid().as_u128() as u64),
                        txt::REMOVE,
                        format!("account-remove-{account_id}"),
                        row.remove.err().map(txt::blocked),
                        cx.listener(move |this, _, window, cx| {
                            this.set_people_open(Some(Open::RemoveAccount(account_id)), window, cx);
                        }),
                    ));
                }
                match row.status {
                    AccountStatus::Active => {
                        content = content.child(row_action(
                            ("account-ban", account_id.as_uuid().as_u128() as u64),
                            txt::BAN,
                            format!("account-ban-{account_id}"),
                            row.ban.err().map(txt::blocked),
                            cx.listener(move |this, _, window, cx| {
                                this.set_people_open(
                                    Some(Open::BanAccount(account_id)),
                                    window,
                                    cx,
                                );
                            }),
                        ));
                    }
                    AccountStatus::Banned => {
                        let held = row.unban.err().map(txt::blocked).or(view
                            .sending(AskKind::UnbanAccount(account_id))
                            .then_some(text::admin::SENDING));
                        content = content.child(row_action(
                            ("account-unban", account_id.as_uuid().as_u128() as u64),
                            txt::UNBAN,
                            format!("account-unban-{account_id}"),
                            held,
                            cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.unban_account(server_id, account_id, cx);
                                });
                            }),
                        ));
                    }
                    AccountStatus::Removed => {}
                }
                content.into_any_element()
            }
        };
        let refused = view
            .refused(AskKind::RemoveAccount(account_id))
            .or(view.refused(AskKind::BanAccount(account_id)))
            .or(view.refused(AskKind::UnbanAccount(account_id)));
        let mut under: Vec<AnyElement> = Vec::new();
        if matches!(
            self.admin.people.open,
            Some(Open::RemoveAccount(id) | Open::BanAccount(id)) if id == account_id
        ) {
            under.push(self.render_reason(theme, cx));
        }
        // The server keeps the account banned for the person: the row states it without a
        // pointer (ADR 0311).
        if row.unban == Err(Blocked::NotUnbannable) {
            under.push(
                sentence(txt::NOT_UNBANNABLE, theme)
                    .mt(u(3.0))
                    .mb(u(4.0))
                    .pl(u(34.0))
                    .ink(theme.colors.ink)
                    .debug_selector(move || format!("account-not-unbannable-{account_id}"))
                    .into_any_element(),
            );
        }
        // Role order protects the account: the row states it without a pointer
        // (ADR 0309).
        if row.outranked() {
            under.push(
                sentence(txt::OUTRANKED, theme)
                    .mt(u(3.0))
                    .mb(u(4.0))
                    .pl(u(34.0))
                    .ink(theme.colors.ink)
                    .debug_selector(move || format!("account-outranked-{account_id}"))
                    .into_any_element(),
            );
        }
        if let Some(choices) = &row.assignable
            && self.admin.roles.account == Some(account_id)
        {
            under.push(self.render_role_choices(view, account_id, &row.name, choices, theme, cx));
        }
        under.extend(refusal(refused, "account-refused"));
        let modes = [
            Open::RemoveAccount(account_id),
            Open::BanAccount(account_id),
        ];
        self.admin
            .people
            .space
            .row(
                self.admin.people.open,
                &modes,
                framed(
                    format!("account-row-{account_id}"),
                    line(theme).child(content),
                    under,
                ),
            )
            .into_any_element()
    }

    pub(crate) fn render_people_page(
        &mut self,
        view: &AdminView,
        page: &PeoplePage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_people(view, window, cx);
        let theme = theme(cx);
        let filter = self.admin.people.filter;
        let choice = Segmented::new(
            "people-filter",
            txt::FILTER,
            [
                SharedString::from(txt::FILTER_ALL),
                SharedString::from(txt::filter_banned(page.banned)),
            ],
        )
        .selected(Some(match filter {
            Filter::All => 0,
            Filter::Banned => 1,
        }))
        .selector("people-filter")
        .on_select({
            let select = cx.listener(|this, index: &usize, _, cx| {
                this.admin.people.filter = if *index == 1 {
                    Filter::Banned
                } else {
                    Filter::All
                };
                cx.notify();
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::People),
                txt::PEOPLE_HELP,
                &theme,
            ))
            .child(
                named(
                    "people-filter".to_owned(),
                    div().flex().w(u(FILTER_WIDTH)).child(choice),
                )
                .mb(u(6.0)),
            );
        match &page.accounts {
            Listed::Loading => {
                column = column.child(self.render_reading(
                    view.server_id,
                    AdminPart::Accounts,
                    None,
                    &theme,
                    cx,
                ));
            }
            Listed::Failed(failure) => {
                column = column.child(self.render_reading(
                    view.server_id,
                    AdminPart::Accounts,
                    Some(*failure),
                    &theme,
                    cx,
                ));
            }
            Listed::Loaded(rows) => {
                let shown: Vec<&AccountLine> = rows
                    .iter()
                    .filter(|row| filter == Filter::All || row.status == AccountStatus::Banned)
                    .collect();
                if shown.is_empty() {
                    let none = match filter {
                        Filter::All => txt::NO_ACCOUNTS,
                        Filter::Banned => txt::NO_BANNED,
                    };
                    column = column.child(
                        sentence(none, &theme)
                            .py(u(10.0))
                            .debug_selector(|| "accounts-none".to_owned()),
                    );
                }
                for row in shown {
                    column = column.child(self.render_account_row(view, row, &theme, cx));
                }
            }
        }
        column
            .child(sentence(txt::REMOVAL_HELP, &theme).mt(u(12.0)))
            .child(sentence(txt::BAN_HELP, &theme).mt(u(4.0)))
            .into_any_element()
    }

    /// The question before the removal of a member from the shown workspace, in the row
    /// of the member. Absent when the row shows no question. A question whose removal is
    /// not possible any more is put away: the order of the roles can change while it is
    /// open. While the order is read again the question stays, and its answer waits.
    fn render_member_question(
        &mut self,
        view: &AdminView,
        page: &MembersPage,
        row: &MemberLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let account_id = row.account_id;
        if self.admin.people.open != Some(Open::RemoveMember(account_id)) {
            return None;
        }
        let reading = row.remove == Some(Err(NoRemoval::OrderNotRead));
        if !matches!(row.remove, Some(Ok(()))) && !reading {
            self.put_away_people_open();
            return None;
        }
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        Some(self.render_account_question(
            Question {
                sentence: txt::remove_member_question(&row.name, &page.workspace_name).into(),
                confirm: txt::REMOVE,
                confirm_selector: "member-remove-confirm",
                cancel_selector: "member-remove-cancel",
                sending: view.sending(AskKind::RemoveMember(account_id)) || reading,
            },
            theme,
            cx,
            move |this, reason, _, cx| {
                this.admin.people.sent = Some(AskKind::RemoveMember(account_id));
                this.state.update(cx, |state, cx| {
                    state.remove_workspace_member(server_id, workspace_id, account_id, reason, cx);
                });
            },
        ))
    }

    pub(crate) fn render_members_page(
        &mut self,
        view: &AdminView,
        page: &MembersPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_people(view, window, cx);
        let theme = theme(cx);
        let server_id = view.server_id;
        let faces = self.faces(server_id);
        let workspace_id = page.workspace_id;
        let mut column = div().flex().flex_col().child(page_title(
            text::admin::page_name(AdminPage::Members),
            txt::members_help(&page.workspace_name),
            &theme,
        ));
        match &page.members {
            MemberList::Loading => {
                column = column.child(
                    sentence(text::admin::READING, &theme)
                        .py(u(10.0))
                        .debug_selector(|| "members-reading".to_owned()),
                );
            }
            MemberList::Failed => {
                column = column.child(
                    div().flex_none().py(u(8.0)).child(
                        Notice::error(txt::MEMBERS_NOT_READ)
                            .selector("members-not-read")
                            .action(
                                Button::new("members-read-again")
                                    .label(text::TRY_AGAIN)
                                    .selector("members-read-again")
                                    .h(u(28.0))
                                    .px(u(10.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.reload_members(cx);
                                        });
                                    })),
                            ),
                    ),
                );
            }
            MemberList::Loaded(rows) => {
                if rows.is_empty() {
                    column = column.child(sentence(txt::NO_MEMBERS, &theme).py(u(10.0)));
                }
                for row in rows {
                    let account_id = row.account_id;
                    let modes = [Open::RemoveMember(account_id)];
                    if let Some(question) = self.render_member_question(view, page, row, &theme, cx)
                    {
                        let mut under = vec![self.render_reason(&theme, cx)];
                        under.extend(refusal(
                            view.refused(AskKind::RemoveMember(account_id)),
                            "member-remove-refused",
                        ));
                        let framed = framed(
                            format!("member-row-{account_id}"),
                            line(&theme).child(question),
                            under,
                        );
                        let open = self.admin.people.open;
                        column = column.child(self.admin.people.space.row(open, &modes, framed));
                        continue;
                    }
                    let roles =
                        (!row.roles.is_empty()).then(|| SharedString::from(row.roles.join(", ")));
                    let mut content = line(&theme)
                        .child(
                            crate::shell::people::avatar_of(
                                &faces,
                                row.account_id,
                                &row.name,
                                row.pictured,
                                26.0,
                            )
                            .radius(2.0),
                        )
                        .child(line_text(row.name.clone(), roles, &theme));
                    if row.is_self {
                        content = content.child(Chip::new(txt::YOU));
                    }
                    let mut under: Vec<AnyElement> = Vec::new();
                    // Role order protects the member: the row states it without a
                    // pointer (ADR 0309).
                    if row.remove == Some(Err(NoRemoval::Outranked)) {
                        under.push(
                            sentence(txt::MEMBER_OUTRANKED, &theme)
                                .mt(u(3.0))
                                .mb(u(4.0))
                                .pl(u(34.0))
                                .ink(theme.colors.ink)
                                .debug_selector(move || format!("member-outranked-{account_id}"))
                                .into_any_element(),
                        );
                    }
                    under.extend(refusal(
                        view.refused(AskKind::RemoveMember(account_id)),
                        "member-remove-refused",
                    ));
                    if let Some(choices) = &row.assignable {
                        let shown = self.admin.roles.account == Some(account_id);
                        content = content.child(row_action(
                            ("member-roles", account_id.as_uuid().as_u128() as u64),
                            if shown {
                                text::admin::roles::HIDE_ROLES
                            } else {
                                text::admin::roles::ROLES
                            },
                            format!("member-roles-toggle-{account_id}"),
                            None,
                            cx.listener(move |this, _, _, cx| {
                                this.show_account_roles((!shown).then_some(account_id), cx);
                            }),
                        ));
                        if shown {
                            under.push(self.render_role_choices(
                                view, account_id, &row.name, choices, &theme, cx,
                            ));
                        }
                    }
                    if let Some(remove) = row.remove {
                        content = content.child(row_action(
                            ("member-remove", account_id.as_uuid().as_u128() as u64),
                            txt::REMOVE_FROM_WORKSPACE,
                            format!("member-remove-{account_id}"),
                            remove.err().map(txt::no_removal),
                            cx.listener(move |this, _, window, cx| {
                                this.set_people_open(
                                    Some(Open::RemoveMember(account_id)),
                                    window,
                                    cx,
                                );
                            }),
                        ));
                    }
                    let framed = framed(format!("member-row-{account_id}"), content, under);
                    let open = self.admin.people.open;
                    column = column.child(self.admin.people.space.row(open, &modes, framed));
                }
            }
        }
        match (&page.addable, &page.members) {
            (None, _) => {
                column = column
                    .child(sentence(txt::ADD_NEEDS, &theme).mt(u(12.0)))
                    .child(sentence(txt::REMOVE_MEMBER_NEEDS, &theme).mt(u(4.0)));
            }
            // The accounts that can be added are known when the members are.
            (Some(_), MemberList::Loading | MemberList::Failed) => {}
            (Some(addable), MemberList::Loaded(_)) => {
                column = column
                    .child(group_label(txt::NOT_MEMBERS, &theme))
                    .child(sentence(txt::ADD_HELP, &theme).mb(u(4.0)));
                match addable {
                    Listed::Loading => {
                        column = column.child(self.render_reading(
                            server_id,
                            AdminPart::Accounts,
                            None,
                            &theme,
                            cx,
                        ));
                    }
                    Listed::Failed(failure) => {
                        column = column.child(self.render_reading(
                            server_id,
                            AdminPart::Accounts,
                            Some(*failure),
                            &theme,
                            cx,
                        ));
                    }
                    Listed::Loaded(rows) => {
                        if rows.is_empty() {
                            column =
                                column.child(sentence(txt::ALL_ARE_MEMBERS, &theme).py(u(10.0)));
                        }
                        for row in rows {
                            let account_id = row.account_id;
                            let what = AskKind::AddMember(account_id);
                            let content = line(&theme)
                                .child(
                                    crate::shell::people::avatar_of(
                                        &faces, account_id, &row.name, false, 26.0,
                                    )
                                    .radius(2.0),
                                )
                                .child(line_text(row.name.clone(), None, &theme))
                                .child(row_action(
                                    ("member-add", account_id.as_uuid().as_u128() as u64),
                                    txt::ADD,
                                    format!("member-add-{account_id}"),
                                    view.sending(what).then_some(text::admin::SENDING),
                                    cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.add_workspace_member(
                                                server_id,
                                                workspace_id,
                                                account_id,
                                                cx,
                                            );
                                        });
                                    }),
                                ));
                            column = column.child(framed(
                                format!("member-addable-{account_id}"),
                                content,
                                refusal(view.refused(what), "member-refused"),
                            ));
                        }
                    }
                }
            }
        }
        column.into_any_element()
    }

    /// One row per workspace that the person may grant, with the switch that chooses it.
    fn render_grant(
        &mut self,
        grant: &Grant,
        purpose: GrantFor,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (chosen, id) = match purpose {
            GrantFor::Invite => (&self.admin.people.invite_workspaces, "invite-workspace"),
            GrantFor::Approval => (&self.admin.people.approval_workspaces, "request-workspace"),
        };
        let chosen = grant.kept(chosen);
        let mut column = div().flex().flex_col().flex_none();
        for choice in &grant.workspaces {
            let workspace_id = choice.id;
            let toggle = Toggle::new(
                (id, workspace_id.as_uuid().as_u128() as u64),
                choice.name.clone(),
                chosen.contains(&workspace_id),
                (text::ON, text::OFF),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let ui = &mut this.admin.people;
                let chosen = match purpose {
                    GrantFor::Invite => &mut ui.invite_workspaces,
                    GrantFor::Approval => &mut ui.approval_workspaces,
                };
                match chosen.iter().position(|held| *held == workspace_id) {
                    Some(at) => {
                        chosen.remove(at);
                    }
                    None => chosen.push(workspace_id),
                }
                ui.invalid = None;
                cx.notify();
            }));
            let mark = choice
                .is_default
                .then(|| SharedString::from(txt::DEFAULT_WORKSPACE));
            column = column.child(
                line(theme)
                    .child(line_text(choice.name.clone(), mark, theme))
                    .child(named(format!("{id}-{workspace_id}"), toggle)),
            );
        }
        if chosen.is_empty()
            && let Some(default) = grant.default_workspace()
        {
            column = column.child(sentence(txt::default_is_given(&default.name), theme).mt(u(5.0)));
        }
        column
    }

    /// The code of a new invite. No later answer of the server carries it.
    fn render_new_code(
        &mut self,
        made: &NewCode,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let invite_id = made.invite_id;
        let code = made.code.clone();
        let shown = SharedString::from(made.code.expose().to_owned());
        let copied = self.admin.people.copied == Some(invite_id);
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .child(
                div()
                    .id("invite-code")
                    .role(Role::Label)
                    // The code is no label: a label is kept by the accessibility tree.
                    .aria_label(txt::NEW_CODE_LABEL)
                    .debug_selector(|| "invite-code".to_owned())
                    .flex_none()
                    .typeface(theme.types.mono_code, theme)
                    .ink(theme.colors.ink)
                    .child(shown),
            )
            .child(
                Button::new("invite-code-copy")
                    .label(if copied { txt::COPIED } else { txt::COPY })
                    .selector("invite-code-copy")
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(code.expose().to_owned()));
                        this.admin.people.copied = Some(invite_id);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("invite-code-done")
                    .kind(ButtonKind::Primary)
                    .label(txt::DONE)
                    .selector("invite-code-done")
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.admin.people.copied = None;
                        this.state
                            .update(cx, |state, cx| state.dismiss_admin_made(cx));
                    })),
            );
        div()
            .flex_none()
            .mb(u(4.0))
            .child(
                Notice::new(txt::CODE_ONCE)
                    .selector("invite-made")
                    .action(actions),
            )
            .into_any_element()
    }

    /// The form of a new invite: its lifetime, its number of uses and its workspaces.
    fn render_invite_form(
        &mut self,
        view: &AdminView,
        grant: &Grant,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let days = self.admin.people.invite_days;
        let uses = self.admin.people.invite_uses;
        let chosen = grant.kept(&self.admin.people.invite_workspaces);
        let held = grant_reason(grant, chosen.len());
        let expiry = Segmented::new(
            "invite-expiry",
            txt::EXPIRY,
            EXPIRY_DAYS.map(|days| SharedString::from(txt::expiry_days(days))),
        )
        .selected(EXPIRY_DAYS.iter().position(|offered| *offered == days))
        .selector("invite-expiry")
        .on_select({
            let select = cx.listener(|this, index: &usize, _, cx| {
                if let Some(days) = EXPIRY_DAYS.get(*index).copied() {
                    this.admin.people.invite_days = days;
                    cx.notify();
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let number = Segmented::new(
            "invite-uses",
            txt::USES,
            USES.iter()
                .map(|uses| SharedString::from(uses.to_string()))
                .chain([SharedString::from(txt::USES_OTHER)]),
        )
        .selected(match uses {
            Uses::Offered(uses) => USES.iter().position(|offered| *offered == uses),
            Uses::Typed => Some(USES.len()),
        })
        .selector("invite-uses")
        .on_select({
            let select = cx.listener(|this, index: &usize, window, cx| {
                match USES.get(*index).copied() {
                    Some(uses) => this.admin.people.invite_uses = Uses::Offered(uses),
                    None => {
                        this.admin.people.invite_uses = Uses::Typed;
                        this.focus_field(&this.admin.people.uses.clone(), window, cx);
                    }
                }
                this.admin.people.invalid = None;
                cx.notify();
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let mut number = vec![number.into_any_element()];
        if uses == Uses::Typed {
            number.push(
                div()
                    .flex_none()
                    .w(u(USES_WIDTH))
                    .child(
                        Field::new(&self.admin.people.uses, txt::USES)
                            .selector("invite-uses-field"),
                    )
                    .into_any_element(),
            );
        }
        let create = Button::new("invite-create")
            .kind(ButtonKind::Primary)
            .label(txt::CREATE_INVITE)
            .selector("invite-create")
            .disabled(held.is_some() || view.sending(AskKind::CreateInvite))
            .h(u(30.0))
            .px(u(12.0))
            .on_click(cx.listener(|this, _, window, cx| {
                this.submit_invite_form(window, cx);
            }));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(u(8.0))
            .debug_selector(|| "invite-form".to_owned())
            .child(labelled(
                txt::EXPIRY,
                vec![expiry.into_any_element()],
                theme,
            ))
            .child(labelled(txt::USES, number, theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(
                        div()
                            .typeface(theme.types.control, theme)
                            .ink(theme.colors.ink_2)
                            .child(txt::WORKSPACES),
                    )
                    .child(sentence(txt::INVITE_GRANTS_HELP, theme))
                    .child(self.render_grant(grant, GrantFor::Invite, theme, cx)),
            )
            .child(
                crate::ui::notice_slot("invite-form-invalid-slot").children(
                    self.admin
                        .people
                        .invalid
                        .map(|invalid| Notice::error(invalid).selector("invite-form-invalid")),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        Button::new("invite-cancel")
                            .label(text::CANCEL)
                            .selector("invite-cancel")
                            .h(u(30.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_people_open(None, window, cx);
                            })),
                    )
                    .child(match held {
                        Some(reason) => create.tooltip(reason),
                        None => create,
                    })
                    .children(held.map(|reason| sentence(reason, theme))),
            )
            .into_any_element()
    }

    fn render_invite_row(
        &mut self,
        view: &AdminView,
        row: &InviteLine,
        new: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let invite_id = row.id;
        let what = AskKind::RevokeInvite(invite_id);
        let mut content = line(theme).child(
            line_text(
                txt::invite_creator(row.creator.as_deref()),
                Some(txt::invite_times(row.created, row.expires).into()),
                theme,
            )
            .child(sentence(
                txt::invite_detail(
                    row.uses,
                    row.max_uses,
                    &row.workspaces,
                    row.other_workspaces,
                ),
                theme,
            )),
        );
        if new {
            content = content.child(Chip::new(txt::NEW_CODE_MARK));
        }
        content = content.child(Chip::new(txt::invite_state(row.state)));
        if row.may_revoke {
            content = content.child(row_action(
                ("invite-revoke", invite_id.as_uuid().as_u128() as u64),
                txt::REVOKE,
                format!("invite-revoke-{invite_id}"),
                view.sending(what).then_some(text::admin::SENDING),
                cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.revoke_invite(server_id, invite_id, cx);
                    });
                }),
            ));
        }
        framed(
            format!("invite-row-{invite_id}"),
            content,
            refusal(view.refused(what), "invite-refused"),
        )
    }

    pub(crate) fn render_invites_page(
        &mut self,
        view: &AdminView,
        page: &InvitesPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_people(view, window, cx);
        let theme = theme(cx);
        let mut column = div().flex().flex_col().child(page_title(
            text::admin::page_name(AdminPage::Invites),
            txt::INVITES_HELP,
            &theme,
        ));
        column = column.child(group_label(txt::NEW_INVITE, &theme));
        // The code that the answer carries is drawn in the row of the form, in the place of
        // the form, so that the answer moves no control under the row (ADR 0730).
        let code = page
            .made
            .as_ref()
            .map(|made| self.render_new_code(made, &theme, cx));
        let open = self.admin.people.open;
        let new = match open {
            Some(Open::NewInvite) => self.render_invite_form(view, &page.grant, &theme, cx),
            _ => div()
                .flex()
                .flex_none()
                .child(
                    Button::new("invite-new")
                        .icon(Icon::Plus)
                        .label(txt::NEW_INVITE)
                        .selector("invite-new")
                        .h(u(30.0))
                        .px(u(12.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_people_open(Some(Open::NewInvite), window, cx);
                        })),
                )
                .into_any_element(),
        };
        column = column.child(self.admin.people.space.row(
            open,
            &[Open::NewInvite],
            div().flex().flex_col().children(code).child(new),
        ));
        column = column
            .children(refusal(
                view.refused(AskKind::CreateInvite),
                "invite-create-refused",
            ))
            .child(group_label(txt::INVITES, &theme));
        match &page.invites {
            Listed::Loading => {
                column = column.child(self.render_reading(
                    view.server_id,
                    AdminPart::Invites,
                    None,
                    &theme,
                    cx,
                ));
            }
            Listed::Failed(failure) => {
                column = column.child(self.render_reading(
                    view.server_id,
                    AdminPart::Invites,
                    Some(*failure),
                    &theme,
                    cx,
                ));
            }
            Listed::Loaded(rows) => {
                if rows.is_empty() {
                    column = column.child(
                        sentence(txt::NO_INVITES, &theme)
                            .py(u(10.0))
                            .debug_selector(|| "invites-none".to_owned()),
                    );
                }
                let new = page.made.as_ref().map(|made| made.invite_id);
                for row in rows {
                    column = column.child(self.render_invite_row(
                        view,
                        row,
                        new == Some(row.id),
                        &theme,
                        cx,
                    ));
                }
            }
        }
        column.into_any_element()
    }

    pub(crate) fn render_requests_page(
        &mut self,
        view: &AdminView,
        page: &RequestsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_people(view, window, cx);
        let theme = theme(cx);
        let server_id = view.server_id;
        let chosen = page.grant.kept(&self.admin.people.approval_workspaces);
        let held = grant_reason(&page.grant, chosen.len());
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::JoinRequests),
                txt::REQUESTS_HELP,
                &theme,
            ))
            .child(group_label(txt::APPROVAL_WORKSPACES, &theme))
            .child(sentence(txt::APPROVAL_GRANTS_HELP, &theme))
            .child(self.render_grant(&page.grant, GrantFor::Approval, &theme, cx))
            .children(held.map(|reason| sentence(reason, &theme).mt(u(5.0))))
            .child(group_label(txt::WAITING, &theme));
        match &page.requests {
            Listed::Loading => {
                column = column.child(self.render_reading(
                    server_id,
                    AdminPart::JoinRequests,
                    None,
                    &theme,
                    cx,
                ));
            }
            Listed::Failed(failure) => {
                column = column.child(self.render_reading(
                    server_id,
                    AdminPart::JoinRequests,
                    Some(*failure),
                    &theme,
                    cx,
                ));
            }
            Listed::Loaded(rows) => {
                if rows.is_empty() {
                    column = column.child(
                        sentence(txt::NO_REQUESTS, &theme)
                            .py(u(10.0))
                            .debug_selector(|| "requests-none".to_owned()),
                    );
                }
                for row in rows {
                    let request_id = row.id;
                    let (approve, deny) = (
                        AskKind::ApproveRequest(request_id),
                        AskKind::DenyRequest(request_id),
                    );
                    let sending = (view.sending(approve) || view.sending(deny))
                        .then_some(text::admin::SENDING);
                    let workspace_ids = chosen.clone();
                    let content = line(&theme)
                        .child(line_text(
                            row.name.clone(),
                            Some(txt::request_times(row.created, row.expires).into()),
                            &theme,
                        ))
                        .child(row_action(
                            ("request-deny", request_id.as_uuid().as_u128() as u64),
                            txt::DENY,
                            format!("request-deny-{request_id}"),
                            sending,
                            cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.deny_join_request(server_id, request_id, cx);
                                });
                            }),
                        ))
                        .child(row_action(
                            ("request-approve", request_id.as_uuid().as_u128() as u64),
                            txt::APPROVE,
                            format!("request-approve-{request_id}"),
                            held.or(sending),
                            cx.listener(move |this, _, _, cx| {
                                let workspace_ids = workspace_ids.clone();
                                this.state.update(cx, |state, cx| {
                                    state.approve_join_request(
                                        server_id,
                                        request_id,
                                        workspace_ids,
                                        cx,
                                    );
                                });
                            }),
                        ));
                    column = column.child(framed(
                        format!("request-row-{request_id}"),
                        content,
                        refusal(
                            view.refused(approve).or(view.refused(deny)),
                            "request-refused",
                        ),
                    ));
                }
            }
        }
        column.into_any_element()
    }
}
