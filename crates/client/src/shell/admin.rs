//! The administration of a server and the settings of a channel (ADR 0761): one layer with
//! a list of pages at the left and the open page at the right. A page shows a form or a
//! question in the row that it belongs to, so no second layer opens above this one.

mod archive;
mod channels;
mod notifications;
pub mod overrides;
pub mod people;
mod recordings;
pub mod records;
pub mod roles;
mod server;
mod workspaces;

use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, Entity, FocusHandle, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder,
};
use gpui_base::FocusTrapElement as _;
use gpui_base::input::InputState;
use junto_client_core::model::{AdminPart, LoadFailure};
use junto_client_core::protocol::ids::{CategoryId, ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::ChannelKind;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::admin::view::{AdminView, ChannelPageView, ChannelSettings, Scope, ServerPage};
use crate::state::admin::{AdminOpen, AdminPage, AskKind, ChannelPage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, ChannelRow, Field, FieldEvent, Notice, ScrollArea, SideHeader, StyleExt,
    float_shadow,
};

/// The width and the height of the layer in design pixels.
const SURFACE_WIDTH: f32 = 880.0;
const SURFACE_HEIGHT: f32 = 620.0;
const NAV_WIDTH: f32 = 208.0;

/// The row of the structure pages that shows a form or a question in place of its
/// actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    CreateWorkspace,
    RenameWorkspace(WorkspaceId),
    DeleteWorkspace(WorkspaceId),
    /// The invite code that enters a further workspace.
    EnterCode,
    CreateCategory,
    RenameCategory(CategoryId),
    DeleteCategory(CategoryId),
    CreateChannel,
    DeleteChannel(ChannelId),
    ArchiveChannel(ChannelId),
    /// The category that a channel moves into is chosen.
    MoveChannel(ChannelId),
    /// The new name of the channel whose settings are open.
    RenameChannel,
    /// A number of days is typed: the archive period or a retention.
    Days(AskKind),
    /// The question before the removal of the logo of the server.
    RemoveLogo,
}

/// What the layer of the administration holds of the window itself: the text of its
/// fields, and which row shows a form or a question.
pub(crate) struct AdminUi {
    /// The name of what is created or renamed.
    pub(crate) name: Entity<InputState>,
    /// A number or a code: days, a user limit, an invite code.
    pub(crate) value: Entity<InputState>,
    pub(crate) mode: Option<Mode>,
    /// The kind and the category of the channel that is created.
    pub(crate) new_kind: ChannelKind,
    pub(crate) new_category: Option<CategoryId>,
    /// Why the text of a field was not sent.
    pub(crate) invalid: Option<&'static str>,
    /// The request that the open form or question was sent as.
    pub(crate) sent: Option<AskKind>,
    pub(crate) nav_scroll: ScrollHandle,
    pub(crate) page_scroll: ScrollHandle,
    pub(crate) overrides: overrides::Ui,
    pub(crate) people: people::Ui,
    pub(crate) records: records::Ui,
    pub(crate) recordings: recordings::Ui,
    pub(crate) roles: roles::Ui,
}

impl AdminUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let name = Field::state(text::admin::NAME_FIELD, window, cx);
        let value = Field::state("", window, cx);
        let mut subscriptions = vec![
            Field::observe(
                &name,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.admin_field_changed(cx),
                    FieldEvent::Submitted => this.submit_admin_form(window, cx),
                },
            ),
            Field::observe(
                &value,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.admin_field_changed(cx),
                    FieldEvent::Submitted => this.submit_admin_form(window, cx),
                },
            ),
        ];
        let (overrides, more) = overrides::Ui::new(window, cx);
        subscriptions.extend(more);
        let (people, more) = people::Ui::new(window, cx);
        subscriptions.extend(more);
        let (records, more) = records::Ui::new(window, cx);
        subscriptions.extend(more);
        let (roles, more) = roles::Ui::new(window, cx);
        subscriptions.extend(more);
        let ui = Self {
            name,
            value,
            mode: None,
            new_kind: ChannelKind::Text,
            new_category: None,
            invalid: None,
            sent: None,
            nav_scroll: ScrollHandle::new(),
            page_scroll: ScrollHandle::new(),
            overrides,
            people,
            records,
            recordings: recordings::Ui::default(),
            roles,
        };
        (ui, subscriptions)
    }
}

/// A part of the layer with a name that is built from an identifier, so that a test finds
/// the control of one row.
pub(crate) fn named(selector: String, element: impl IntoElement) -> gpui::Div {
    div()
        .flex_none()
        .debug_selector(move || selector)
        .child(element)
}

