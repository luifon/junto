//! The main window (ADR 0703, ADR 0731): the server book, the channel column, the main
//! area with its side panel, and the layers above them. The window draws what
//! [`AppState::view`] gives and holds only what belongs to the window itself: which layer
//! is open, the text of its fields, and where the keyboard focus returns to.

pub mod call;
mod channels;
mod join;
mod main_area;
mod rail;
mod server_menu;
mod settings;

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, Modifiers,
    ModifiersChangedEvent, ParentElement, Render, ScrollHandle, Styled, Subscription, Task,
    WeakFocusHandle, Window, div, prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::flow::add_server::AddServerFlow;
use junto_client_core::protocol::ids::{AccountId, ServerId};
use junto_client_core::voice::{InputMode, TalkKey};

use call::{CallTab, DeviceKind, VolumeControl};

use crate::state::AppState;
use crate::state::view::ShellView;
use crate::text;
use crate::theme::{self, theme};
use crate::ui::{Field, FieldEvent, StyleExt, tabular_numerals};

/// The layer that lies over the window content.
pub(crate) enum Layer {
    None,
    /// The first start: the person chooses a name.
    Welcome,
    Settings,
    Join(join::Join),
    ServerMenu,
    ConfirmRemove(ServerId),
    ConfirmForget(ServerId),
    /// The list of the input or of the output devices, in the sound settings of a call.
    DeviceMenu(DeviceKind),
}

/// A part of the window that can be opened without a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    Settings,
    ServerMenu,
    Members,
    /// The menu of the first server of the collapsed server book.
    ServerHoverMenu,
    /// A pane of the panel of the call.
    CallTab(CallTab),
    /// The list of the input devices, in the sound settings of the call.
    InputDevices,
}

/// How long the menu of a collapsed server stays after the pointer has left it, so that
/// the pointer can move from the icon into the menu.
const HOVER_MENU_LINGER: Duration = Duration::from_millis(220);

pub struct Shell {
    pub(crate) state: Entity<AppState>,
    focus: FocusHandle,
    /// The focus of the layer that is open.
    layer_focus: FocusHandle,
    pub(crate) layer: Layer,
    /// Where the keyboard focus was before the layer opened.
    return_focus: Option<WeakFocusHandle>,

    // The server book.
    pub(crate) address: Entity<InputState>,
    /// The flow of the address that was sent last, while the server answers.
    pub(crate) adding: Option<AddServerFlow>,
    pub(crate) address_error: Option<String>,
    /// The server whose menu is open in the collapsed server book, and whether the
    /// keyboard opened it.
    pub(crate) hover_menu: Option<(ServerId, bool)>,
    hover_menu_focus: FocusHandle,
    hover_menu_close: Option<Task<()>>,
    /// The press of the pointer that closed the menu of the server was on the button of
    /// that menu: the click that follows does not open the menu again.
    pub(crate) server_menu_closed_by_press: bool,
    /// The person expanded the server book while the window is narrow.
    expanded_while_narrow: bool,
    /// The width of the server book as it was drawn last, in design pixels.
    pub(crate) rail_width: f32,

    // The side panel.
    pub(crate) members_open: bool,

    // The call.
    pub(crate) call_tab: CallTab,
    /// The panel of the call lies over the content of a narrow window.
    pub(crate) call_panel_open: bool,
    pub(crate) volumes: HashMap<(ServerId, AccountId), VolumeControl>,
    /// The press of the pointer that closed a list of devices was on the box of that
    /// list: the click that follows does not open the list again.
    pub(crate) device_menu_closed_by_press: bool,
    // The positions of the lists that scroll.
    pub(crate) servers_scroll: ScrollHandle,
    pub(crate) channels_scroll: ScrollHandle,
    pub(crate) members_scroll: ScrollHandle,
    pub(crate) call_members_scroll: ScrollHandle,
    pub(crate) call_settings_scroll: ScrollHandle,

