//! The account layer (ADR 0781): the profile, the devices and the recoveries of the account
//! on the selected server, in the frame of the administration layer: a list of pages at the left
//! and the open page at the right. A form and a question are drawn in the row that they
//! belong to.

mod departure;
mod devices;
mod profile;
mod recoveries;

use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, PathPromptOptions, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div,
};
use gpui_base::FocusTrapElement as _;
use gpui_base::input::InputState;
use junto_client_core::protocol::ids::{AccountId, DeviceId, ServerId, WorkspaceId};

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::account::{AccountPage, AccountView, AskKind, AskState, PageView};
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{
    Button, ButtonKind, Field, FieldEvent, FormSpace, ScrollArea, SideHeader, float_shadow,
};

/// The width and the height of a layer with pages, in design pixels.
const SURFACE_WIDTH: f32 = 880.0;
const SURFACE_HEIGHT: f32 = 620.0;
const NAV_WIDTH: f32 = 208.0;

/// The row of the devices page that shows a form or a question in place of its actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    RenameDevice(DeviceId),
    RemoveDevice(DeviceId),
    /// The question before the account leaves a workspace (ADR 0121).
    LeaveWorkspace(WorkspaceId),
    /// The question before the account is deleted (ADR 0121).
    DeleteAccount,
}

impl Mode {
    /// The result of the request of the form or question (ADR 0730).
    fn done(self) -> &'static str {
        match self {
            Self::RenameDevice(_) => text::account::DEVICE_NAME_SAVED,
            Self::RemoveDevice(_) => text::account::DEVICE_REMOVED,
            Self::LeaveWorkspace(_) => text::account::WORKSPACE_LEFT,
            Self::DeleteAccount => text::account::ACCOUNT_DELETED,
        }
    }
}

/// What the account layer holds of the window itself: the text of its fields, and which
/// row shows a form or a question.
pub(crate) struct AccountUi {
    /// The name of the account.
    pub(crate) name: Entity<InputState>,
    /// The new name of a device.
    pub(crate) device_name: Entity<InputState>,
    pub(crate) mode: Option<Mode>,
    /// Why a file that the dialog returned was not sent as the picture.
    pub(crate) picture_refused: Option<&'static str>,
    /// The request that the open form or question was sent as.
    pub(crate) sent: Option<AskKind>,
    /// The space of the rows of the devices with a form or a question (ADR 0730).
    pub(crate) space: FormSpace<Mode>,
    /// What the layer drew last. A test reads the layer from it.
    #[cfg(feature = "test-support")]
    pub(crate) drawn: Option<AccountView>,
}

impl AccountUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let name = Field::state(text::account::NAME_FIELD, window, cx);
        let device_name = Field::state(text::account::DEVICE_NAME, window, cx);
        let subscriptions = vec![
            Field::observe(
                &name,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.account_name_changed(cx),
                    FieldEvent::Submitted => this.submit_account_name(window, cx),
                },
            ),
            Field::observe(
                &device_name,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => cx.notify(),
                    FieldEvent::Submitted => this.submit_device_name(window, cx),
                },
            ),
        ];
        let ui = Self {
            name,
            device_name,
            mode: None,
            picture_refused: None,
            sent: None,
            space: FormSpace::default(),
            #[cfg(feature = "test-support")]
            drawn: None,
        };
        (ui, subscriptions)
    }
}

