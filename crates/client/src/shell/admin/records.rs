//! The pages of the records of a server (ADR 0761): the access tokens with the form of a
//! new one (ADR 0210), the exports of the person with their files (ADR 0411), and the
//! audit log with its filters (ADR 0303, ADR 0305).

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    div,
};
use gpui_base::input::InputState;
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::access_token::TokenAction;
use junto_client_core::protocol::ids::{AccessTokenId, AccountId, ExportId, ServerId};
use time::OffsetDateTime;

use super::{group_label, line, named, page_title, refusal, row_action, sentence};
use crate::shell::Shell;
use crate::shell::room::option_row;
use crate::state::admin::view::records::{
    ActorChoice, AuditGroup, AuditLine, AuditPage, AuditRead, DEFAULT_TOKEN_EXPIRY_DAYS,
    ExportLine, ExportSave, ExportStatus, ExportsPage, NewToken, TOKEN_EXPIRY_DAYS, TokenLine,
    TokensPage, group_of_action, new_token,
};
use crate::state::admin::view::{AdminView, Listed};
use crate::state::admin::{AdminPage, AskKind, AskState, AuditFilter, AuditPeriod, AuditScope};
use crate::text;
use crate::text::admin::records as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Chip, Field, FieldEvent, Notice, Segmented, SelectBox, StyleExt, Toggle,
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
    About,
}

/// A filter of the audit log by a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersonFilter {
    Actor,
    About,
}

/// The actions that the form of a new access token has on when it opens: the one that
/// changes nothing on the server.
const FIRST_ACTIONS: [TokenAction; 1] = [TokenAction::ReadStructure];

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
    /// The log whose first read the page of the audit log asked for.
    pub(crate) audit_asked: Option<(ServerId, AuditScope)>,
    /// The filter of the audit log whose choices the page shows.
    pub(crate) audit_menu: Option<AuditMenu>,
}

impl Ui {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let token_name = Field::state(txt::TOKEN_NAME, window, cx);
        let subscriptions = vec![Field::observe(
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
            audit_asked: None,
            audit_menu: None,
        };
        (ui, subscriptions)
    }
}