    // The fields of the layers.
    pub(crate) name: Entity<InputState>,
    pub(crate) name_error: Option<String>,
    pub(crate) name_saved: bool,
    pub(crate) code: Entity<InputState>,
    /// The number of join dialogs that this window has opened.
    pub(crate) joins_opened: u64,
    /// The number of results of join actions that this window has received.
    pub(crate) join_results: u64,
    /// What the window drew last. A test reads the data of the window from it, and not
    /// from the client core, which can be ahead of the window.
    #[cfg(feature = "test-support")]
    drawn: Option<ShellView>,

    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = Field::state(text::ADDRESS_PLACEHOLDER, window, cx);
        let name = Field::state(text::NAME_FIELD, window, cx);
        let code = Field::state("", window, cx);
        let subscriptions = vec![
            // A key that is held when the window loses the focus is not seen when it is
            // released: push-to-talk ends with the focus (ADR 0507).
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.set_talking(false, cx);
                }
            }),
            cx.observe_in(&state, window, |this, state, window, cx| {
                if let Some((submitted, result)) =
                    state.update(cx, |state, _| state.take_name_result())
                {
                    this.name_was_decided(&submitted, result, window, cx);
                }
                // A layer whose server is gone, or whose work is done, has nothing to show.
                let view = state.read(cx).view();
                this.drop_stale_layer(&view, window, cx);
                cx.notify();
            }),
            Field::observe(
                &address,
                window,
                cx,
                |this: &mut Self, event, window, cx| match event {
                    FieldEvent::Changed => {
                        this.address_error = None;
                        cx.notify();
                    }
                    FieldEvent::Submitted => this.submit_address(window, cx),
                },
            ),
            Field::observe(
                &name,
                window,
                cx,
                |this: &mut Self, event, window, cx| match event {
                    FieldEvent::Changed => {
                        this.name_error = None;
                        this.name_saved = false;
                        cx.notify();
                    }
                    FieldEvent::Submitted => this.submit_name(window, cx),
                },
            ),
            Field::observe(
                &code,
                window,
                cx,
                |this: &mut Self, event, window, cx| match event {
                    FieldEvent::Changed => this.code_changed(cx),
                    FieldEvent::Submitted => this.join_with_code(window, cx),
                },
            ),
        ];
        let mut shell = Self {
            state,
            focus: cx.focus_handle(),
            layer_focus: cx.focus_handle(),
            layer: Layer::None,
            return_focus: None,
            address,
            adding: None,
            address_error: None,
            hover_menu: None,
            hover_menu_focus: cx.focus_handle(),
            hover_menu_close: None,
            server_menu_closed_by_press: false,
            expanded_while_narrow: false,
            rail_width: 0.0,
            members_open: false,
            call_tab: CallTab::default(),
            call_panel_open: false,
            volumes: HashMap::new(),
            device_menu_closed_by_press: false,
            servers_scroll: ScrollHandle::new(),
            channels_scroll: ScrollHandle::new(),
            members_scroll: ScrollHandle::new(),
            call_members_scroll: ScrollHandle::new(),
            call_settings_scroll: ScrollHandle::new(),
            name,
            name_error: None,
            name_saved: false,
            code,
            joins_opened: 0,
            join_results: 0,
            #[cfg(feature = "test-support")]
            drawn: None,
            _subscriptions: subscriptions,
        };
        if shell.state.read(cx).preset_name().is_empty() {
            shell.open_layer(Layer::Welcome, window, cx);
            shell.focus_field(&shell.name.clone(), window, cx);
        } else {
            // The window holds the keyboard focus from its start, so that the key of
            // push-to-talk reaches it before the first click.
            shell.focus.focus(window, cx);
        }
        shell
    }

    /// The pane of the panel of the call that is shown.
    pub fn call_tab(&self) -> CallTab {
        self.call_tab
    }

    /// The modifier keys changed. With push-to-talk, the chosen key sends the microphone
    /// while it is held.
    fn modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let held = {
            let voice = self.state.read(cx).voice();
            voice.input_mode == InputMode::PushToTalk
                && match voice.talk_key {
                    TalkKey::Alt => modifiers.alt,
                    TalkKey::Control => modifiers.control,
                }
        };
        self.set_talking(held, cx);
    }

    /// The calls keep the state of the key and act on a change only, so the window tells
    /// them what holds and keeps no copy that could differ.
    fn set_talking(&mut self, talking: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _| state.set_talking(talking));
    }

    pub fn state(&self) -> &Entity<AppState> {
        &self.state
    }

    /// The server of the server book that the open join dialog is about.
    pub fn join_target(&self) -> Option<ServerId> {
        match &self.layer {
            Layer::Join(join) => match &join.target {
                join::Target::Saved(server_id) => Some(*server_id),
                join::Target::Preview(_) => None,
            },
            _ => None,
        }
    }

    /// The number of results of join actions that this window has received, for whichever
    /// dialog was open then.
    pub fn join_results(&self) -> u64 {
        self.join_results
    }

    /// What the window drew last.
    #[cfg(feature = "test-support")]
    pub fn drawn(&self) -> Option<&ShellView> {
        self.drawn.as_ref()
    }

    /// True when the settings mark the text of the name field as the stored name.
    pub fn name_is_marked_saved(&self) -> bool {
        self.name_saved
    }

    /// True while the dialog that joins a server is open.
    pub fn join_is_open(&self) -> bool {
        matches!(self.layer, Layer::Join(_))
    }

    /// True when the server book shows why the typed address was not accepted.
    pub fn address_was_refused(&self) -> bool {
        self.address_error.is_some()
    }

    /// Puts text into the address field of the server book, as typing does.
    pub fn type_address(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        let address = address.to_owned();
        self.address
            .update(cx, |field, cx| field.set_value(address, window, cx));
    }

    /// Moves the keyboard focus into the address field of the server book.
    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_field(&self.address.clone(), window, cx);
    }

    /// Puts text into the name field of the first start and of the settings, as typing
    /// does.
    pub fn type_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.name
            .update(cx, |field, cx| field.set_value(name, window, cx));
    }

    /// The sentence under the address field that tells why an address was not accepted.
    pub fn address_error(&self) -> Option<&str> {
        self.address_error.as_deref()
    }

    /// The failure that the join dialog shows for its last action.
    pub fn join_error(&self) -> Option<&str> {
        match &self.layer {
            Layer::Join(join) => join.error.as_deref(),
            _ => None,
        }
    }

    /// True while the keyboard focus is inside the open layer.
    pub fn layer_holds_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        !matches!(self.layer, Layer::None) && self.layer_focus.contains_focused(window, cx)
    }

    /// True while the member list is open.
    pub fn members_are_open(&self) -> bool {
        self.members_open
    }

    /// Puts text into the code field of the join dialog, as typing does.
    pub fn type_code(&mut self, code: &str, window: &mut Window, cx: &mut Context<Self>) {
        let code = code.to_owned();
        self.code
            .update(cx, |field, cx| field.set_value(code, window, cx));
    }

    /// Asks whether a server is to be removed from this device.
    pub fn ask_to_remove(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(Layer::ConfirmRemove(server_id), window, cx);
    }

    /// Opens a part of the window that a start argument or a tool names.
    pub fn open(&mut self, open: Open, window: &mut Window, cx: &mut Context<Self>) {
        match open {
            Open::Settings => self.open_settings(window, cx),
            Open::ServerMenu => self.open_server_menu(window, cx),
            Open::Members => {
                self.members_open = true;
                cx.notify();
            }
            Open::ServerHoverMenu => {
                let first = self.state.read(cx).view().servers.first().map(|row| row.id);
                if let Some(server_id) = first {
                    self.open_hover_menu(server_id, true, window, cx);
                }
            }
            Open::CallTab(tab) => {
                self.call_panel_open = true;
                self.show_call_tab(tab, cx);
            }
            Open::InputDevices => {
                self.call_panel_open = true;
                self.show_call_tab(CallTab::Settings, cx);
                self.open_device_menu(DeviceKind::Input, window, cx);
            }
        }
    }

    pub(crate) fn open_layer(&mut self, layer: Layer, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.layer, Layer::None) {
            self.return_focus = window.focused(cx).map(|handle| handle.downgrade());
        }
        self.close_hover_menu(cx);
        self.layer = layer;
        self.layer_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.layer, Layer::Join(_)) {
            // A code is not kept in a field that nobody sees.
            self.code
                .update(cx, |code, cx| code.set_value("", window, cx));
        }
        self.layer = Layer::None;
        self.return_focus_to_content(window, cx);
        cx.notify();
    }

    /// A control that holds the keyboard focus can leave the window: a button is disabled
    /// while its action runs. The focus then returns to the open layer, so that Escape and
    /// Tab still reach it.
    fn keep_focus_in_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.layer, Layer::None) && !self.layer_focus.contains_focused(window, cx) {
            self.layer_focus.focus(window, cx);
        }
    }

    /// A control that holds the keyboard focus can leave the window: the button that
    /// joins a call is gone when the call begins. The focus then returns to the window, so
    /// that Tab and the key of push-to-talk still reach it.
    fn keep_focus_in_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.layer, Layer::None) && !self.focus.contains_focused(window, cx) {
            self.focus.focus(window, cx);
        }
    }

    fn return_focus_to_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.return_focus.take().and_then(|handle| handle.upgrade()) {
            Some(handle) => handle.focus(window, cx),
            None => self.focus.focus(window, cx),
        }
    }

    pub(crate) fn focus_field(
        &self,
        field: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        field.update(cx, |field, cx| field.focus(window, cx));
    }

    /// A handler for a component that tells no event, such as the close of a menu.
    pub(crate) fn handler(
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let listener = cx.listener(move |this, _: &(), window, cx| run(this, window, cx));
        move |window, cx| listener(&(), window, cx)
    }

    pub(crate) fn layer_focus(&self) -> &FocusHandle {
        &self.layer_focus
    }

    pub(crate) fn hover_menu_focus(&self) -> &FocusHandle {
        &self.hover_menu_focus
    }

    /// Opens the menu of a server of the collapsed server book. The keyboard path moves
    /// the focus into the menu; the pointer path leaves the focus where it is.
    pub(crate) fn open_hover_menu(
        &mut self,
        server_id: ServerId,
        by_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hover_menu_close = None;
        let already = self.hover_menu.map(|(open, _)| open) == Some(server_id);
        let keeps_focus = self.hover_menu.is_some_and(|(_, keyboard)| keyboard);
        if already && (keeps_focus || !by_keyboard) {
            return;
        }
        if by_keyboard {
            self.return_focus = window.focused(cx).map(|handle| handle.downgrade());
            self.hover_menu_focus.focus(window, cx);
        }
        self.hover_menu = Some((server_id, by_keyboard));
        cx.notify();
    }

    /// The pointer left the icon or the menu: the menu closes after a short time unless
    /// the pointer enters one of them again. A menu that the keyboard opened stays.
    pub(crate) fn leave_hover_menu(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.hover_menu, Some((_, false))) {
            return;
        }
        self.hover_menu_close = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_MENU_LINGER).await;
            let _ = this.update(cx, |this, cx| {
                this.hover_menu = None;
                this.hover_menu_close = None;
                cx.notify();
            });
        }));
    }

    pub(crate) fn stay_in_hover_menu(&mut self) {
        self.hover_menu_close = None;
    }

    pub(crate) fn close_hover_menu(&mut self, cx: &mut Context<Self>) {
        self.hover_menu_close = None;
        if self.hover_menu.take().is_some() {
            cx.notify();
        }
    }

    /// Closes a menu that holds the keyboard focus and returns the focus.
    pub(crate) fn dismiss_hover_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let by_keyboard = matches!(self.hover_menu, Some((_, true)));
        self.close_hover_menu(cx);
        if by_keyboard {
            self.return_focus_to_content(window, cx);
        }
    }

    pub(crate) fn toggle_server_book(&mut self, narrow: bool, cx: &mut Context<Self>) {
        self.close_hover_menu(cx);
        if narrow {
            self.expanded_while_narrow = !self.expanded_while_narrow;
            cx.notify();
            return;
        }
        let collapsed = !self.state.read(cx).settings().server_book_collapsed;
        self.state.update(cx, |state, cx| {
            state.set_server_book_collapsed(collapsed, cx)
        });
    }

    /// The address field of the server book is the first step of joining a server. In the
    /// collapsed book the "+" button expands the book and moves the focus into the field.
    pub(crate) fn show_address_field(
        &mut self,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if narrow {
            self.expanded_while_narrow = true;
        } else {
            self.state
                .update(cx, |state, cx| state.set_server_book_collapsed(false, cx));
        }
        self.close_hover_menu(cx);
        self.focus_field(&self.address.clone(), window, cx);
        cx.notify();
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The width of the window in design pixels at the present interface scale.
fn logical_width(window: &Window) -> f32 {
    let rem = window.rem_size();
    window.viewport_size().width / rem * theme::REM_AT_100
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (view, settings): (ShellView, _) = {
            let state = self.state.read(cx);
            (state.view(), state.settings())
        };
        #[cfg(feature = "test-support")]
        {
            self.drawn = Some(view.clone());
        }
        window.set_rem_size(theme::rem_size(settings.scale_percent));
        let theme = theme(cx);
        let narrow = logical_width(window) < theme.metrics.narrow_below;
        if !narrow {
            self.expanded_while_narrow = false;
        }
        let collapsed = if narrow {
            !self.expanded_while_narrow
        } else {
            settings.server_book_collapsed
        };
        if !matches!(self.layer, Layer::None) {
            cx.defer_in(window, |this, window, cx| {
                this.keep_focus_in_layer(window, cx)
            });
        } else if !self.focus.contains_focused(window, cx) {
            cx.defer_in(window, |this, window, cx| {
                this.keep_focus_in_window(window, cx)
            });
        }
        self.rail_width = if collapsed {
            theme.metrics.rail_collapsed_width
        } else {
            theme.metrics.rail_width
        };
        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .overflow_hidden()
            .bg(Hsla::from(theme.colors.travertine))
            .typeface(theme.types.body, &theme)
            .font_features(tabular_numerals())
            .ink(theme.colors.ink)
            // After every click: a press that closed the menu of the server is over.
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.server_menu_closed_by_press = false;
                    this.device_menu_closed_by_press = false;
                }),
            )
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.modifiers_changed(event.modifiers, cx);
            }))
            .child(self.render_rail(&view, collapsed, narrow, window, cx))
            .child(self.render_channel_column(&view, window, cx))
            .child(self.render_main(&view, narrow, window, cx))
            .when(!matches!(self.layer, Layer::None), |shell| {
                shell.child(self.render_layer(&view, window, cx))
            })
    }
}

