//! The page of the glossary of a workspace (ADR 0616): the terms in their order, a new term
//! at the end, a changed term, a removed term and a term that moves by one place.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::list::Row;
use super::{
    Form, FormField, Mode, Question, group_label, line, named, page_title, refusal, row_action,
    sentence,
};
use crate::shell::Shell;
use crate::state::admin::glossary::GlossaryChange;
use crate::state::admin::view::glossary::GlossaryPage;
use crate::state::admin::view::{AdminView, Listed};
use crate::state::admin::{AdminPage, AskKind, AskState};
use crate::text;
use crate::text::admin::glossary as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, StyleExt};

/// A request of the page is on its way: the page takes no second one, because each change
/// is made from the list that the one before changed.
fn sending(view: &AdminView) -> bool {
    matches!(
        view.ask.as_ref(),
        Some(ask) if ask.state == AskState::Sending
            && matches!(
                ask.what,
                AskKind::AddTerm
                    | AskKind::ChangeTerm(_)
                    | AskKind::RemoveTerm(_)
                    | AskKind::MoveTerm(_)
            )
    )
}

impl Shell {
    /// Opens the form or the question of the term at `position`, which the page shows as
    /// `term`.
    pub fn open_term_mode(
        &mut self,
        mode: Mode,
        term: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = if matches!(mode, Mode::ChangeTerm(_)) {
            term
        } else {
            ""
        };
        self.set_admin_mode(Some(mode), name, window, cx);
        self.admin.opened_term = Some(term.to_owned());
    }

    /// Sends a change of the glossary. A change that the page does not send is told in the
    /// form; one that changes nothing closes it.
    fn send_glossary_change(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        change: GlossaryChange,
        cx: &mut Context<Self>,
    ) {
        let what = change.ask();
        let sent = self.state.update(cx, |state, cx| {
            state.change_glossary(server_id, workspace_id, change, cx)
        });
        match sent {
            Ok(true) => self.admin_sent(what),
            Ok(false) => {
                self.admin.mode = None;
                self.admin.opened_term = None;
            }
            Err(reason) => self.admin.invalid = Some(reason),
        }
        cx.notify();
    }

