//! The pages of the records of a server (ADR 0761): the access tokens with the form of a
//! new one (ADR 0210), the exports of the person with their files (ADR 0411), and the
//! audit log with its filters (ADR 0303, ADR 0305).

use super::list::Row;
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    div,
};
use gpui_base::input::InputState;
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::access_token::TokenAction;
use junto_client_core::protocol::ids::{AccessTokenId, ExportId, ServerId};
use time::OffsetDateTime;

use super::{group_label, line, named, page_title, refusal, row_action, sentence};
use crate::shell::Shell;
use crate::shell::room::option_row;
use crate::state::admin::records::AuditReadKind;
use crate::state::admin::view::records::{
    AuditGroup, AuditLine, AuditPage, AuditRead, DEFAULT_TOKEN_EXPIRY_DAYS, ExportLine, ExportSave,
    ExportStatus, ExportsPage, NewToken, TOKEN_EXPIRY_DAYS, TargetKind, TokenLine, TokensPage,
    group_of_action, new_token,
};
use crate::state::admin::view::{AdminView, Listed, Scope, ServerPage};
use crate::state::admin::{AdminPage, AskKind, AskState, AuditFilter, AuditPeriod, AuditScope};
use crate::text;
use crate::text::admin::records as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Chip, Field, FieldEvent, FormSpace, NewRow, Notice, Segmented, SelectBox,
    StyleExt, Toggle,
};

/// The width of the label before a control of a form or of a filter, in design pixels.
const LABEL_WIDTH: f32 = 130.0;
/// The width of the box that shows the choice of a filter of the audit log.
const FILTER_WIDTH: f32 = 260.0;

/// The list of choices that the page of the audit log shows under a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditMenu {
    /// The actions of one group.
    Action(AuditGroup),
    Actor,
    /// The things of one kind that an entry can concern.
    About(TargetKind),
    /// The form of a range of dates.
    Dates,
}

/// The actions that the form of a new access token has on when it opens: the one that
/// changes nothing on the server.
const FIRST_ACTIONS: [TokenAction; 1] = [TokenAction::ReadStructure];

/// What a page of the records shows in place of the actions of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Open {
    /// The form of a new access token.
    NewToken,
    /// The question before the removal of an export.
    RemoveExport(ExportId),
}

/// What the pages of the records hold of the window itself.
pub(crate) struct Ui {
    /// The name of a new access token.
    pub(crate) token_name: Entity<InputState>,
    /// The page shows the form of a new access token.
    pub(crate) token_form: bool,
    pub(crate) token_actions: Vec<TokenAction>,
    /// The lifetime of a new access token in days.
    pub(crate) token_days: u32,
    /// Why the form of a new access token was not sent.
    pub(crate) token_invalid: Option<&'static str>,
    /// The form was sent, and the server has not answered or has done it.
    pub(crate) token_sent: bool,
    /// The new access token whose text was copied.
    pub(crate) copied: Option<AccessTokenId>,
    /// The export whose row shows the question before its removal.
    pub(crate) removing: Option<ExportId>,
    /// The removal was confirmed, and the server has not answered or has done it.
    pub(crate) remove_sent: bool,
    /// The space of the rows with the form of a new access token or the question before
    /// the removal of an export (ADR 0730).
    pub(crate) space: FormSpace<Open>,
    /// The token that the form of a new access token makes, which waits for the release of
    /// the space before it is drawn in the list (ADR 0730).
    pub(crate) new_token: NewRow<AccessTokenId>,
    /// The log whose first read the page of the audit log asked for.
    pub(crate) audit_asked: Option<(ServerId, AuditScope)>,
    /// The filter of the audit log whose choices the page shows.
    pub(crate) audit_menu: Option<AuditMenu>,
    /// The first and the last date of a range of the audit log, as typed.
    pub(crate) audit_from: Entity<InputState>,
    pub(crate) audit_to: Entity<InputState>,
    /// Why the typed dates were not applied.
    pub(crate) dates_invalid: Option<&'static str>,
}

impl Ui {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let token_name = Field::state(txt::TOKEN_NAME, window, cx);
        let audit_from = Field::state(txt::DATES_FROM, window, cx);
        let audit_to = Field::state(txt::DATES_TO, window, cx);
        let mut subscriptions = vec![Field::observe(
            &token_name,
            window,
            cx,
            |this: &mut Shell, event, window, cx| match event {
                FieldEvent::Changed => {
                    this.admin.records.token_invalid = None;
                    cx.notify();
                }
                FieldEvent::Submitted => this.submit_token_form(window, cx),
            },
        )];
        for field in [&audit_from, &audit_to] {
            subscriptions.push(Field::observe(
                field,
                window,
                cx,
                |this: &mut Shell, event, _, cx| match event {
                    FieldEvent::Changed => {
                        this.admin.records.dates_invalid = None;
                        cx.notify();
                    }
                    FieldEvent::Submitted => this.apply_audit_dates(cx),
                },
            ));
        }
        let ui = Self {
            token_name,
            token_form: false,
            token_actions: FIRST_ACTIONS.to_vec(),
            token_days: DEFAULT_TOKEN_EXPIRY_DAYS,
            token_invalid: None,
            token_sent: false,
            copied: None,
            removing: None,
            remove_sent: false,
            space: FormSpace::default(),
            new_token: NewRow::default(),
            audit_asked: None,
            audit_menu: None,
            audit_from,
            audit_to,
            dates_invalid: None,
        };
        (ui, subscriptions)
    }
}

/// A row with what belongs under it. `selector` names the row for a test.
fn framed(selector: String, row: gpui::Div, under: Vec<AnyElement>) -> AnyElement {
    div()
        .id(SharedString::from(selector.clone()))
        .flex()
        .flex_col()
        .flex_none()
        .child(named(selector, row).flex_1())
        .children(under)
        .into_any_element()
}

/// A control of a form or of a filter with its label before it.
fn labelled(label: &'static str, control: AnyElement, theme: &Theme) -> gpui::Div {
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
        .child(div().flex().flex_1().min_w_0().child(control))
}

/// The name of a row with lines under it. A line takes as many lines of text as it
/// needs.
fn lines(name: String, details: Vec<String>, theme: &Theme) -> gpui::Div {
    let mut column = div().flex_1().min_w_0().child(
        div()
            .typeface(theme.types.control_strong, theme)
            .ink(theme.colors.ink)
            .truncate()
            .child(name),
    );
    for detail in details {
        column = column.child(
            div()
                .typeface(theme.types.small, theme)
                .ink(theme.colors.ink_2)
                .child(detail),
        );
    }
    column
}