impl Shell {
    fn drop_stale_layer(&mut self, view: &ShellView, window: &mut Window, cx: &mut Context<Self>) {
        let gone = |server_id: &ServerId| !view.servers.iter().any(|row| row.id == *server_id);
        let stale = match &self.layer {
            Layer::ConfirmRemove(server_id) | Layer::ConfirmForget(server_id) => gone(server_id),
            Layer::ServerMenu => view.selected.is_none(),
            Layer::Join(join) => join.is_stale(view),
            // The list of devices belongs to the sound settings of the call.
            Layer::DeviceMenu(_) => {
                self.call_tab != CallTab::Settings
                    || view
                        .workspace()
                        .is_none_or(|workspace| workspace.call.is_none())
            }
            Layer::None | Layer::Welcome | Layer::Settings => false,
        };
        if stale {
            self.close_layer(window, cx);
        }
    }

    fn render_layer(
        &mut self,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match &self.layer {
            // The list of devices is drawn at its box, in the panel of the call.
            Layer::None | Layer::DeviceMenu(_) => div().into_any_element(),
            Layer::Welcome => self.render_welcome(window, cx).into_any_element(),
            Layer::Settings => self.render_settings(window, cx).into_any_element(),
            Layer::Join(_) => self.render_join(view, window, cx).into_any_element(),
            Layer::ServerMenu => self.render_server_menu(view, window, cx).into_any_element(),
            Layer::ConfirmRemove(server_id) => self
                .render_confirm_remove(*server_id, view, window, cx)
                .into_any_element(),
            Layer::ConfirmForget(server_id) => self
                .render_confirm_forget(*server_id, view, window, cx)
                .into_any_element(),
        }
    }
}

/// A measure in design pixels that differs between systems: the height of the strip at the
/// top of the server book where macOS draws the window controls. The other systems draw
/// their title bar outside the window content.
pub(crate) fn window_controls_height(cx: &App) -> f32 {
    if cfg!(target_os = "macos") {
        theme(cx).metrics.window_controls_height
    } else {
        10.0
    }
}