/// The heading of a page with the sentence that says what the page is for.
pub(crate) fn page_title(
    title: impl Into<SharedString>,
    help: impl Into<SharedString>,
    theme: &Theme,
) -> gpui::Div {
    let title = title.into();
    div()
        .flex_none()
        .mb(u(12.0))
        .child(
            div()
                .id("admin-page-title")
                .role(Role::Heading)
                .aria_level(2)
                .aria_label(title.clone())
                .typeface(theme.types.heading_large, theme)
                .ink(theme.colors.ink)
                .child(title),
        )
        .child(
            div()
                .mt(u(3.0))
                .typeface(theme.types.small, theme)
                .ink(theme.colors.ink_2)
                .child(help.into()),
        )
}

/// The label of a group of rows on a page.
pub(crate) fn group_label(label: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .mt(u(14.0))
        .mb(u(4.0))
        .typeface(theme.types.label, theme)
        .ink(theme.colors.bronze)
        .child(label.into())
}

/// One row of a page: its content at the left, and its actions at the right.
pub(crate) fn line(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(8.0))
        .min_h(u(40.0))
        .py(u(5.0))
        .border_b_1()
        .border_color(Hsla::from(theme.colors.rule_soft))
}

/// The name of a row with a second line under it. Both are cut at the width of the row.
pub(crate) fn line_text(
    name: impl Into<SharedString>,
    detail: Option<SharedString>,
    theme: &Theme,
) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .typeface(theme.types.control_strong, theme)
                .ink(theme.colors.ink)
                .truncate()
                .child(name.into()),
        )
        .when_some(detail, |column, detail| {
            column.child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .truncate()
                    .child(detail),
            )
        })
}

/// The second line of a row with one action: why the action cannot be used now, and
/// otherwise what it does. A disabled control states its reason without a pointer.
pub(crate) fn detail_or(held: Option<&'static str>, help: &'static str) -> Option<SharedString> {
    Some(held.unwrap_or(help).into())
}

/// A sentence of a page: help under a control, or the statement of an empty list.
pub(crate) fn sentence(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .typeface(theme.types.small, theme)
        .ink(theme.colors.ink_2)
        .child(text.into())
}

/// An action of a row. `selector` names it for one row.
pub(crate) fn row_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selector: String,
    disabled: Option<&'static str>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let button = Button::new(id)
        .label(label)
        .h(u(28.0))
        .px(u(10.0))
        .disabled(disabled.is_some())
        .on_click(on_click);
    named(
        selector,
        match disabled {
            Some(reason) => button.tooltip(reason),
            None => button,
        },
    )
}

/// The statement of a refusal under the control that asked.
pub(crate) fn refusal(reason: Option<&str>, selector: &'static str) -> Option<AnyElement> {
    Some(
        crate::ui::notice_slot(selector)
            .children(reason.map(|reason| Notice::error(reason.to_owned()).selector(selector)))
            .into_any_element(),
    )
}

/// A question in the row that it is about: the sentence, and the two answers.
pub(crate) struct Question {
    pub sentence: SharedString,
    /// The label of the button that confirms.
    pub confirm: &'static str,
    /// The names of the two buttons for a test.
    pub confirm_selector: &'static str,
    pub cancel_selector: &'static str,
    /// The request is on its way.
    pub sending: bool,
}

/// Which field of the layer a form uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormField {
    Name,
    Value,
}

/// A form in a row.
pub(crate) struct Form {
    pub field: FormField,
    /// The label of the field.
    pub label: &'static str,
    pub field_selector: &'static str,
    /// The label of the button that sends the form.
    pub send: &'static str,
    pub sending: bool,
    pub help: Option<&'static str>,
    /// Controls between the field and the buttons.
    pub extra: Vec<AnyElement>,
}

impl Shell {
    /// Opens the administration at a page.
    pub fn open_admin(&mut self, open: AdminOpen, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_admin_ui(window, cx);
        self.state
            .update(cx, |state, cx| state.open_admin(open, cx));
        match self.layer {
            Layer::Admin => {}
            Layer::None => self.open_layer(Layer::Admin, window, cx),
            // The layer takes the place of the menu that opened it; the focus returns
            // to where it was before that menu.
            _ => {
                self.layer = Layer::Admin;
                self.layer_focus().clone().focus(window, cx);
            }
        }
        cx.notify();
    }

    /// Opens the administration of the selected server at its first page.
    pub(crate) fn open_server_admin(
        &mut self,
        page: AdminPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let server_id = self.state.read(cx).view().selected.map(|server| server.id);
        if let Some(server_id) = server_id {
            self.open_admin(AdminOpen::Server { server_id, page }, window, cx);
        }
    }

