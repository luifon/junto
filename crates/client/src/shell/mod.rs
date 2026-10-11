//! The main window (ADR 0703, ADR 0731): the server book, the channel column, the main
//! area with its side panel, and the layers above them. The window draws what
//! [`AppState::view`] gives and holds only what belongs to the window itself: which layer
//! is open, the text of its fields, and where the keyboard focus returns to.

pub mod call;
mod channels;
mod composer;
mod feed;
mod join;
mod main_area;
mod rail;
pub mod room;
mod server_menu;
mod settings;
pub mod share;
mod threads;

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
    /// The dialog in which the person chooses what to share.
    SharePicker(share::SharePicker),
    /// The menu of a person in a voice channel, at the row of the person.
    PersonMenu(room::PersonMenuOpen),
    /// The menu of a message, at the message.
    MessageMenu(feed::MessageMenuOpen),
    /// The question before a message is deleted.
    ConfirmDelete(junto_client_core::protocol::ids::MessageId),
    /// The emoji picker: for a reaction to a message, or for the text of a composer.
    Emoji(EmojiTarget),
    /// The dialog that forks the thread of the side panel into a channel.
    Fork(threads::ForkDialog),
}

/// What the emoji of the picker is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmojiTarget {
    React(junto_client_core::protocol::ids::MessageId),
    Compose(Pane),
}

/// A list of messages with its composer: the one of the main area, or the thread of the
/// side panel (ADR 0741).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Main,
    Thread,
}

impl Pane {
    /// The pane that shows a place of messages.
    pub fn of(place: junto_client_core::model::PlaceId) -> Self {
        match place.thread_root() {
            Some(_) => Self::Thread,
            None => Self::Main,
        }
    }