/// The row of an entry, with an action of the row at its right.
fn audit_row(row: &AuditLine, action: Option<AnyElement>, theme: &Theme) -> AnyElement {
    let id = row.id;
    let content = line(theme).child(
        div()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .child(txt::audit_head(row)),
            )
            .child(
                div()
                    .typeface(theme.types.control, theme)
                    .ink(theme.colors.ink)
                    .child(row.sentence.clone()),
            )
            // The reason is text of the person who acted: it takes the lines that it
            // needs and is not cut.
            .children(row.token.as_ref().map(|token| {
                div()
                    .debug_selector(move || format!("audit-token-{id}"))
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .child(txt::audit_token(token))
            }))
            .children(row.reason.as_ref().map(|reason| {
                div()
                    .debug_selector(move || format!("audit-reason-{id}"))
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink)
                    .child(txt::audit_reason(reason))
            }))
            .children(row.message.as_ref().map(|message| {
                div()
                    .debug_selector(move || format!("audit-message-{id}"))
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink)
                    .child(txt::audit_message(message))
            })),
    );
    framed(
        format!("audit-row-{}", row.id),
        content.children(action),
        Vec::new(),
    )
}

impl Shell {
    /// The form, the question and the choices of the pages of the records belong to the
    /// page that was shown.
    pub(crate) fn reset_records_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.admin.records;
        ui.token_form = false;
        ui.token_invalid = None;
        ui.token_sent = false;
        ui.copied = None;
        ui.removing = None;
        ui.remove_sent = false;
        ui.space.release();
        self.admin.list.state.remeasure();
        ui.new_token.reset();
        ui.audit_asked = None;
        ui.audit_menu = None;
        ui.dates_invalid = None;
        for field in [ui.audit_from.clone(), ui.audit_to.clone()] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.reset_token_form(window, cx);
    }

    fn reset_token_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.admin.records;
        ui.token_actions = FIRST_ACTIONS.to_vec();
        ui.token_days = DEFAULT_TOKEN_EXPIRY_DAYS;
        let field = ui.token_name.clone();
        field.update(cx, |field, cx| field.set_value("", window, cx));
    }

    /// Shows the form of a new access token, or puts it away.
    pub fn set_token_form(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.admin.records.token_form = open;
        self.admin.records.token_invalid = None;
        self.admin.records.token_sent = false;
        self.admin.records.space.release();
        self.admin.list.state.remeasure();
        self.admin.records.new_token.release();
        self.reset_token_form(window, cx);
        self.state
            .update(cx, |state, cx| state.dismiss_admin_ask(cx));
        if open {
            self.focus_field(&self.admin.records.token_name.clone(), window, cx);
        }
        cx.notify();
    }

    /// Puts text into the field for the name of a new access token, as typing does.
    pub fn type_token_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.admin
            .records
            .token_name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        cx.notify();
    }

    /// Sends the form of a new access token. The expiry counts from this moment.
    fn submit_token_form(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.admin.records.token_form {
            return;
        }
        let Some(view) = self.state.read(cx).admin_view() else {
            return;
        };
        if view.sending(AskKind::CreateToken) {
            return;
        }
        let server_id = view.server_id;
        let name = self.admin.records.token_name.read(cx).value().to_string();
        let asked = new_token(
            &name,
            &self.admin.records.token_actions,
            self.admin.records.token_days,
            OffsetDateTime::now_utc(),
        );
        match asked {
            Ok(token) => {
                let listed = match &view.scope {
                    Scope::Server {
                        page: ServerPage::Tokens(page),
                        ..
                    } => page
                        .tokens
                        .loaded()
                        .map(|rows| rows.iter().map(|row| row.id).collect()),
                    _ => None,
                };
                self.admin.records.new_token.send(listed);
                self.admin.records.token_invalid = None;
                self.admin.records.token_sent = true;
                self.state.update(cx, |state, cx| {
                    state.create_access_token(server_id, token, cx);
                });
            }
            Err(fault) => self.admin.records.token_invalid = Some(txt::token_fault(fault)),
        }
        cx.notify();
    }

    /// The form that was sent goes when the server created the token. A refusal keeps it,
    /// with the reason.
    fn settle_token_form(&mut self, view: &AdminView, window: &mut Window, cx: &mut Context<Self>) {
        // The new token follows its request, also when its form went while it was sent: a
        // person who is no administrator any more gets no form.
        if self.admin.records.new_token.waits() {
            match view.asked(AskKind::CreateToken) {
                Some(AskState::Sending) => {}
                Some(AskState::Refused(_)) => self.admin.records.new_token.answer(None),
                None => {
                    let made = match &view.scope {
                        Scope::Server {
                            page: ServerPage::Tokens(page),
                            ..
                        } => page.made.as_ref().map(|made| made.access_token_id),
                        _ => None,
                    };
                    self.admin.records.new_token.answer(made);
                }
            }
        }
        if !self.admin.records.token_sent {
            return;
        }
        match view.asked(AskKind::CreateToken) {
            Some(AskState::Sending) => {}
            Some(AskState::Refused(_)) => self.admin.records.token_sent = false,
            None => {
                self.admin.records.token_sent = false;
                // The answer arrives while the person can be pointing at a control under
                // the form: its row keeps the height of the form.
                self.put_away_token_form();
                self.admin.records.token_invalid = None;
                self.reset_token_form(window, cx);
            }
        }
    }

    /// The form of a new access token closes: its row keeps the height of the form.
    fn put_away_token_form(&mut self) {
        let ui = &mut self.admin.records;
        if ui.token_form {
            ui.token_form = false;
            ui.token_sent = false;
            ui.space.keep(Open::NewToken, text::admin::done::TOKEN_MADE);
        }
    }

    /// Shows the question before the removal of an export in its row, or the actions of
    /// every row again.
    pub fn set_export_removing(&mut self, export_id: Option<ExportId>, cx: &mut Context<Self>) {
        self.admin.records.removing = export_id;
        self.admin.records.remove_sent = false;
        self.admin.records.space.release();
        self.admin.list.state.remeasure();
        self.admin.records.new_token.release();
        self.state
            .update(cx, |state, cx| state.dismiss_admin_ask(cx));
        cx.notify();
    }

    /// The question that was confirmed goes when the server removed the export. A
    /// refusal keeps it, with the reason.
    fn settle_export_question(&mut self, view: &AdminView) {
        let ui = &mut self.admin.records;
        let (Some(export_id), true) = (ui.removing, ui.remove_sent) else {
            return;
        };
        match view.asked(AskKind::RemoveExport(export_id)) {
            Some(AskState::Sending) => {}
            Some(AskState::Refused(_)) => ui.remove_sent = false,
            None => {
                ui.remove_sent = false;
                ui.removing = None;
                // The row of the export stays until the state of the server removes it:
                // until then it keeps the height of its question.
                ui.space.keep(Open::RemoveExport(export_id), text::admin::done::EXPORT_REMOVED);
            }
        }
    }

    /// The secret of a new access token. No later answer of the server carries it.
    fn render_new_token(
        &mut self,
        made: &NewToken,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let token_id = made.access_token_id;
        // The elements hold the token as the text that is drawn and in no other place:
        // the copy reads it from the state when the person asks for it.
        let shown = SharedString::from(made.token.expose().to_owned());
        let copied = self.admin.records.copied == Some(token_id);
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .child(
                Button::new("token-secret-copy")
                    .label(if copied { txt::COPIED } else { txt::COPY })
                    .selector("token-secret-copy")
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(secret) = this.state.read(cx).new_token_text(token_id) else {
                            return;
                        };
                        cx.write_to_clipboard(ClipboardItem::new_string(secret));
                        this.admin.records.copied = Some(token_id);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("token-secret-done")
                    .kind(ButtonKind::Primary)
                    .label(txt::DONE)
                    .selector("token-secret-done")
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        // The person puts the secret away: the row of the form gives up its
                        // height and the new token takes its place in the list.
                        this.admin.records.copied = None;
                        this.admin.records.space.release();
                        this.admin.list.state.remeasure();
                        this.admin.records.new_token.release();
                        this.state.update(cx, |state, cx| {
                            state.dismiss_admin_made(AskKind::CreateToken, cx);
                        });
                    })),
            );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .mb(u(4.0))
            .child(
                Notice::new(txt::TOKEN_ONCE)
                    .selector("token-made")
                    .action(actions),
            )
            .child(
                div()
                    .id("token-secret")
                    .role(Role::Label)
                    .aria_label(txt::NEW_TOKEN_LABEL)
                    .debug_selector(|| "token-secret".to_owned())
                    .flex_none()
                    .mt(u(6.0))
                    .py(u(6.0))
                    .px(u(9.0))
                    .rounded(u(theme.metrics.radius))
                    .border_1()
                    .border_color(Hsla::from(theme.colors.rule))
                    .bg(Hsla::from(theme.colors.travertine_hi))
                    .typeface(theme.types.mono_code, theme)
                    .ink(theme.colors.ink)
                    .child(shown),
            )
            .into_any_element()
    }

    /// The form of a new access token: its name, its actions and its lifetime.
    fn render_token_form(
        &mut self,
        view: &AdminView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let days = self.admin.records.token_days;
        let expiry = Segmented::new(
            "token-expiry",
            txt::TOKEN_EXPIRY,
            TOKEN_EXPIRY_DAYS.map(|days| SharedString::from(txt::token_days(days))),
        )
        .selected(
            TOKEN_EXPIRY_DAYS
                .iter()
                .position(|offered| *offered == days),
        )
        .selector("token-expiry")
        .on_select({
            let select = cx.listener(|this, index: &usize, _, cx| {
                if let Some(days) = TOKEN_EXPIRY_DAYS.get(*index).copied() {
                    this.admin.records.token_days = days;
                    cx.notify();
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let mut column = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(u(8.0))
            .debug_selector(|| "token-form".to_owned())
            .child(labelled(
                txt::TOKEN_NAME,
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Field::new(&self.admin.records.token_name, txt::TOKEN_NAME)
                            .selector("token-name"),
                    )
                    .into_any_element(),
                theme,
            ))
            .child(sentence(txt::TOKEN_NAME_HELP, theme))
            .child(
                div()
                    .flex_none()
                    .typeface(theme.types.control, theme)
                    .ink(theme.colors.ink_2)
                    .child(txt::TOKEN_ACTIONS),
            );
        for action in TokenAction::ALL.iter().copied() {
            let on = self.admin.records.token_actions.contains(&action);
            let selector = txt::token_action_selector(action);
            let title = txt::token_action_title(action);
            column = column.child(
                option_row(
                    title,
                    txt::token_action_help(action),
                    Toggle::new(selector, title, on, (text::ON, text::OFF))
                        .selector(selector)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let chosen = &mut this.admin.records.token_actions;
                            if on {
                                chosen.retain(|held| *held != action);
                            } else if !chosen.contains(&action) {
                                chosen.push(action);
                            }
                            this.admin.records.token_invalid = None;
                            cx.notify();
                        }))
                        .into_any_element(),
                    theme,
                )
                .px(u(0.0)),
            );
        }
        column = column
            .child(labelled(
                txt::TOKEN_EXPIRY,
                expiry.into_any_element(),
                theme,
            ))
            .child(sentence(txt::TOKEN_EXPIRY_HELP, theme))
            .child(
                crate::ui::notice_slot("token-form-invalid-slot").children(
                    self.admin
                        .records
                        .token_invalid
                        .map(|invalid| Notice::error(invalid).selector("token-form-invalid")),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        Button::new("token-cancel")
                            .label(text::CANCEL)
                            .selector("token-cancel")
                            .disabled(view.sending(AskKind::CreateToken))
                            .h(u(30.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_token_form(false, window, cx);
                            })),
                    )
                    .child(
                        Button::new("token-create")
                            .kind(ButtonKind::Primary)
                            .label(txt::CREATE_TOKEN)
                            .selector("token-create")
                            .disabled(view.sending(AskKind::CreateToken))
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_token_form(window, cx);
                            })),
                    ),
            );
        column.into_any_element()
    }

    fn render_token_row(
        &mut self,
        view: &AdminView,
        row: &TokenLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let token_id = row.id;
        let what = AskKind::RevokeToken(token_id);
        let mut content = line(theme).child(lines(
            row.name.clone(),
            vec![
                txt::token_actions(&row.actions),
                txt::token_detail(row),
                txt::token_identifier(token_id),
            ],
            theme,
        ));
        if row.expired {
            content = content.child(Chip::new(txt::EXPIRED));
        }
        content = content.child(row_action(
            ("token-revoke", token_id.as_uuid().as_u128() as u64),
            txt::REVOKE,
            format!("token-revoke-{token_id}"),
            if view.sending(what) {
                Some(text::admin::SENDING)
            } else {
                view.sending(AskKind::CreateToken)
                    .then_some(txt::TOKEN_BEING_MADE)
            },
            cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.revoke_access_token(server_id, token_id, cx);
                });
            }),
        ));
        framed(
            format!("token-row-{token_id}"),
            content,
            refusal(view.refused(what), "token-revoke-refused")
                .into_iter()
                .collect(),
        )
    }

    pub(crate) fn render_tokens_page(
        &mut self,
        view: &AdminView,
        page: &TokensPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_token_form(view, window, cx);
        if !page.may_create {
            self.put_away_token_form();
        }
        let rows: Vec<TokenLine> = page
            .tokens
            .loaded()
            .map(|listed| {
                self.admin
                    .records
                    .new_token
                    .drawn(listed, |row| row.id)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let list = std::iter::once(Row::new("tokens-header", true).part())
            .chain(
                rows.iter()
                    .map(|row| Row::new(format!("token-{}", row.id), true)),
            )
            .chain(std::iter::once(Row::new("tokens-footer", false).part()))
            .collect();
        self.admin.list.take(Some(view), list, window, cx);
        let view = view.clone();
        let page = page.clone();
        self.render_admin_list(
            move |this, index, window, cx| {
                let theme = theme(cx);
                if index == 0 {
                    return this.render_tokens_header(&view, &page, &rows, window, cx);
                }
                if let Some(row) = rows.get(index - 1) {
                    return this.render_token_row(&view, row, &theme, cx);
                }
                if page.more {
                    sentence(txt::more_tokens(rows.len()), &theme)
                        .mt(u(6.0))
                        .debug_selector(|| "tokens-more".to_owned())
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            },
            window,
            cx,
        )
    }

    fn render_tokens_header(
        &mut self,
        view: &AdminView,
        page: &TokensPage,
        rows: &[TokenLine],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = view.server_id;
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::Tokens),
                txt::tokens_help(page.may_create),
                &theme,
            ))
            .child(sentence(txt::TOKEN_ACTS_AS, &theme).mb(u(6.0)));
        // A new token.
        column = column.child(group_label(txt::NEW_TOKEN, &theme));
        // A person who is no administrator any more gets no form, also when it was open:
        // the form is put away for good, and its row keeps its height.
        // The secret that the answer carries is drawn in the row of the form, in the place
        // of the form, so that the answer moves no control under the row (ADR 0730).
        let secret = page
            .made
            .as_ref()
            .map(|made| self.render_new_token(made, &theme, cx));
        let open = self.admin.records.token_form.then_some(Open::NewToken);
        let new = div().flex().flex_col().children(secret);
        let new = if open.is_some() {
            new.child(self.render_token_form(view, &theme, cx))
        } else {
            let held = (!page.may_create).then_some(txt::TOKEN_NEEDS);
            let button = Button::new("token-new")
                .icon(crate::assets::Icon::Plus)
                .label(txt::NEW_TOKEN)
                .selector("token-new")
                .disabled(held.is_some())
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_token_form(true, window, cx);
                }));
            new.child(div().flex().flex_none().child(button))
                .children(held.map(|reason| {
                    sentence(reason, &theme)
                        .mt(u(5.0))
                        .debug_selector(|| "token-new-held".to_owned())
                }))
        };
        column = column.child(self.admin.records.space.row(open, &[Open::NewToken], new));
        column = column.children(refusal(
            view.refused(AskKind::CreateToken),
            "token-create-refused",
        ));
        // The tokens of the server, or the own ones.
        column = column.child(group_label(txt::tokens_label(page.may_create), &theme));
        let part = AdminPart::AccessTokens;
        match &page.tokens {
            Listed::Loading => {
                column = column.child(self.render_reading(server_id, part, None, &theme, cx));
            }
            Listed::Failed(failure) => {
                column =
                    column.child(self.render_reading(server_id, part, Some(*failure), &theme, cx));
            }
            Listed::Loaded(listed) => {
                let none = sentence(txt::no_tokens(page.may_create), &theme).py(u(6.0));
                if listed.is_empty() {
                    column = column.child(none.debug_selector(|| "no-tokens".to_owned()));
                } else if rows.is_empty() {
                    // The list holds only the new token, which waits for the release of the
                    // space of its form: the place of the sentence stays, without it.
                    column = column.child(
                        none.invisible()
                            .debug_selector(|| "no-tokens-space".to_owned()),
                    );
                }
            }
        }
        column.into_any_element()
    }

    /// The question before the removal of an export, in its row.
    fn render_export_question(
        &mut self,
        view: &AdminView,
        row: &ExportLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let server_id = view.server_id;
        let export_id = row.id;
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
                    .child(txt::remove_export_question(&row.of)),
            )
            .child(
                Button::new("export-cancel")
                    .label(text::CANCEL)
                    .selector("export-cancel")
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_export_removing(None, cx);
                    })),
            )
            .child(
                Button::new("export-confirm")
                    .kind(ButtonKind::Primary)
                    .label(txt::REMOVE)
                    .selector("export-confirm")
                    .disabled(view.sending(AskKind::RemoveExport(export_id)))
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.admin.records.remove_sent = true;
                        this.state.update(cx, |state, cx| {
                            state.remove_export(server_id, export_id, cx);
                        });
                    })),
            )
    }

    fn render_export_row(
        &mut self,
        view: &AdminView,
        row: &ExportLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let export_id = row.id;
        let number = export_id.as_uuid().as_u128() as u64;
        let content = if self.admin.records.removing == Some(export_id) {
            line(theme).child(self.render_export_question(view, row, theme, cx))
        } else {
            let mut content = line(theme).child(lines(
                txt::export_of(&row.of),
                vec![
                    txt::export_status(row.status).to_owned(),
                    txt::export_detail(row),
                ],
                theme,
            ));
            if row.status == ExportStatus::Ready {
                content = content.child(row_action(
                    ("export-save", number),
                    txt::SAVE,
                    format!("export-save-{export_id}"),
                    (!row.can_save).then_some(txt::SAVING_NOW),
                    cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.save_export(server_id, export_id, cx);
                        });
                    }),
                ));
            }
            content.child(row_action(
                ("export-remove", number),
                txt::REMOVE,
                format!("export-remove-{export_id}"),
                None,
                cx.listener(move |this, _, _, cx| {
                    this.set_export_removing(Some(export_id), cx);
                }),
            ))
        };
        let mut under: Vec<AnyElement> = Vec::new();
        match &row.save {
            Some(ExportSave::Saving { written }) => under.push(
                named(
                    format!("export-progress-{export_id}"),
                    sentence(txt::save_progress(*written, row.bytes), theme).py(u(4.0)),
                )
                .into_any_element(),
            ),
            Some(ExportSave::Saved(path)) => under.push(
                named(
                    format!("export-saved-{export_id}"),
                    sentence(txt::saved_as(path), theme).py(u(4.0)),
                )
                .into_any_element(),
            ),
            Some(ExportSave::Failed(reason)) => under.push(
                div()
                    .flex_none()
                    .mt(u(6.0))
                    .child(Notice::error(reason.clone()).selector("export-save-failed"))
                    .into_any_element(),
            ),
            None => {}
        }
        let mut under = vec![
            crate::ui::notice_slot("export-save-slot")
                .children(under)
                .into_any_element(),
        ];
        under.extend(refusal(
            view.refused(AskKind::RemoveExport(export_id)),
            "export-remove-refused",
        ));
        let open = self.admin.records.removing.map(Open::RemoveExport);
        self.admin
            .records
            .space
            .row(
                open,
                &[Open::RemoveExport(export_id)],
                framed(format!("export-row-{export_id}"), content, under),
            )
            .into_any_element()
    }

    pub(crate) fn render_exports_page(
        &mut self,
        view: &AdminView,
        page: &ExportsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_export_question(view);
        let rows = page.exports.loaded().cloned().unwrap_or_default();
        let list = std::iter::once(Row::new("exports-header", true).part())
            .chain(
                rows.iter()
                    .map(|row| Row::new(format!("export-{}", row.id), true)),
            )
            .collect();
        self.admin.list.take(Some(view), list, window, cx);
        let view = view.clone();
        let page = page.clone();
        self.render_admin_list(
            move |this, index, window, cx| {
                if index == 0 {
                    this.render_exports_header(&view, &page, window, cx)
                } else {
                    this.render_export_row(&view, &rows[index - 1], &theme(cx), cx)
                }
            },
            window,
            cx,
        )
    }

    fn render_exports_header(
        &mut self,
        view: &AdminView,
        page: &ExportsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = view.server_id;
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::Exports),
                txt::EXPORTS_HELP,
                &theme,
            ))
            .child(sentence(txt::EXPORT_LIMITS, &theme).mb(u(6.0)));
        // An export can be asked for in another place, and this page opens with that
        // request on its way.
        if view.sending(AskKind::RequestExport) {
            column = column.child(
                sentence(txt::EXPORT_REQUEST_SENDING, &theme)
                    .py(u(4.0))
                    .debug_selector(|| "export-request-sending".to_owned()),
            );
        }
        column = column.children(refusal(
            view.refused(AskKind::RequestExport),
            "export-request-refused",
        ));
        let part = AdminPart::Exports;
        match &page.exports {
            Listed::Loading => {
                column = column.child(self.render_reading(server_id, part, None, &theme, cx));
            }
            Listed::Failed(failure) => {
                column =
                    column.child(self.render_reading(server_id, part, Some(*failure), &theme, cx));
            }
            Listed::Loaded(rows) if rows.is_empty() => {
                column = column.child(
                    sentence(txt::NO_EXPORTS, &theme)
                        .py(u(6.0))
                        .debug_selector(|| "no-exports".to_owned()),
                );
            }
            Listed::Loaded(_) => {}
        }
        column.into_any_element()
    }

    /// Asks the server for the shown log with these filters, and puts the open list of
    /// choices away.
    pub fn set_audit_filter(&mut self, filter: AuditFilter, cx: &mut Context<Self>) {
        self.admin.records.audit_menu = None;
        let Some(server_id) = self
            .state
            .read(cx)
            .admin_open()
            .map(|open| open.server_id())
        else {
            return;
        };
        self.state.update(cx, |state, cx| {
            state.set_audit_filter(server_id, filter, cx);
        });
        cx.notify();
    }

    /// Shows the choices of one filter of the audit log under it, or with `None` of no
    /// filter.
    pub fn show_audit_menu(&mut self, menu: Option<AuditMenu>, cx: &mut Context<Self>) {
        self.admin.records.audit_menu = menu;
        cx.notify();
    }

    /// Shows the choices of one filter under it, or puts them away.
    fn toggle_audit_menu(&mut self, menu: AuditMenu, cx: &mut Context<Self>) {
        let same = match (self.admin.records.audit_menu, menu) {
            (Some(AuditMenu::Action(_)), AuditMenu::Action(_))
            | (Some(AuditMenu::About(_)), AuditMenu::About(_)) => true,
            (open, menu) => open == Some(menu),
        };
        self.admin.records.audit_menu = (!same).then_some(menu);
        cx.notify();
    }

    /// One choice of an open filter.
    fn audit_choice(
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        selector: String,
        chosen: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Div {
        named(
            selector,
            Button::new(id)
                .kind(if chosen {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Default
                })
                .label(label)
                .h(u(28.0))
                .px(u(10.0))
                .on_click(on_click),
        )
    }

    /// The filter by action: a box with the chosen action, and under it, while it is
    /// open, the groups and the actions of one group.
    fn render_audit_actions(
        &mut self,
        page: &AuditPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filter = page.filter;
        let chosen = filter
            .action
            .map_or_else(|| txt::EVERY_ACTION.to_owned(), txt::audit_action);
        let first = filter
            .action
            .and_then(group_of_action)
            .unwrap_or(AuditGroup::ALL[0]);
        let mut parts = vec![
            labelled(
                txt::AUDIT_ACTION,
                div()
                    .flex_none()
                    .w(u(FILTER_WIDTH))
                    .child(
                        SelectBox::new("audit-action", txt::AUDIT_ACTION, chosen)
                            .selector("audit-action")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_audit_menu(AuditMenu::Action(first), cx);
                            })),
                    )
                    .into_any_element(),
                theme,
            )
            .into_any_element(),
        ];
        let Some(AuditMenu::Action(group)) = self.admin.records.audit_menu else {
            return parts;
        };
        let groups = Segmented::new(
            "audit-action-group",
            txt::AUDIT_ACTION_GROUP,
            AuditGroup::ALL
                .iter()
                .map(|group| SharedString::from(txt::audit_group(*group))),
        )
        .selected(AuditGroup::ALL.iter().position(|held| *held == group))
        .selector("audit-action-group")
        .on_select({
            let select = cx.listener(|this, index: &usize, _, cx| {
                if let Some(group) = AuditGroup::ALL.get(*index).copied() {
                    this.admin.records.audit_menu = Some(AuditMenu::Action(group));
                    cx.notify();
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        parts.push(
            div()
                .flex()
                .flex_none()
                .debug_selector(|| "audit-action-group".to_owned())
                .child(groups)
                .into_any_element(),
        );
        let mut choices = div()
            .flex()
            .flex_wrap()
            .flex_none()
            .gap(u(6.0))
            .debug_selector(|| "audit-actions".to_owned())
            .child(Self::audit_choice(
                "audit-action-every",
                txt::EVERY_ACTION,
                "audit-action-every".to_owned(),
                filter.action.is_none(),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(
                        AuditFilter {
                            action: None,
                            ..filter
                        },
                        cx,
                    );
                }),
            ));
        for (index, action) in group.actions().enumerate() {
            choices = choices.child(Self::audit_choice(
                ("audit-action-choice", index),
                txt::audit_action(action),
                format!("audit-action-{action}"),
                filter.action == Some(action),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(
                        AuditFilter {
                            action: Some(action),
                            ..filter
                        },
                        cx,
                    );
                }),
            ));
        }
        parts.push(choices.into_any_element());
        parts
    }

    /// The filter by the person who acted: a box with the choice of now, and under it,
    /// while it is open, the accounts that the page offers.
    fn render_audit_actor(
        &mut self,
        page: &AuditPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filter = page.filter;
        let held = filter.actor;
        let id = "audit-actor";
        let chosen = page
            .people
            .iter()
            .find(|choice| Some(choice.id) == held)
            .map_or_else(|| txt::EVERYONE.to_owned(), txt::audit_actor_choice);
        let mut parts = vec![
            labelled(
                txt::AUDIT_ACTOR,
                div()
                    .flex_none()
                    .w(u(FILTER_WIDTH))
                    .child(
                        SelectBox::new(id, txt::AUDIT_ACTOR, chosen)
                            .selector(id)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_audit_menu(AuditMenu::Actor, cx);
                            })),
                    )
                    .into_any_element(),
                theme,
            )
            .into_any_element(),
        ];
        if self.admin.records.audit_menu != Some(AuditMenu::Actor) {
            return parts;
        }
        let mut choices = div()
            .flex()
            .flex_wrap()
            .flex_none()
            .gap(u(6.0))
            .debug_selector(move || format!("{id}-choices"))
            .child(Self::audit_choice(
                (id, 0u64),
                txt::EVERYONE,
                format!("{id}-none"),
                held.is_none(),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(
                        AuditFilter {
                            actor: None,
                            ..filter
                        },
                        cx,
                    );
                }),
            ));
        for choice in &page.people {
            let account_id = choice.id;
            choices = choices.child(Self::audit_choice(
                (id, account_id.as_uuid().as_u128() as u64),
                txt::audit_actor_choice(choice),
                format!("{id}-{account_id}"),
                held == Some(account_id),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(
                        AuditFilter {
                            actor: Some(account_id),
                            ..filter
                        },
                        cx,
                    );
                }),
            ));
        }
        parts.push(choices.into_any_element());
        parts
    }

    /// The filter by the thing concerned: a box with the choice of now, and under it,
    /// while it is open, the kinds of things and the things of one kind (ADR 0305).
    fn render_audit_about(
        &mut self,
        page: &AuditPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filter = page.filter;
        let held = filter.about;
        let id = "audit-about";
        let chosen = page
            .targets
            .iter()
            .flat_map(|(_, choices)| choices.iter())
            .find(|choice| Some(choice.target) == held)
            .map_or_else(|| txt::ANYONE.to_owned(), txt::target_choice);
        let first = page
            .targets
            .iter()
            .find(|(_, choices)| choices.iter().any(|choice| Some(choice.target) == held))
            .or(page.targets.first())
            .map_or(TargetKind::People, |(kind, _)| *kind);
        let mut parts = vec![
            labelled(
                txt::AUDIT_ABOUT,
                div()
                    .flex_none()
                    .w(u(FILTER_WIDTH))
                    .child(
                        SelectBox::new(id, txt::AUDIT_ABOUT, chosen)
                            .selector(id)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_audit_menu(AuditMenu::About(first), cx);
                            })),
                    )
                    .into_any_element(),
                theme,
            )
            .into_any_element(),
        ];
        let Some(AuditMenu::About(kind)) = self.admin.records.audit_menu else {
            return parts;
        };
        let kinds: Vec<TargetKind> = page.targets.iter().map(|(kind, _)| *kind).collect();
        if kinds.len() > 1 {
            let offered = kinds.clone();
            let segmented = Segmented::new(
                "audit-about-kind",
                txt::AUDIT_ABOUT_KIND,
                kinds
                    .iter()
                    .map(|kind| SharedString::from(txt::target_kind(*kind))),
            )
            .selected(kinds.iter().position(|held| *held == kind))
            .selector("audit-about-kind")
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(kind) = offered.get(*index).copied() {
                        this.admin.records.audit_menu = Some(AuditMenu::About(kind));
                        cx.notify();
                    }
                });
                move |index, window, cx| select(&index, window, cx)
            });
            parts.push(
                div()
                    .flex()
                    .flex_none()
                    .debug_selector(|| "audit-about-kind".to_owned())
                    .child(segmented)
                    .into_any_element(),
            );
        }
        let mut choices = div()
            .flex()
            .flex_wrap()
            .flex_none()
            .gap(u(6.0))
            .debug_selector(move || format!("{id}-choices"))
            .child(Self::audit_choice(
                (id, 0u64),
                txt::ANYONE,
                format!("{id}-none"),
                held.is_none(),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(
                        AuditFilter {
                            about: None,
                            ..filter
                        },
                        cx,
                    );
                }),
            ));
        let offered = page
            .targets
            .iter()
            .find(|(held, _)| *held == kind)
            .map(|(_, choices)| choices.as_slice())
            .unwrap_or_default();
        for choice in offered {
            let target = choice.target;
            let identifier = target.id();
            choices = choices.child(Self::audit_choice(
                (id, identifier.as_u128() as u64),
                txt::target_choice(choice),
                format!("{id}-{identifier}"),
                held == Some(target),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(
                        AuditFilter {
                            about: Some(target),
                            ..filter
                        },
                        cx,
                    );
                }),
            ));
        }
        parts.push(choices.into_any_element());
        parts
    }

    /// Shows the form of a range of dates, with the range of the filter in its fields.
    fn open_audit_dates(
        &mut self,
        period: AuditPeriod,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.admin.records.audit_menu = Some(AuditMenu::Dates);
        self.admin.records.dates_invalid = None;
        if let AuditPeriod::Dates { from, to } = period {
            let day = |date: time::Date| txt::audit_dates(date, date);
            self.type_audit_dates(&day(from), &day(to), window, cx);
        }
        let from = self.admin.records.audit_from.clone();
        self.focus_field(&from, window, cx);
        cx.notify();
    }

    /// Puts dates into the two fields of the range of the audit log, as typing does.
    pub fn type_audit_dates(
        &mut self,
        from: &str,
        to: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (field, text) in [
            (self.admin.records.audit_from.clone(), from),
            (self.admin.records.audit_to.clone(), to),
        ] {
            let text = text.to_owned();
            field.update(cx, |field, cx| field.set_value(text, window, cx));
        }
    }

    /// Reads the shown log for the typed range of dates, when both are dates and the first
    /// is not after the last.
    pub fn apply_audit_dates(&mut self, cx: &mut Context<Self>) {
        let typed = |field: &Entity<InputState>| txt::typed_date(&field.read(cx).value());
        let ui = &self.admin.records;
        let (from, to) = (typed(&ui.audit_from), typed(&ui.audit_to));
        let Some(view) = self.state.read(cx).admin_view() else {
            return;
        };
        let Scope::Server {
            page: ServerPage::AuditLog(page),
            ..
        } = &view.scope
        else {
            return;
        };
        match (from, to) {
            (Some(from), Some(to)) if from <= to => {
                let filter = AuditFilter {
                    period: AuditPeriod::Dates { from, to },
                    ..page.filter
                };
                self.set_audit_filter(filter, cx);
            }
            _ => {
                self.admin.records.dates_invalid = Some(txt::DATES_INVALID);
                cx.notify();
            }
        }
    }

    /// The form of a range of dates under the filter by time.
    fn render_audit_dates(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.admin.records;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .debug_selector(|| "audit-dates".to_owned())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        div().w(u(150.0)).child(
                            Field::new(&ui.audit_from, txt::DATES_FROM).selector("audit-from"),
                        ),
                    )
                    .child(
                        div()
                            .w(u(150.0))
                            .child(Field::new(&ui.audit_to, txt::DATES_TO).selector("audit-to")),
                    )
                    .child(
                        Button::new("audit-dates-cancel")
                            .label(text::CANCEL)
                            .selector("audit-dates-cancel")
                            .h(u(30.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_audit_menu(None, cx);
                            })),
                    )
                    .child(
                        Button::new("audit-dates-apply")
                            .kind(ButtonKind::Primary)
                            .label(txt::DATES_APPLY)
                            .selector("audit-dates-apply")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| this.apply_audit_dates(cx))),
                    ),
            )
            .child(
                crate::ui::notice_slot("audit-dates-invalid-slot").children(
                    ui.dates_invalid
                        .map(|invalid| Notice::error(invalid).selector("audit-dates-invalid")),
                ),
            )
            .child(sentence(txt::DATES_HELP, theme).mt(u(5.0)))
            .into_any_element()
    }

    pub(crate) fn render_audit_page(
        &mut self,
        view: &AdminView,
        page: &AuditPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        // The page reads the log that it shows when it is drawn first. The read is asked
        // for once for each log: a read that cannot start does not ask again with every
        // frame.
        if page.state == AuditRead::NotRead
            && let Some(scope) = page.scope
            && self.admin.records.audit_asked != Some((server_id, scope))
        {
            self.admin.records.audit_asked = Some((server_id, scope));
            cx.defer_in(window, move |this, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.show_audit_log(server_id, scope, cx);
                });
            });
        }
        let list =
            std::iter::once(Row::new("audit-header", true).part())
                .chain(page.rows.iter().map(|row| {
                    Row::new(format!("audit-{}", row.id), row.deleted_workspace.is_some())
                }))
                .chain(std::iter::once(Row::new("audit-footer", true).part()))
                .collect();
        self.admin.list.take(Some(view), list, window, cx);
        // "Continue" and "Read again" start the view at the first entry of their read: the
        // request ends with the read.
        if page.state != AuditRead::Reading {
            self.admin.list.end_restart();
        }
        let page = page.clone();
        self.render_admin_list(
            move |this, index, window, cx| {
                let theme = theme(cx);
                if index == 0 {
                    return this.render_audit_header(server_id, &page, window, cx);
                }
                if let Some(row) = page.rows.get(index - 1) {
                    return this.render_audit_entry(server_id, row, &theme, cx);
                }
                this.render_audit_footer(server_id, &page, cx)
            },
            window,
            cx,
        )
    }

    fn render_audit_header(
        &mut self,
        server_id: ServerId,
        page: &AuditPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let mut column = div().flex().flex_col().gap(u(8.0)).child(page_title(
            text::admin::page_name(AdminPage::AuditLog),
            txt::AUDIT_HELP,
            &theme,
        ));
        // The log that is shown.
        if page.scopes.len() > 1 {
            let scopes: Vec<AuditScope> = page.scopes.iter().map(|choice| choice.scope).collect();
            let scope = Segmented::new(
                "audit-scope",
                txt::AUDIT_SCOPE,
                page.scopes
                    .iter()
                    .map(|choice| SharedString::from(txt::audit_scope(choice))),
            )
            .selected(
                page.scope
                    .and_then(|shown| scopes.iter().position(|held| *held == shown)),
            )
            .selector("audit-scope")
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(scope) = scopes.get(*index).copied() {
                        this.admin.records.audit_asked = Some((server_id, scope));
                        this.admin.records.audit_menu = None;
                        this.state.update(cx, |state, cx| {
                            state.show_audit_log(server_id, scope, cx);
                        });
                        cx.notify();
                    }
                });
                move |index, window, cx| select(&index, window, cx)
            });
            column = column.child(labelled(
                txt::AUDIT_SCOPE,
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(|| "audit-scope".to_owned())
                    .child(scope)
                    .into_any_element(),
                &theme,
            ));
        }
        // The filters. The server applies them: each change reads the log again.
        let filter = page.filter;
        column = column.children(self.render_audit_actions(page, &theme, cx));
        column = column.children(self.render_audit_actor(page, &theme, cx));
        column = column.children(self.render_audit_about(page, &theme, cx));
        // The periods that end at the read, and a range of dates that the person types.
        let dates_choice = AuditPeriod::ALL.len();
        let period = Segmented::new(
            "audit-period",
            txt::AUDIT_PERIOD,
            AuditPeriod::ALL
                .iter()
                .map(|period| SharedString::from(txt::audit_period(*period)))
                .chain([SharedString::from(txt::DATES)]),
        )
        .selected(match filter.period {
            AuditPeriod::Dates { .. } => Some(dates_choice),
            held => AuditPeriod::ALL.iter().position(|period| *period == held),
        })
        .selector("audit-period")
        .on_select({
            let select = cx.listener(move |this, index: &usize, window, cx| {
                if let Some(period) = AuditPeriod::ALL.get(*index).copied() {
                    this.set_audit_filter(AuditFilter { period, ..filter }, cx);
                } else if *index == dates_choice {
                    this.open_audit_dates(filter.period, window, cx);
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        column = column.child(labelled(
            txt::AUDIT_PERIOD,
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .debug_selector(|| "audit-period".to_owned())
                .child(period)
                .into_any_element(),
            &theme,
        ));
        if self.admin.records.audit_menu == Some(AuditMenu::Dates) {
            column = column.child(self.render_audit_dates(&theme, cx));
        } else if let AuditPeriod::Dates { from, to } = filter.period {
            column = column.child(
                sentence(txt::audit_dates(from, to), &theme)
                    .debug_selector(|| "audit-dates-shown".to_owned()),
            );
        }
        if filter.is_set() {
            column = column.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(10.0))
                    .child(
                        sentence(txt::AUDIT_FILTERED, &theme)
                            .flex_1()
                            .debug_selector(|| "audit-filtered".to_owned()),
                    )
                    .child(
                        Button::new("audit-clear")
                            .label(txt::CLEAR_FILTERS)
                            .selector("audit-clear")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_audit_filter(AuditFilter::default(), cx);
                            })),
                    ),
            );
        }
        column.mb(u(8.0)).into_any_element()
    }

    fn render_audit_entry(
        &mut self,
        server_id: ServerId,
        row: &AuditLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The entry of the deletion of a workspace opens the log of that workspace.
        let action = row.deleted_workspace.clone().map(|(workspace_id, name)| {
            row_action(
                ("audit-deleted-log", row.id.as_uuid().as_u128() as u64),
                txt::READ_ITS_LOG,
                format!("audit-deleted-log-{workspace_id}"),
                None,
                cx.listener(move |this, _, _, cx| {
                    let scope = AuditScope::DeletedWorkspace(workspace_id);
                    this.admin.records.audit_asked = Some((server_id, scope));
                    this.admin.records.audit_menu = None;
                    let name = name.clone();
                    this.state.update(cx, |state, cx| {
                        state.show_deleted_workspace_log(server_id, workspace_id, name, cx);
                    });
                }),
            )
            .into_any_element()
        });
        audit_row(row, action, theme)
    }

    fn render_audit_footer(
        &mut self,
        server_id: ServerId,
        page: &AuditPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let filter = page.filter;
        let reading = matches!(page.state, AuditRead::NotRead | AuditRead::Reading);
        let mut column = div().flex().flex_col().gap(u(8.0)).mt(u(8.0));
        if page.loaded == 0 && page.state == AuditRead::Loaded {
            column = column.child(if filter.is_set() {
                sentence(txt::NO_ENTRIES_FILTERED, &theme)
                    .py(u(6.0))
                    .debug_selector(|| "audit-none-pass".to_owned())
            } else {
                sentence(txt::NO_ENTRIES, &theme)
                    .py(u(6.0))
                    .debug_selector(|| "audit-empty".to_owned())
            });
        }
        let mut result = crate::ui::notice_slot("audit-result-slot");
        // The end of the list: a read that runs or failed, the older entries, the bound.
        match &page.state {
            AuditRead::NotRead | AuditRead::Reading => {
                result = result.child(
                    sentence(text::admin::READING, &theme)
                        .py(u(6.0))
                        .debug_selector(|| "audit-reading".to_owned()),
                );
            }
            AuditRead::Failed {
                sentence: reason,
                read,
            } => {
                let read = *read;
                let scope = page.scope;
                result = result.child(
                    div().flex_none().child(
                        Notice::error(reason.clone())
                            .selector("audit-failed")
                            .action(
                                Button::new("audit-retry")
                                    .label(text::TRY_AGAIN)
                                    .selector("audit-retry")
                                    .h(u(28.0))
                                    .px(u(10.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| match read {
                                            AuditReadKind::Older => {
                                                state.read_older_audit(server_id, cx);
                                            }
                                            AuditReadKind::Next => {
                                                state.continue_audit(server_id, cx);
                                            }
                                            AuditReadKind::First => {
                                                if let Some(scope) = scope {
                                                    state.show_audit_log(server_id, scope, cx);
                                                }
                                            }
                                        });
                                    })),
                            ),
                    ),
                );
            }
            AuditRead::Loaded if page.older => {
                result = result.child(
                    div().flex().flex_none().child(
                        Button::new("audit-older")
                            .label(txt::LOAD_OLDER)
                            .selector("audit-older")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.read_older_audit(server_id, cx);
                                });
                            })),
                    ),
                );
            }
            AuditRead::Loaded => {}
        }
        if page.at_bound {
            result = result.child(
                sentence(txt::AUDIT_BOUND, &theme).debug_selector(|| "audit-bound".to_owned()),
            );
            if page.continues && page.state == AuditRead::Loaded {
                result = result.child(
                    div().flex().flex_none().mt(u(6.0)).child(
                        Button::new("audit-continue")
                            .label(txt::CONTINUE)
                            .selector("audit-continue")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.admin.list.start_at_first_item();
                                this.state.update(cx, |state, cx| {
                                    state.continue_audit(server_id, cx);
                                });
                            })),
                    ),
                );
            }
        }
        column = column.child(result);
        if page.state != AuditRead::NotRead {
            let scope = page.scope;
            column = column.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(10.0))
                    .child(
                        sentence(txt::audit_count(page.loaded, page.passed), &theme)
                            .flex_1()
                            .debug_selector(|| "audit-count".to_owned()),
                    )
                    .child(
                        Button::new("audit-reload")
                            .label(txt::READ_AGAIN)
                            .selector("audit-reload")
                            .disabled(reading)
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(scope) = scope {
                                    this.admin.list.start_at_first_item();
                                    this.state.update(cx, |state, cx| {
                                        state.show_audit_log(server_id, scope, cx);
                                    });
                                }
                            })),
                    ),
            );
        }
        column.into_any_element()
    }
}