    /// Opens the settings of a channel of the selected server.
    pub(crate) fn open_channel_settings(
        &mut self,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        page: ChannelPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let server_id = self.state.read(cx).view().selected.map(|server| server.id);
        if let Some(server_id) = server_id {
            let open = AdminOpen::Channel {
                server_id,
                workspace_id,
                channel_id,
                page,
            };
            self.open_admin(open, window, cx);
        }
    }

    /// The fields and the open forms of the layer belong to the page that was shown.
    pub(crate) fn reset_admin_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.admin.mode = None;
        self.admin.invalid = None;
        self.admin.sent = None;
        for field in [self.admin.name.clone(), self.admin.value.clone()] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.reset_overrides_ui(window, cx);
        self.reset_people_ui(window, cx);
        self.reset_records_ui(window, cx);
        self.reset_recordings_ui();
        self.reset_roles_ui(window, cx);
    }

    /// Shows a form or a question in a row, with the fields empty or holding `name`.
    pub fn set_admin_mode(
        &mut self,
        mode: Option<Mode>,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.admin.mode = mode;
        self.admin.invalid = None;
        self.admin.sent = None;
        let name = name.to_owned();
        self.admin
            .name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.admin
            .value
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.state
            .update(cx, |state, cx| state.dismiss_admin_ask(cx));
        match mode {
            Some(
                Mode::CreateWorkspace
                | Mode::RenameWorkspace(_)
                | Mode::CreateCategory
                | Mode::RenameCategory(_)
                | Mode::CreateChannel
                | Mode::RenameChannel,
            ) => self.focus_field(&self.admin.name.clone(), window, cx),
            Some(Mode::EnterCode | Mode::Days(_)) => {
                self.focus_field(&self.admin.value.clone(), window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn admin_field_changed(&mut self, cx: &mut Context<Self>) {
        self.admin.invalid = None;
        cx.notify();
    }

    /// The text of the name field, without the spaces around it.
    pub(crate) fn admin_name(&self, cx: &App) -> String {
        self.admin.name.read(cx).value().trim().to_owned()
    }

    pub(crate) fn admin_value(&self, cx: &App) -> String {
        self.admin.value.read(cx).value().trim().to_owned()
    }

    /// Puts text into the name field of the layer, as typing does.
    pub fn type_admin_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.admin
            .name
            .update(cx, |field, cx| field.set_value(name, window, cx));
    }

    /// Puts text into the field of the layer for a number or a code, as typing does.
    pub fn type_admin_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.to_owned();
        self.admin
            .value
            .update(cx, |field, cx| field.set_value(value, window, cx));
    }

    /// Enter in a field of the layer sends the form of the open row.
    pub fn submit_admin_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A form whose request is on its way is not sent a second time.
        if self.admin.sent.is_some() {
            return;
        }
        let Some(view) = self.state.read(cx).admin_view() else {
            return;
        };
        match self.admin.mode {
            Some(Mode::CreateWorkspace | Mode::RenameWorkspace(_) | Mode::EnterCode) => {
                self.submit_workspace_form(&view, window, cx)
            }
            Some(
                Mode::CreateCategory
                | Mode::RenameCategory(_)
                | Mode::CreateChannel
                | Mode::RenameChannel,
            ) => self.submit_channel_form(&view, window, cx),
            Some(Mode::Days(_)) => self.submit_days(&view, window, cx),
            _ => {}
        }
    }

    /// States that the form or the question of the open row was sent as the request
    /// `what`. The row shows its actions again when the server did it; a refusal keeps the
    /// form, with the reason.
    pub(crate) fn admin_sent(&mut self, what: AskKind) {
        self.admin.sent = Some(what);
    }

    fn settle_admin_form(&mut self, view: &AdminView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(what) = self.admin.sent else {
            return;
        };
        match view.asked(what) {
            Some(crate::state::admin::AskState::Sending) => {}
            Some(crate::state::admin::AskState::Refused(_)) => self.admin.sent = None,
            None => {
                self.admin.sent = None;
                self.admin.mode = None;
                self.admin.invalid = None;
                for field in [self.admin.name.clone(), self.admin.value.clone()] {
                    field.update(cx, |field, cx| field.set_value("", window, cx));
                }
            }
        }
    }

    pub(crate) fn reload_admin_part(
        &mut self,
        server_id: ServerId,
        part: AdminPart,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |state, _| state.reload_admin(server_id, part));
    }

    /// A collection that is on its way or was not read, in place of its rows.
    pub(crate) fn render_reading(
        &mut self,
        server_id: ServerId,
        part: AdminPart,
        failure: Option<LoadFailure>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match failure {
            None => sentence(text::admin::READING, theme)
                .py(u(10.0))
                .debug_selector(|| "admin-reading".to_owned())
                .into_any_element(),
            Some(failure) => div()
                .flex_none()
                .py(u(8.0))
                .child(
                    Notice::error(text::admin::load_failure(failure))
                        .selector("admin-read-failed")
                        .action(
                            Button::new("admin-read-again")
                                .label(text::TRY_AGAIN)
                                .selector("admin-read-again")
                                .h(u(28.0))
                                .px(u(10.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.reload_admin_part(server_id, part, cx);
                                })),
                        ),
                )
                .into_any_element(),
        }
    }

    /// A question in a row, with its two answers.
    pub(crate) fn render_row_question(
        &mut self,
        question: Question,
        theme: &Theme,
        cx: &mut Context<Self>,
        confirmed: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Div {
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
                Button::new("admin-cancel")
                    .label(text::CANCEL)
                    .selector(question.cancel_selector)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_admin_mode(None, "", window, cx);
                    })),
            )
            .child(
                Button::new("admin-confirm")
                    .kind(ButtonKind::Primary)
                    .label(question.confirm)
                    .selector(question.confirm_selector)
                    .disabled(question.sending)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        confirmed(this, window, cx);
                    })),
            )
    }

    /// A form in a row: one field and the two buttons that send it or put it away.
    /// Enter in the field sends the form.
    pub(crate) fn render_form(
        &mut self,
        form: Form,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let field = match form.field {
            FormField::Name => self.admin.name.clone(),
            FormField::Value => self.admin.value.clone(),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Field::new(&field, form.label).selector(form.field_selector)),
                    )
                    .children(form.extra)
                    .child(
                        Button::new("admin-form-cancel")
                            .label(text::CANCEL)
                            .selector("admin-form-cancel")
                            .h(u(30.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_admin_mode(None, "", window, cx);
                            })),
                    )
                    .child(
                        Button::new("admin-form-send")
                            .kind(ButtonKind::Primary)
                            .label(form.send)
                            .selector("admin-form-send")
                            .disabled(form.sending)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_admin_form(window, cx);
                            })),
                    ),
            )
            .child(
                crate::ui::notice_slot("admin-form-invalid-slot").children(
                    self.admin
                        .invalid
                        .map(|invalid| Notice::error(invalid).selector("admin-form-invalid")),
                ),
            )
            .when_some(form.help, |column, help| {
                column.child(sentence(help, theme).mt(u(5.0)))
            })
    }

    pub(crate) fn render_admin(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.state.read(cx).admin_view() else {
            return div().into_any_element();
        };
        self.settle_admin_form(&view, window, cx);
        let theme = theme(cx);
        let colors = theme.colors;
        let focus: FocusHandle = self.layer_focus().clone();
        let by_button = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let by_key = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let by_cover = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let (title, note, nav, page) = match &view.scope {
            Scope::Server {
                nav,
                selected,
                page,
            } => (
                text::admin::TITLE,
                view.server_name.clone(),
                self.render_admin_nav(nav, *selected, &theme, cx),
                self.render_server_page(&view, page, window, cx),
            ),
            Scope::Channel(settings) => (
                text::admin::CHANNEL_SETTINGS,
                text::admin::channel_name(settings.kind, &settings.name),
                self.render_channel_nav(settings, &theme, cx),
                self.render_channel_page(&view, settings, window, cx),
            ),
        };
        let header = SideHeader::new(title).icon(Icon::Gear).note(note).trailing(
            Button::new("admin-close")
                .kind(ButtonKind::Ghost)
                .icon(Icon::X)
                .tooltip(text::CLOSE)
                .selector("admin-close")
                .on_click(move |_, window, cx| by_button(window, cx)),
        );
        let panel = div()
            .id("admin")
            .role(Role::Dialog)
            .aria_label(title)
            .debug_selector(|| "admin".to_owned())
            .w(u(SURFACE_WIDTH))
            .h(u(SURFACE_HEIGHT))
            .max_w_full()
            .max_h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(u(theme.metrics.radius_dialog))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine))
            .shadow(float_shadow(colors.shadow_deep, 14.0, 40.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    by_key(window, cx);
                    cx.stop_propagation();
                }
            })
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(u(NAV_WIDTH))
                            .h_full()
                            .bg(Hsla::from(colors.limestone))
                            .border_r_1()
                            .border_color(Hsla::from(colors.rule))
                            .child(
                                ScrollArea::new(
                                    "admin-nav",
                                    &self.admin.nav_scroll,
                                    text::admin::PAGES,
                                )
                                .child(nav),
                            ),
                    )
                    .child(
                        div().flex().flex_col().flex_1().min_w_0().h_full().child(
                            ScrollArea::new("admin-page", &self.admin.page_scroll, title)
                                .selector("admin-page")
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .pt(u(14.0))
                                        .px(u(18.0))
                                        .pb(u(18.0))
                                        .child(page),
                                ),
                        ),
                    ),
            )
            .focus_trap("admin-trap", &focus);
        div()
            .id("admin-cover")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(u(16.0))
            .bg(Hsla::from(colors.scrim))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| by_cover(window, cx))
            .child(panel)
            .into_any_element()
    }

    fn render_admin_nav(
        &mut self,
        nav: &[crate::state::admin::view::NavGroup],
        selected: AdminPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut column = div().flex().flex_col().pt(u(6.0)).pb(u(10.0));
        for group in nav {
            column = column.child(
                div()
                    .flex_none()
                    .pt(u(10.0))
                    .px(u(14.0))
                    .pb(u(4.0))
                    .typeface(theme.types.label, theme)
                    .ink(theme.colors.bronze)
                    .truncate()
                    .child(text::admin::nav_title(&group.title)),
            );
            for page in group.pages.iter().copied() {
                let name = text::admin::page_name(page);
                column = column.child(
                    ChannelRow::new(name, text::admin::page_icon(page), name)
                        .selected(page == selected)
                        .selector(text::admin::page_selector(page))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_server_admin(page, window, cx);
                        })),
                );
            }
        }
        column.into_any_element()
    }

    fn render_channel_nav(
        &mut self,
        settings: &ChannelSettings,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (workspace_id, channel_id) = (settings.workspace_id, settings.channel_id);
        let mut column = div().flex().flex_col().pt(u(6.0)).pb(u(10.0)).child(
            div()
                .flex_none()
                .pt(u(10.0))
                .px(u(14.0))
                .pb(u(4.0))
                .typeface(theme.types.label, theme)
                .ink(theme.colors.bronze)
                .truncate()
                .child(text::admin::channel_name(settings.kind, &settings.name)),
        );
        for page in settings.pages.iter().copied() {
            let name = text::admin::channel_page_name(page);
            column = column.child(
                ChannelRow::new(name, text::admin::channel_page_icon(page), name)
                    .selected(page == settings.selected)
                    .selector(text::admin::channel_page_selector(page))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_channel_settings(workspace_id, channel_id, page, window, cx);
                    })),
            );
        }
        column.into_any_element()
    }

    fn render_server_page(
        &mut self,
        view: &AdminView,
        page: &ServerPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            ServerPage::Workspaces(page) => self.render_workspaces_page(view, page, window, cx),
            ServerPage::Server(page) => self.render_server_settings_page(view, page, window, cx),
            ServerPage::Channels(page) => self.render_channels_page(view, page, window, cx),
            ServerPage::Archive(page) => self.render_archive_page(view, page, window, cx),
            ServerPage::Members(page) => self.render_members_page(view, page, window, cx),
            ServerPage::Notifications(page) => {
                self.render_notifications_page(view, page, window, cx)
            }
            ServerPage::People(page) => self.render_people_page(view, page, window, cx),
            ServerPage::Invites(page) => self.render_invites_page(view, page, window, cx),
            ServerPage::JoinRequests(page) => self.render_requests_page(view, page, window, cx),
            ServerPage::Tokens(page) => self.render_tokens_page(view, page, window, cx),
            ServerPage::Exports(page) => self.render_exports_page(view, page, window, cx),
            ServerPage::AuditLog(page) => self.render_audit_page(view, page, window, cx),
            ServerPage::Recordings(page) => self.render_recordings_page(view, page, window, cx),
            ServerPage::Roles(page) => self.render_roles_page(view, page, window, cx),
        }
    }

    fn render_channel_page(
        &mut self,
        view: &AdminView,
        settings: &ChannelSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &settings.page {
            ChannelPageView::Overview(page) => {
                self.render_channel_overview(view, settings, page, window, cx)
            }
            ChannelPageView::Permissions(page) => {
                self.render_permissions_page(view, settings, page, window, cx)
            }
            ChannelPageView::Archive(page) => {
                self.render_channel_archive(view, settings, page, window, cx)
            }
        }
    }
}