    /// The identifier of a part of the pane. The parts of the thread have names of
    /// their own, so that both panes can be drawn at once.
    pub(crate) fn id(self, part: &'static str) -> &'static str {
        match (self, part) {
            (Self::Main, part) => part,
            (Self::Thread, "feed") => "thread-feed",
            (Self::Thread, "message-list") => "thread-message-list",
            (Self::Thread, "go-to-end") => "thread-go-to-end",
            (Self::Thread, "composer") => "thread-composer",
            (Self::Thread, "composer-input") => "thread-composer-input",
            (Self::Thread, "send-message") => "thread-send-message",
            (Self::Thread, "messages-state") => "thread-messages-state",
            (Self::Thread, "no-messages") => "thread-no-messages",
            (Self::Thread, part) => part,
        }
    }
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
    /// The dialog in which the person chooses what to share.
    SharePicker,
    /// What happened to the last recording of the call.
    RecordingDetails,
    /// The list of the members who can be invited into the call.
    MemberPicker,
    /// The settings of the room, at the end of the Settings pane of the call.
    RoomSettings,
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
    /// The pictures of the shares of the call, as images of the window.
    pub(crate) pictures: share::ShownPictures,
    pub(crate) picture_tasks: Vec<Task<()>>,
    /// The number of dialogs of a share that this window has opened.
    pub(crate) share_reads: u64,
    /// The volume control of the sound of the watched share.
    pub(crate) share_volume: Option<VolumeControl>,
    /// The details of the last recording of the call are shown.
    pub(crate) recording_details_open: bool,
    /// The press of the pointer that closed the details was on their button: the click
    /// that follows does not open them again.
    pub(crate) recording_details_closed_by_press: bool,
    /// The list of the members who can be invited into the call is open.
    pub(crate) invite_open: bool,
    /// The press of the pointer that closed that list was on its button: the click that
    /// follows does not open it again.
    pub(crate) invite_closed_by_press: bool,
    /// The text that the list of the members is searched for.
    pub(crate) invite_find: Entity<InputState>,
    /// What the state of the application gave last. It is derived again only after the
    /// state told a change: a picture of a share draws the window many times a second.
    view: Option<ShellView>,
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
    // The messages.
    /// The list of the messages of the shown channel.
    pub(crate) feed: feed::FeedList,
    pub(crate) message_pictures: feed::MessagePictures,
    pub(crate) composer: composer::Composer,
    /// The list and the composer of the thread of the side panel.
    pub(crate) thread_feed: feed::FeedList,
    pub(crate) thread_composer: composer::Composer,
    pub(crate) pins_scroll: ScrollHandle,
    /// The name of the channel that a thread is forked into.
    pub(crate) fork_name: Entity<InputState>,
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
        let invite_find = Field::state(text::FIND_A_MEMBER, window, cx);
        let composer = composer::Composer::new(window, cx);
        let thread_composer = composer::Composer::new(window, cx);
        let fork_name = Field::state(text::FORK_NAME, window, cx);
        let feed = feed::FeedList::new(Pane::Main);
        feed.list
            .set_scroll_handler(cx.listener(|this, event, window, cx| {
                this.feed_scrolled(Pane::Main, event, window, cx);
            }));
        let thread_feed = feed::FeedList::new(Pane::Thread);
        thread_feed
            .list
            .set_scroll_handler(cx.listener(|this, event, window, cx| {
                this.feed_scrolled(Pane::Thread, event, window, cx);
            }));
        let subscriptions = vec![
            Self::observe_composer(Pane::Main, &composer.input, window, cx),
            Self::observe_composer(Pane::Thread, &thread_composer.input, window, cx),
            Field::observe(
                &fork_name,
                window,
                cx,
                |this: &mut Self, event, window, cx| match event {
                    FieldEvent::Changed => this.fork_name_changed(cx),
                    FieldEvent::Submitted => this.submit_fork(window, cx),
                },
            ),
            // The emoji of the picker follow the text that they are searched for.
            Field::observe(
                &composer.emoji_find,
                window,
                cx,
                |_: &mut Self, _, _, cx| {
                    cx.notify();
                },
            ),
            // A key that is held when the window loses the focus is not seen when it is
            // released: push-to-talk ends with the focus (ADR 0507).
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.set_talking(false, cx);
                } else if let Some(view) = this.view.clone() {
                    // The person looks at the window again: what it shows is seen.
                    this.feed_changed(&view, window, cx);
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
                // The list of the members who can be invited belongs to the call that
                // the window shows.
                if view
                    .workspace()
                    .is_none_or(|workspace| workspace.call.is_none())
                {
                    this.invite_open = false;
                }
                this.view = Some(view.clone());
                this.feed_changed(&view, window, cx);
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
            // The list of the members follows the text that it is searched for.
            Field::observe(&invite_find, window, cx, |_: &mut Self, _, _, cx| {
                cx.notify();
            }),
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
            pictures: share::ShownPictures::default(),
            picture_tasks: Vec::new(),
            share_reads: 0,
            share_volume: None,
            recording_details_open: false,
            recording_details_closed_by_press: false,
            invite_open: false,
            invite_closed_by_press: false,
            invite_find,
            view: None,
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
            feed,
            message_pictures: feed::MessagePictures::default(),
            composer,
            thread_feed,
            thread_composer,
            pins_scroll: ScrollHandle::new(),
            fork_name,
            #[cfg(feature = "test-support")]
            drawn: None,
            _subscriptions: subscriptions,
        };
        shell.follow_pictures(cx);
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

    pub(crate) fn pen(&self, pane: Pane) -> &composer::Composer {
        match pane {
            Pane::Main => &self.composer,
            Pane::Thread => &self.thread_composer,
        }
    }

    pub(crate) fn pen_mut(&mut self, pane: Pane) -> &mut composer::Composer {
        match pane {
            Pane::Main => &mut self.composer,
            Pane::Thread => &mut self.thread_composer,
        }
    }

    pub(crate) fn list(&self, pane: Pane) -> &feed::FeedList {
        match pane {
            Pane::Main => &self.feed,
            Pane::Thread => &self.thread_feed,
        }
    }

    pub(crate) fn list_mut(&mut self, pane: Pane) -> &mut feed::FeedList {
        match pane {
            Pane::Main => &mut self.feed,
            Pane::Thread => &mut self.thread_feed,
        }
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
            Open::SharePicker => self.open_share_picker(window, cx),
            Open::RecordingDetails => {
                self.recording_details_open = true;
                cx.notify();
            }
            Open::RoomSettings => {
                self.call_panel_open = true;
                self.show_call_tab(CallTab::Settings, cx);
                self.call_settings_scroll.scroll_to_bottom();
            }
            Open::MemberPicker => {
                self.call_panel_open = true;
                self.show_call_tab(CallTab::Members, cx);
                if !self.invite_open {
                    self.toggle_member_picker(window, cx);
                }
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
        let settings = self.state.read(cx).settings();
        let view: ShellView = match &self.view {
            Some(view) => view.clone(),
            None => {
                let view = self.state.read(cx).view();
                self.view = Some(view.clone());
                view
            }
        };
        #[cfg(feature = "test-support")]
        {
            self.drawn = Some(view.clone());
        }
        window.set_rem_size(theme::rem_size(settings.scale_percent));
        // The frame sets this again when it draws the list of messages.
        self.feed.drawn = false;
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
                    this.recording_details_closed_by_press = false;
                    this.invite_closed_by_press = false;
                }),
            )
            // Escape closes the details of a recording and the list of the members who
            // can be invited. Neither is a layer of the window.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.invite_open {
                    this.invite_open = false;
                    this.focus.focus(window, cx);
                } else if this.recording_details_open {
                    this.recording_details_open = false;
                } else {
                    return;
                }
                cx.stop_propagation();
                cx.notify();
            }))
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
            // A share belongs to the call of this device.
            Layer::SharePicker(_) => view.call.is_none(),
            // The person left the voice channel, or the channel is no longer shown.
            Layer::PersonMenu(open) => view.person_menu(open.channel_id, open.account_id).is_none(),
            // The message is no longer among the shown ones.
            Layer::MessageMenu(feed::MessageMenuOpen { message_id, .. })
            | Layer::ConfirmDelete(message_id)
            | Layer::Emoji(EmojiTarget::React(message_id)) => {
                let held = |feed: Option<&crate::state::feed::Feed>| {
                    feed.is_some_and(|feed| {
                        feed.rows.iter().any(|row| {
                            matches!(row, crate::state::feed::FeedRow::Message(row)
                                if row.message.id == *message_id)
                        })
                    })
                };
                let workspace = view.workspace();
                let thread = workspace.and_then(|workspace| workspace.thread.as_ref());
                !held(workspace.and_then(|workspace| workspace.feed.as_ref()))
                    && !held(thread.and_then(|thread| thread.feed.as_ref()))
                    && thread.is_none_or(|thread| thread.root_id != *message_id)
            }
            Layer::Emoji(EmojiTarget::Compose(Pane::Main)) => view.workspace().is_none(),
            // The thread of the side panel was closed.
            Layer::Emoji(EmojiTarget::Compose(Pane::Thread)) => view
                .workspace()
                .is_none_or(|workspace| workspace.thread.is_none()),
            // The thread was closed, or the server made the channel.
            Layer::Fork(dialog) => {
                view.workspace()
                    .is_none_or(|workspace| workspace.thread.is_none())
                    || Self::fork_is_done(dialog, view)
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
            // The list of devices is drawn at its box, in the panel of the call, and the
            // menu of a person at the row of the person.
            Layer::None | Layer::DeviceMenu(_) | Layer::PersonMenu(_) | Layer::MessageMenu(_) => {
                div().into_any_element()
            }
            Layer::ConfirmDelete(message_id) => self.render_confirm_delete(*message_id, window, cx),
            Layer::Emoji(_) => self.render_emoji_picker(window, cx),
            Layer::Fork(_) => self.render_fork_dialog(view, window, cx),
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
            Layer::SharePicker(_) => self.render_share_picker(window, cx),
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
