//! The account layer (ADR 0781): the profile and the devices of the account on the
//! selected server, in the frame of the administration layer: a list of pages at the left
//! and the open page at the right. A form and a question are drawn in the row that they
//! belong to.

mod devices;
mod profile;

use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, PathPromptOptions, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div,
};
use gpui_base::FocusTrapElement as _;
use gpui_base::input::InputState;
use junto_client_core::protocol::ids::{AccountId, DeviceId, ServerId};

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::account::{AccountOpen, AccountPage, AccountView, AskKind, AskState, PageView};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, ChannelRow, Field, FieldEvent, ScrollArea, SideHeader, StyleExt,
    float_shadow,
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
}

/// What the account layer holds of the window itself: the text of its fields, and which
/// row shows a form or a question.
pub(crate) struct AccountUi {
    /// The name of the account.
    pub(crate) name: Entity<InputState>,
    /// The new name of a device.
    pub(crate) device_name: Entity<InputState>,
    /// The link code of a new device.
    pub(crate) code: Entity<InputState>,
    pub(crate) mode: Option<Mode>,
    /// Why the text of the code field was not sent.
    pub(crate) invalid: Option<&'static str>,
    /// Why a file that the dialog returned was not sent as the picture.
    pub(crate) picture_refused: Option<&'static str>,
    /// The request that the open form or question was sent as.
    pub(crate) sent: Option<AskKind>,
    pub(crate) nav_scroll: ScrollHandle,
    pub(crate) page_scroll: ScrollHandle,
    /// What the layer drew last. A test reads the layer from it.
    #[cfg(feature = "test-support")]
    pub(crate) drawn: Option<AccountView>,
}

impl AccountUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let name = Field::state(text::account::NAME_FIELD, window, cx);
        let device_name = Field::state(text::account::DEVICE_NAME, window, cx);
        let code = Field::state("", window, cx);
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
            Field::observe(
                &code,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.link_code_changed(cx),
                    FieldEvent::Submitted => this.submit_link_code(window, cx),
                },
            ),
        ];
        let ui = Self {
            name,
            device_name,
            code,
            mode: None,
            invalid: None,
            picture_refused: None,
            sent: None,
            nav_scroll: ScrollHandle::new(),
            page_scroll: ScrollHandle::new(),
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
            .flex()
            .items_center()
            .justify_center()
            .p(u(16.0))
            .bg(Hsla::from(colors.scrim))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| by_cover(window, cx))
            .child(panel)
            .into_any_element()
    }

    /// Opens the account layer of the selected server at a page.
    pub fn open_account(&mut self, page: AccountPage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server_id) = self.state.read(cx).view().selected.map(|server| server.id) else {
            return;
        };
        self.reset_account_ui(window, cx);
        self.state.update(cx, |state, cx| {
            state.open_account(AccountOpen { server_id, page }, cx);
        });
        // The name field starts with the name that the server holds.
        let held = self
            .state
            .read(cx)
            .account_view()
            .and_then(|view| match view.page {
                PageView::Profile(page) => Some(page.name),
                PageView::Devices(_) => None,
            });
        if let Some(name) = held {
            self.account
                .name
                .update(cx, |field, cx| field.set_value(name, window, cx));
        }
        match self.layer {
            Layer::Account => {}
            Layer::None => self.open_layer(Layer::Account, window, cx),
            // The layer takes the place of the menu that opened it; the focus returns
            // to where it was before that menu.
            _ => {
                self.layer = Layer::Account;
                self.layer_focus().clone().focus(window, cx);
            }
        }
        cx.notify();
    }

    /// The fields and the open form of the layer belong to the page that was shown. A
    /// code is not kept in a field that nobody sees.
    pub(crate) fn reset_account_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account.mode = None;
        self.account.invalid = None;
        self.account.picture_refused = None;
        self.account.sent = None;
        #[cfg(feature = "test-support")]
        {
            self.account.drawn = None;
        }
        for field in [
            self.account.name.clone(),
            self.account.device_name.clone(),
            self.account.code.clone(),
        ] {
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
                    self.account.mode = None;
                    self.account
                        .device_name
                        .update(cx, |field, cx| field.set_value("", window, cx));
                }
            }
        }
        self.settle_link_code(view, window, cx);
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
        if !matches!(self.layer, Layer::Account) {
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

    /// True while the account layer is open.
    pub fn account_is_open(&self) -> bool {
        matches!(self.layer, Layer::Account)
    }

    pub(crate) fn render_account(
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
        let theme = theme(cx);
        let nav = self.render_account_nav(view.selected, &theme, cx);
        let page = match &view.page {
            PageView::Profile(page) => self.render_profile_page(&view, page, window, cx),
            PageView::Devices(page) => self.render_devices_page(&view, page, window, cx),
        };
        let surface = Surface {
            id: "account",
            title: text::account::TITLE,
            icon: Icon::People,
            note: view.server_name.clone().into(),
            close_selector: "account-close",
            nav: Some((nav, text::account::PAGES, self.account.nav_scroll.clone())),
            page,
            page_scroll: self.account.page_scroll.clone(),
        };
        self.render_surface(surface, cx)
    }

    fn render_account_nav(
        &mut self,
        selected: AccountPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut column = div().flex().flex_col().pt(u(6.0)).pb(u(10.0)).child(
            div()
                .flex_none()
                .pt(u(10.0))
                .px(u(14.0))
                .pb(u(4.0))
                .typeface(theme.types.label, theme)
                .ink(theme.colors.bronze)
                .truncate()
                .child(text::account::ON_THIS_SERVER),
        );
        for page in [AccountPage::Profile, AccountPage::Devices] {
            let name = text::account::page_name(page);
            column = column.child(
                ChannelRow::new(name, text::account::page_icon(page), name)
                    .selected(page == selected)
                    .selector(text::account::page_selector(page))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_account(page, window, cx);
                    })),
            );
        }
        column.into_any_element()
    }
}