/// A layer above the window with a header, an optional list of pages at the left, and
/// one page that scrolls.
pub(crate) struct Surface {
    /// The name of the layer for its element and for a test.
    pub id: &'static str,
    pub title: &'static str,
    pub icon: Icon,
    /// The short text after the title: the server that the layer is about.
    pub note: SharedString,
    pub close_selector: &'static str,
    /// The list of pages, with the label of the list and its scroll position.
    pub nav: Option<(AnyElement, &'static str, ScrollHandle)>,
    pub page: AnyElement,
    pub page_scroll: ScrollHandle,
}

impl Shell {
    /// Draws a layer with pages. The layer holds the keyboard focus; Escape, its close
    /// button and a click outside close it.
    pub(crate) fn render_surface(
        &mut self,
        surface: Surface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let focus: FocusHandle = self.layer_focus().clone();
        let by_button = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let by_key = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let by_cover = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let (id, title) = (surface.id, surface.title);
        let header = SideHeader::new(title)
            .icon(surface.icon)
            .note(surface.note)
            .trailing(
                Button::new((id, 0usize))
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE)
                    .selector(surface.close_selector)
                    .on_click(move |_, window, cx| by_button(window, cx)),
            );
        let nav = surface.nav.map(|(nav, label, scroll)| {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(u(NAV_WIDTH))
                .h_full()
                .bg(Hsla::from(colors.limestone))
                .border_r_1()
                .border_color(Hsla::from(colors.rule))
                .child(ScrollArea::new((id, 1usize), &scroll, label).child(nav))
        });
        let panel = div()
            .id(id)
            .role(Role::Dialog)
            .aria_label(title)
            .debug_selector(move || id.to_owned())
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
                div().flex().flex_1().min_h_0().children(nav).child(
                    div().flex().flex_col().flex_1().min_w_0().h_full().child(
                        ScrollArea::new((id, 2usize), &surface.page_scroll, title).child(
                            div()
                                .flex()
                                .flex_col()
                                .pt(u(14.0))
                                .px(u(18.0))
                                .pb(u(18.0))
                                .child(surface.page),
                        ),
                    ),
                ),
            )
            .focus_trap((id, 3usize), &focus);
        div()
            .id((id, 4usize))
            .absolute()
            .inset_0()
            .occlude()
            .child(crate::ui::cover())
            .flex()
            .items_center()
            .justify_center()
            .p(u(16.0))
            .pt(u(crate::shell::title_bar::cover_top(window, cx)))
            .bg(Hsla::from(colors.scrim))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| by_cover(window, cx))
            .child(panel)
            .children(crate::shell::title_bar::layer_band(window, cx))
            .into_any_element()
    }

    /// Opens Server settings of the selected server at a page of the account (ADR 0781).
    pub fn open_account(&mut self, page: AccountPage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server_id) = self.state.read(cx).view().selected.map(|server| server.id) else {
            return;
        };
        self.open_admin(
            crate::state::admin::AdminOpen::Server {
                server_id,
                page: crate::state::admin::AdminPage::of_account(page),
            },
            window,
            cx,
        );
    }

    /// Readies a page of the account that Server settings open, whatever opened it: the menu,
    /// the list of pages or a link. The name field starts with the name that the server holds.
    pub(crate) fn ready_account_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_account_ui(window, cx);
        let held = self
            .state
            .read(cx)
            .account_view()
            .and_then(|view| match view.page {
                PageView::Profile(page) => Some(page.name),
                PageView::Devices(_) | PageView::Recoveries(_) | PageView::Departure(_) => None,
            });
        if let Some(name) = held {
            self.account
                .name
                .update(cx, |field, cx| field.set_value(name, window, cx));
        }
        cx.notify();
    }

    /// The fields and the open form of the layer belong to the page that was shown. A
    /// code is not kept in a field that nobody sees.
    pub(crate) fn reset_account_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account.mode = None;
        self.account.picture_refused = None;
        self.account.sent = None;
        self.account.space.release();
        #[cfg(feature = "test-support")]
        {
            self.account.drawn = None;
        }
        for field in [self.account.name.clone(), self.account.device_name.clone()] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// Shows a form or a question in the row of a device, with the name field holding
    /// `name`.
    pub fn set_account_mode(
        &mut self,
        mode: Option<Mode>,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.account.mode = mode;
        self.account.sent = None;
        self.account.space.release();
        let name = name.to_owned();
        self.account
            .device_name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.state
            .update(cx, |state, cx| state.dismiss_account_ask(cx));
        if matches!(mode, Some(Mode::RenameDevice(_))) {
            self.focus_field(&self.account.device_name.clone(), window, cx);
        }
        cx.notify();
    }

    /// A row shows its actions again when the server did what its form or question
    /// asked for. A refusal keeps the form, with the reason.
    fn settle_account_form(
        &mut self,
        view: &AccountView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(what) = self.account.sent {
            match view.asked(what) {
                Some(AskState::Sending) => {}
                Some(AskState::Refused(_)) => self.account.sent = None,
                None => {
                    self.account.sent = None;
                    // The answer arrives while the person can be pointing at a control
                    // under the row: the row keeps the height of its form or question.
                    if let Some(mode) = self.account.mode.take() {
                        self.account.space.keep(mode, mode.done());
                    }
                    self.account
                        .device_name
                        .update(cx, |field, cx| field.set_value("", window, cx));
                }
            }
        }
    }

    fn account_name_changed(&mut self, cx: &mut Context<Self>) {
        // What was told about the name before belongs to the text before.
        let told = self
            .state
            .read(cx)
            .account_view()
            .is_some_and(|view| view.refused(AskKind::SaveName).is_some());
        if told {
            self.state
                .update(cx, |state, cx| state.dismiss_account_ask(cx));
        }
        cx.notify();
    }

    /// The text of the name field of the profile page. The window tests read it.
    pub fn account_name_typed(&self, cx: &App) -> String {
        self.account.name.read(cx).value().to_string()
    }

    /// Puts text into the name field of the profile page, as typing does.
    pub fn type_account_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.account
            .name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.account_name_changed(cx);
    }

    /// Sends the text of the name field as the name of the account. Nothing is sent
    /// while a request of the page is on its way: the page sends one at a time.
    pub fn submit_account_name(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).account_busy() {
            return;
        }
        let name = self.account.name.read(cx).value().to_string();
        self.state
            .update(cx, |state, cx| state.save_account_name(&name, cx));
    }

    /// Opens the file dialog of the system for the picture of the account.
    fn choose_account_picture(&mut self, cx: &mut Context<Self>) {
        self.account.picture_refused = None;
        cx.notify();
        let asked = self.account_owner(cx);
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    // The dialog can outlast the layer that opened it: a file is sent
                    // only for the server and the account that it was chosen for.
                    if asked.is_some() && this.account_owner(cx) == asked {
                        this.set_account_picture_path(path, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// The server and the account of the open layer.
    fn account_owner(&self, cx: &App) -> Option<(ServerId, AccountId)> {
        self.state
            .read(cx)
            .account_view()
            .map(|view| (view.server_id, view.account_id))
    }

    /// Sends the file at this path as the picture of the account, as the file dialog
    /// does. The dialog can return a file while another request of the page is on its
    /// way: that file is not sent, and the page says so at the button.
    pub fn set_account_picture_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.account_is_open(cx) {
            return;
        }
        if self.state.read(cx).account_busy() {
            self.account.picture_refused = Some(text::account::PAGE_BUSY);
            cx.notify();
            return;
        }
        self.account.picture_refused = None;
        self.state
            .update(cx, |state, cx| state.set_account_picture(path, cx));
    }

    /// Why a file that the dialog returned was not sent as the picture.
    pub fn account_picture_refusal(&self) -> Option<&'static str> {
        self.account.picture_refused
    }

    /// What the layer drew last.
    #[cfg(feature = "test-support")]
    pub fn account_drawn(&self) -> Option<&AccountView> {
        self.account.drawn.as_ref()
    }

    /// The scroll position of the page of the account layer, for the window tests.
    #[cfg(feature = "test-support")]
    pub fn account_scroll_handle(&self) -> ScrollHandle {
        self.admin.page_scroll.clone()
    }

    /// True while Server settings show a page of the account.
    pub fn account_is_open(&self, cx: &App) -> bool {
        matches!(self.layer, Layer::Admin) && self.state.read(cx).account_open().is_some()
    }

    /// The open page of the account, in the frame of Server settings (ADR 0781).
    pub(crate) fn render_account_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.state.read(cx).account_view() else {
            return div().into_any_element();
        };
        self.settle_account_form(&view, window, cx);
        #[cfg(feature = "test-support")]
        {
            self.account.drawn = Some(view.clone());
        }
        div()
            .id("account")
            .debug_selector(|| "account".to_owned())
            .flex()
            .flex_col()
            .child(match &view.page {
                PageView::Profile(page) => self.render_profile_page(&view, page, window, cx),
                PageView::Devices(page) => self.render_devices_page(&view, page, window, cx),
                PageView::Recoveries(list) => self.render_recoveries_page(list, window, cx),
                PageView::Departure(page) => self.render_departure_page(&view, page, cx),
            })
            .into_any_element()
    }
}