    /// Sends the form of the open row of the glossary page.
    pub(crate) fn submit_glossary_form(
        &mut self,
        view: &AdminView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let crate::state::admin::view::Scope::Server {
            page: crate::state::admin::view::ServerPage::Glossary(page),
            ..
        } = &view.scope
        else {
            return;
        };
        // A form can stay open while the permission is lost or the list is read again.
        if let Some(held) = page.held() {
            self.admin.invalid = Some(held);
            cx.notify();
            return;
        }
        let typed = self.admin_name(cx);
        let change = match self.admin.mode {
            Some(Mode::AddTerm) => GlossaryChange::Add(typed),
            Some(Mode::ChangeTerm(position)) => GlossaryChange::Change {
                position,
                from: self.admin.opened_term.clone().unwrap_or_default(),
                to: typed,
            },
            _ => return,
        };
        self.send_glossary_change(view.server_id, page.workspace_id, change, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_term_row(
        &mut self,
        view: &AdminView,
        page: &GlossaryPage,
        index: usize,
        term: &str,
        count: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let position = u32::try_from(index).unwrap_or(u32::MAX);
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let held = page
            .held()
            .or(sending(view).then_some(text::admin::SENDING));
        let content: AnyElement = match self.admin.mode {
            Some(Mode::ChangeTerm(open)) if open == position => self
                .render_form(
                    Form {
                        field: FormField::Name,
                        label: txt::TERM,
                        field_selector: "glossary-term",
                        send: text::SAVE,
                        sending: view.sending(AskKind::ChangeTerm(position)),
                        help: None,
                        held: page.held(),
                        extra: Vec::new(),
                    },
                    theme,
                    cx,
                )
                .into_any_element(),
            Some(Mode::RemoveTerm(open)) if open == position => {
                let term = term.to_owned();
                self.render_row_question(
                    Question {
                        sentence: txt::remove_question(&term).into(),
                        confirm: text::REMOVE,
                        confirm_selector: "glossary-remove-confirm",
                        cancel_selector: "glossary-remove-cancel",
                        sending: view.sending(AskKind::RemoveTerm(position)),
                    },
                    theme,
                    cx,
                    move |this, _, cx| {
                        let change = GlossaryChange::Remove {
                            position,
                            term: term.clone(),
                        };
                        this.send_glossary_change(server_id, workspace_id, change, cx);
                    },
                )
                .into_any_element()
            }
            _ => {
                let moves = |up: bool| -> Option<&'static str> {
                    held.or(match (up, index) {
                        (true, 0) => Some(text::admin::FIRST_ALREADY),
                        (false, index) if index + 1 >= count => Some(text::admin::LAST_ALREADY),
                        _ => None,
                    })
                };
                let id = index as u64;
                let mover = |up: bool| {
                    let term = term.to_owned();
                    move |this: &mut Self,
                          _: &gpui::ClickEvent,
                          _: &mut Window,
                          cx: &mut Context<Self>| {
                        let change = GlossaryChange::Move {
                            position,
                            term: term.clone(),
                            up,
                        };
                        this.send_glossary_change(server_id, workspace_id, change, cx);
                    }
                };
                let up = cx.listener(mover(true));
                let down = cx.listener(mover(false));
                let (change_term, remove_term) = (term.to_owned(), term.to_owned());
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        div()
                            .flex_none()
                            .w(u(30.0))
                            .typeface(theme.types.small, theme)
                            .ink(theme.colors.ink_3)
                            .child(txt::position(index)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .typeface(theme.types.control_strong, theme)
                            .ink(theme.colors.ink)
                            .truncate()
                            .child(SharedString::from(term.to_owned())),
                    )
                    .child(row_action(
                        ("glossary-up", id),
                        text::admin::UP,
                        format!("glossary-up-{index}"),
                        moves(true),
                        up,
                    ))
                    .child(row_action(
                        ("glossary-down", id),
                        text::admin::DOWN,
                        format!("glossary-down-{index}"),
                        moves(false),
                        down,
                    ))
                    .child(row_action(
                        ("glossary-change", id),
                        text::admin::CHANGE,
                        format!("glossary-change-{index}"),
                        held,
                        cx.listener(move |this, _, window, cx| {
                            this.open_term_mode(
                                Mode::ChangeTerm(position),
                                &change_term,
                                window,
                                cx,
                            );
                        }),
                    ))
                    .child(row_action(
                        ("glossary-remove", id),
                        text::REMOVE,
                        format!("glossary-remove-{index}"),
                        held,
                        cx.listener(move |this, _, window, cx| {
                            this.open_term_mode(
                                Mode::RemoveTerm(position),
                                &remove_term,
                                window,
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()
            }
        };
        let refused = view
            .refused(AskKind::ChangeTerm(position))
            .or(view.refused(AskKind::RemoveTerm(position)))
            .or(view.refused(AskKind::MoveTerm(position)));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(self.mode_row(
                &[Mode::ChangeTerm(position), Mode::RemoveTerm(position)],
                named(format!("glossary-row-{index}"), line(theme).child(content)).flex_1(),
            ))
            .children(refusal(refused, "glossary-refused"))
            .into_any_element()
    }

    pub(crate) fn render_glossary_page(
        &mut self,
        view: &AdminView,
        page: &GlossaryPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A form or a question of a term that the list holds no longer at its position
        // closes: the list changed, and the form would change another term.
        if let Some(Mode::ChangeTerm(position) | Mode::RemoveTerm(position)) = self.admin.mode {
            let shown = page.terms.loaded().and_then(|terms| {
                usize::try_from(position)
                    .ok()
                    .and_then(|index| terms.get(index))
            });
            if shown.is_none_or(|term| self.admin.opened_term.as_deref() != Some(term.as_str()))
                && self.admin.sent.is_none()
            {
                // The change of another person closes the form at a time that the person
                // does not choose: the row keeps the height of the form until the person
                // acts (ADR 0730).
                if let Some(mode) = self.admin.mode.take() {
                    self.admin.space.keep(mode);
                }
                self.admin.opened_term = None;
                self.admin.invalid = None;
            }
        }
        let held = page
            .held()
            .or(sending(view).then_some(text::admin::SENDING));
        let terms = page.terms.loaded().cloned().unwrap_or_default();
        // A row holds a control that takes the focus while the page takes changes, or while
        // its form or question is open.
        let open = |index: usize| {
            matches!(
                self.admin.mode,
                Some(Mode::ChangeTerm(at) | Mode::RemoveTerm(at)) if at as usize == index
            )
        };
        let list =
            std::iter::once(
                Row::new(
                    "glossary-header",
                    held.is_none()
                        || matches!(page.terms, Listed::Failed(_))
                        || self.admin.mode == Some(Mode::AddTerm),
                )
                .part(),
            )
            .chain(terms.iter().enumerate().map(|(index, term)| {
                Row::new(format!("term-{term}"), held.is_none() || open(index))
            }))
            .chain(std::iter::once(Row::new("glossary-footer", false).part()))
            .collect();
        self.admin.list.take(Some(view), list, window, cx);
        let view = view.clone();
        let page = page.clone();
        self.render_admin_list(
            move |this, index, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    return this.render_glossary_header(&view, &page, &theme, cx);
                }
                if let Some(term) = terms.get(index - 1) {
                    return this.render_term_row(
                        &view,
                        &page,
                        index - 1,
                        term,
                        terms.len(),
                        &theme,
                        cx,
                    );
                }
                // Below every row, so that a permission given or taken moves no control
                // (ADR 0730).
                div()
                    .children((!page.may_manage).then(|| {
                        sentence(txt::MANAGE_NEEDS, &theme)
                            .mt(u(6.0))
                            .debug_selector(|| "glossary-manage-needs".to_owned())
                    }))
                    .into_any_element()
            },
            window,
            cx,
        )
    }

    /// The title of the page, the form of a new term and the state of the list.
    fn render_glossary_header(
        &mut self,
        view: &AdminView,
        page: &GlossaryPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let held = page
            .held()
            .or(sending(view).then_some(text::admin::SENDING));
        let mut column = div().flex().flex_col().child(page_title(
            text::admin::page_name(AdminPage::Glossary),
            txt::HELP,
            theme,
        ));
        // A new term goes to the end of the list: the rows above it do not move.
        column = column.child(group_label(txt::NEW_TERM, theme));
        let add = match self.admin.mode {
            Some(Mode::AddTerm) => self.render_form(
                Form {
                    field: FormField::Name,
                    label: txt::TERM,
                    field_selector: "glossary-term",
                    send: txt::ADD,
                    sending: view.sending(AskKind::AddTerm),
                    help: None,
                    held: page.held(),
                    extra: Vec::new(),
                },
                theme,
                cx,
            ),
            _ => {
                let button = Button::new("glossary-add")
                    .icon(crate::assets::Icon::Plus)
                    .label(txt::NEW_TERM)
                    .selector("glossary-add")
                    .disabled(held.is_some())
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_term_mode(Mode::AddTerm, "", window, cx);
                    }));
                let button = match held {
                    Some(reason) => button.tooltip(reason),
                    None => button,
                };
                div().flex().flex_none().child(button)
            }
        };
        column = column.child(self.mode_row(&[Mode::AddTerm], add));
        column = column.children(refusal(
            view.refused(AskKind::AddTerm),
            "glossary-add-refused",
        ));
        column = column.child(group_label(txt::TERMS, theme));
        match &page.terms {
            Listed::Loading => {
                let reading = self.render_reading(
                    view.server_id,
                    AdminPart::Glossary(page.workspace_id),
                    None,
                    theme,
                    cx,
                );
                column = column.child(reading);
            }
            Listed::Failed(failure) => {
                let failed = self.render_reading(
                    view.server_id,
                    AdminPart::Glossary(page.workspace_id),
                    Some(*failure),
                    theme,
                    cx,
                );
                column = column.child(failed);
            }
            Listed::Loaded(terms) if terms.is_empty() => {
                column = column.child(
                    sentence(txt::EMPTY, theme)
                        .py(u(6.0))
                        .debug_selector(|| "glossary-empty".to_owned()),
                );
            }
            Listed::Loaded(terms) => {
                column = column.child(
                    sentence(txt::count(terms.len()), theme)
                        .mb(u(4.0))
                        .debug_selector(|| "glossary-count".to_owned()),
                );
                if page.cut {
                    column = column.child(sentence(txt::CUT, theme).mb(u(4.0)));
                }
            }
        }
        column.into_any_element()
    }
}