/// A row with what belongs under it. `selector` names the row for a test.
fn framed(selector: String, row: gpui::Div, under: Vec<AnyElement>) -> AnyElement {
    div()
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

fn audit_row(row: &AuditLine, theme: &Theme) -> AnyElement {
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
            .children(row.reason.as_ref().map(|reason| {
                div()
                    .debug_selector(move || format!("audit-reason-{id}"))
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink)
                    .child(txt::audit_reason(reason))
            })),
    );
    framed(format!("audit-row-{}", row.id), content, Vec::new())
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
        ui.audit_asked = None;
        ui.audit_menu = None;
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
        if !self.admin.records.token_sent {
            return;
        }
        match view.asked(AskKind::CreateToken) {
            Some(AskState::Sending) => {}
            Some(AskState::Refused(_)) => self.admin.records.token_sent = false,
            None => {
                self.admin.records.token_sent = false;
                self.admin.records.token_form = false;
                self.admin.records.token_invalid = None;
                self.reset_token_form(window, cx);
            }
        }
    }

    /// Shows the question before the removal of an export in its row, or the actions of
    /// every row again.
    pub fn set_export_removing(&mut self, export_id: Option<ExportId>, cx: &mut Context<Self>) {
        self.admin.records.removing = export_id;
        self.admin.records.remove_sent = false;
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
                        this.admin.records.copied = None;
                        this.state
                            .update(cx, |state, cx| state.dismiss_admin_made(cx));
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
            .children(
                self.admin
                    .records
                    .token_invalid
                    .map(|invalid| Notice::error(invalid).selector("token-form-invalid")),
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
            vec![txt::token_actions(&row.actions), txt::token_detail(row)],
            theme,
        ));
        if row.expired {
            content = content.child(Chip::new(txt::EXPIRED));
        }
        content = content.child(row_action(
            ("token-revoke", token_id.as_uuid().as_u128() as u64),
            txt::REVOKE,
            format!("token-revoke-{token_id}"),
            view.sending(what).then_some(text::admin::SENDING),
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
        if let Some(made) = &page.made {
            column = column.child(self.render_new_token(made, &theme, cx));
        }
        // A new token.
        column = column.child(group_label(txt::NEW_TOKEN, &theme));
        // A person who is no administrator any more gets no form, also when it was open.
        column = if self.admin.records.token_form && page.may_create {
            column.child(self.render_token_form(view, &theme, cx))
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
            column
                .child(div().flex().flex_none().child(button))
                .children(held.map(|reason| {
                    sentence(reason, &theme)
                        .mt(u(5.0))
                        .debug_selector(|| "token-new-held".to_owned())
                }))
        };
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
            Listed::Loaded(rows) if rows.is_empty() => {
                column = column.child(
                    sentence(txt::no_tokens(page.may_create), &theme)
                        .py(u(6.0))
                        .debug_selector(|| "no-tokens".to_owned()),
                );
            }
            Listed::Loaded(rows) => {
                for row in rows {
                    column = column.child(self.render_token_row(view, row, &theme, cx));
                }
                if page.more {
                    column = column.child(
                        sentence(txt::more_tokens(rows.len()), &theme)
                            .mt(u(6.0))
                            .debug_selector(|| "tokens-more".to_owned()),
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
        under.extend(refusal(
            view.refused(AskKind::RemoveExport(export_id)),
            "export-remove-refused",
        ));
        framed(format!("export-row-{export_id}"), content, under)
    }

    pub(crate) fn render_exports_page(
        &mut self,
        view: &AdminView,
        page: &ExportsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_export_question(view);
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
            Listed::Loaded(rows) => {
                for row in rows {
                    column = column.child(self.render_export_row(view, row, &theme, cx));
                }
            }
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
            (Some(AuditMenu::Action(_)), AuditMenu::Action(_)) => true,
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

    /// A filter by a person: a box with the choice of now, and under it, while it is
    /// open, the accounts that the page offers.
    fn render_audit_people(
        &mut self,
        page: &AuditPage,
        which: PersonFilter,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filter = page.filter;
        let (label, none, id, menu, held) = match which {
            PersonFilter::Actor => (
                txt::AUDIT_ACTOR,
                txt::EVERYONE,
                "audit-actor",
                AuditMenu::Actor,
                filter.actor,
            ),
            PersonFilter::About => (
                txt::AUDIT_ABOUT,
                txt::ANYONE,
                "audit-about",
                AuditMenu::About,
                filter.about,
            ),
        };
        let with = move |account_id: Option<AccountId>| match which {
            PersonFilter::Actor => AuditFilter {
                actor: account_id,
                ..filter
            },
            PersonFilter::About => AuditFilter {
                about: account_id,
                ..filter
            },
        };
        let name_of = |choice: &ActorChoice| txt::audit_actor_choice(choice);
        let chosen = page
            .people
            .iter()
            .find(|choice| Some(choice.id) == held)
            .map_or_else(|| none.to_owned(), name_of);
        let mut parts =
            vec![
                labelled(
                    label,
                    div()
                        .flex_none()
                        .w(u(FILTER_WIDTH))
                        .child(SelectBox::new(id, label, chosen).selector(id).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.toggle_audit_menu(menu, cx);
                            }),
                        ))
                        .into_any_element(),
                    theme,
                )
                .into_any_element(),
            ];
        if self.admin.records.audit_menu != Some(menu) {
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
                none,
                format!("{id}-none"),
                held.is_none(),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(with(None), cx);
                }),
            ));
        for choice in &page.people {
            let account_id = choice.id;
            choices = choices.child(Self::audit_choice(
                (id, account_id.as_uuid().as_u128() as u64),
                name_of(choice),
                format!("{id}-{account_id}"),
                held == Some(account_id),
                cx.listener(move |this, _, _, cx| {
                    this.set_audit_filter(with(Some(account_id)), cx);
                }),
            ));
        }
        parts.push(choices.into_any_element());
        parts
    }

    pub(crate) fn render_audit_page(
        &mut self,
        view: &AdminView,
        page: &AuditPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
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
        let reading = matches!(page.state, AuditRead::NotRead | AuditRead::Reading);
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
        column = column.children(self.render_audit_people(page, PersonFilter::Actor, &theme, cx));
        column = column.children(self.render_audit_people(page, PersonFilter::About, &theme, cx));
        let period = Segmented::new(
            "audit-period",
            txt::AUDIT_PERIOD,
            AuditPeriod::ALL
                .iter()
                .map(|period| SharedString::from(txt::audit_period(*period))),
        )
        .selected(
            AuditPeriod::ALL
                .iter()
                .position(|held| *held == filter.period),
        )
        .selector("audit-period")
        .on_select({
            let select = cx.listener(move |this, index: &usize, _, cx| {
                if let Some(period) = AuditPeriod::ALL.get(*index).copied() {
                    this.set_audit_filter(AuditFilter { period, ..filter }, cx);
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
        // The entries.
        let mut entries = div().flex().flex_col().flex_none();
        for row in &page.rows {
            entries = entries.child(audit_row(row, &theme));
        }
        column = column.child(entries);
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
        // The end of the list: a read that runs or failed, the older entries, the bound.
        match &page.state {
            AuditRead::NotRead | AuditRead::Reading => {
                column = column.child(
                    sentence(text::admin::READING, &theme)
                        .py(u(6.0))
                        .debug_selector(|| "audit-reading".to_owned()),
                );
            }
            AuditRead::Failed {
                sentence: reason,
                older,
            } => {
                let older = *older;
                let scope = page.scope;
                column = column.child(
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
                                        this.state.update(cx, |state, cx| {
                                            if older {
                                                state.read_older_audit(server_id, cx);
                                            } else if let Some(scope) = scope {
                                                state.show_audit_log(server_id, scope, cx);
                                            }
                                        });
                                    })),
                            ),
                    ),
                );
            }
            AuditRead::Loaded if page.older => {
                column = column.child(
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
            column = column.child(
                sentence(txt::AUDIT_BOUND, &theme).debug_selector(|| "audit-bound".to_owned()),
            );
        }
        if page.state != AuditRead::NotRead {
            let scope = page.scope;
            column = column.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(10.0))
                    .child(
                        sentence(txt::audit_count(page.loaded), &theme)
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
