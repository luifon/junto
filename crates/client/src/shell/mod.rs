//! The main window (ADR 0703, ADR 0731): the server book, the channel column, the main
//! area with its side panel, and the layers above them. The window draws what
//! [`AppState::view`] gives and holds only what belongs to the window itself: which layer
//! is open, the text of its fields, and where the keyboard focus returns to.

pub mod account;
pub mod admin;
mod book;
pub mod call;
mod camera;
mod channels;
mod composer;
mod contacts;
pub mod detached;
mod direct;
mod feed;
mod invited;
mod join;
pub mod linking;
mod main_area;
mod notices;
mod people;
mod person_drag;
mod pictures;
mod prepared;
mod rail;
mod recorded;
pub mod room;
mod search;
mod server_menu;
mod settings;
pub mod share;
mod threads;
pub mod title_bar;
mod transcript_edit;

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
pub use search::FilterKind;
pub use transcript_edit::Incoming;

use crate::state::view::ShellView;
use crate::state::{AppState, LevelsChanged, OpenAdmin};
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
    /// The dialog in which the person chooses what to share. It holds the lists of the
    /// system, which are larger than every other layer.
    SharePicker(Box<share::SharePicker>),
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
    /// The menu of a channel, at its row.
    ChannelMenu(junto_client_core::protocol::ids::ChannelId),
    /// The menu of a direct conversation, at its row.
    DirectMenu(junto_client_core::protocol::ids::ConversationId),
    /// The dialog of the notification settings of a scope.
    Notify,
    /// The list of the choices of a filter of the search, under its box.
    SearchFilter(FilterKind),
    /// The list of the files of a recording, at its card.
    RecordingFiles(junto_client_core::protocol::ids::RecordingId),
    /// The parts of a recording with their time ranges, at its card (ADR 0616).
    RecordingParts(junto_client_core::protocol::ids::RecordingId),
    /// The languages of the speech of a new recording, at the button that records
    /// (ADR 0616).
    RecordLanguage,
    /// The question before a recording is removed.
    ConfirmRemoveRecording(ServerId, junto_client_core::protocol::ids::RecordingId),
    /// The administration of a server, or the settings of a channel. The state of the
    /// application holds which page is open.
    Admin,
    /// The profile and the devices of the account on a server (ADR 0781). The state of
    /// the application holds which page is open.
    Account,
    /// The link of a device on all servers (ADR 0782).
    LinkAll,
    /// The chooser of a new direct conversation (ADR 0307). The state of the application
    /// holds the accounts that it lists.
    NewDirect,
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
    /// The list of the choices of a filter of the search.
    SearchFilter(FilterKind),
}

/// How long the menu of a collapsed server stays after the pointer has left it, so that
/// the pointer can move from the icon into the menu.
const HOVER_MENU_LINGER: Duration = Duration::from_millis(220);

pub struct Shell {
    detached: bool,
    software_video: bool,
    camera_focus: bool,
    focused_camera: std::cell::Cell<Option<AccountId>>,
    pub(crate) state: Entity<AppState>,
    focus: FocusHandle,
    /// The focus of the layer that is open.
    layer_focus: FocusHandle,
    pub(crate) layer: Layer,
    /// The message under the pointer. Only its row carries the bar of actions, so the
    /// other rows build, lay out and hit-test no hidden buttons at each frame (ADR 0914).
    pub(crate) hovered_message: Option<(Pane, junto_client_core::protocol::ids::MessageId)>,
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
    /// The invitations beyond the first ones are shown (ADR 0739).
    pub(crate) invitations_open: bool,
    pub(crate) invitations_scroll: ScrollHandle,
    /// The panel of the call lies over the content of a narrow window.
    pub(crate) call_panel_open: bool,
    pub(crate) volumes: HashMap<(ServerId, AccountId), VolumeControl>,
    /// The press of the pointer that closed a list of devices was on the box of that
    /// list: the click that follows does not open the list again.
    pub(crate) device_menu_closed_by_press: bool,
    /// The pictures of the shares of the call, as images of the window.
    pub(crate) pictures: share::ShownPictures,
    pub(crate) cameras: camera::ShownCameras,
    drawing_call: bool,
    pub(crate) picture_tasks: Vec<Task<()>>,
    /// The pictures that the follower converted for the next frame of the display.
    pub(crate) pictures_prepared: Option<prepared::Converted>,
    /// Asks the follower of the pictures for all of them: the window draws the call again.
    pub(crate) pictures_again: std::sync::Arc<tokio::sync::Notify>,
    /// The follower converts pictures on a thread of the runtime now.
    pub(crate) pictures_converting: bool,
    /// The picture of the watched share that the window took last.
    pub(crate) main_taken: Option<std::sync::Arc<junto_media::share::picture::VideoPicture>>,
    /// How often the window was drawn.
    #[cfg(feature = "test-support")]
    draws: usize,
    /// How often received pictures asked for a drawing of the window.
    #[cfg(feature = "test-support")]
    pub(crate) picture_draws: usize,
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
    pub(crate) directs_scroll: ScrollHandle,
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
    /// The tokens of the code blocks that the window drew.
    pub(crate) code_tokens: feed::CodeTokens,
    pub(crate) composer: composer::Composer,
    /// The list and the composer of the thread of the side panel.
    pub(crate) thread_feed: feed::FeedList,
    pub(crate) thread_composer: composer::Composer,
    pub(crate) pins_scroll: ScrollHandle,
    /// The name of the channel that a thread is forked into.
    pub(crate) fork_name: Entity<InputState>,
    /// The part of a name that the chooser of a direct conversation searches.
    pub(crate) contact_part: Entity<InputState>,
    pub(crate) contacts_scroll: ScrollHandle,
    // Notices, quiet times and the search.
    pub(crate) notices_scroll: ScrollHandle,
    pub(crate) search_scroll: ScrollHandle,
    pub(crate) transcript_list: recorded::TranscriptList,
    /// The forms and the shown parts of the transcript of the side panel (ADR 0616).
    pub(crate) transcript_edit: transcript_edit::TranscriptEdit,
    /// The press of the pointer that closed the list of the files of a recording was on
    /// its button: the click that follows does not open the list again.
    pub(crate) recording_files_closed_by_press: bool,
    /// The press that closed the parts of a recording, or the languages of a new
    /// recording, was on the button that opens them: the click that follows does not open
    /// them again.
    pub(crate) recording_parts_closed_by_press: bool,
    pub(crate) record_language_closed_by_press: bool,
    /// The text of the search field of the header.
    pub(crate) search_field: Entity<InputState>,
    /// The press of the pointer that closed the list of a filter was on the box of that
    /// list: the click that follows does not open the list again.
    pub(crate) filter_closed_by_press: bool,
    /// The two times of the daily quiet time, as the settings show them.
    pub(crate) quiet_from: Entity<InputState>,
    pub(crate) quiet_until: Entity<InputState>,
    /// The two typed times are no time span.
    pub(crate) quiet_refused: bool,
    /// The time zone of the daily quiet time, as the settings show and send it
    /// (ADR 0798).
    pub(crate) quiet_zone: String,
    /// The search of the list of time zones, and whether the list is open.
    pub(crate) quiet_zone_search: Entity<InputState>,
    pub(crate) quiet_zone_open: bool,
    /// The fields and the open forms of the administration.
    pub(crate) admin: admin::AdminUi,
    /// The fields and the open form of the account layer.
    pub(crate) account: account::AccountUi,
    /// The fields of the link on all servers.
    pub(crate) linking: linking::LinkingUi,
    /// What the window holds of the update notice (ADR 0780).
    pub(crate) book: book::BookUi,
    /// What the window drew last. A test reads the data of the window from it, and not
    /// from the client core, which can be ahead of the window.
    #[cfg(feature = "test-support")]
    drawn: Option<ShellView>,

    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// The call and the seat of this device in the view that the window draws. A control
    /// that opens a microphone, a camera or a share holds them from its drawing
    /// (ADR 0788).
    pub(crate) fn drawn_seat(&self) -> Option<junto_media::calls::Seat> {
        self.view
            .as_ref()
            .and_then(|view| view.controls.seat.clone())
    }

    /// The view that the window draws. Before the first drawing it is the view of now.
    pub(crate) fn drawn_view(&self, cx: &gpui::App) -> ShellView {
        match &self.view {
            Some(view) => view.clone(),
            None if self.detached => self.state.read(cx).call_view(),
            None => self.state.read(cx).view(),
        }
    }

    fn call_listener<E: ?Sized>(
        &self,
        cx: &Context<Self>,
        listener: impl Fn(&mut Self, &E, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut gpui::App) + 'static {
        let action = self.state.read(cx).call_action();
        cx.listener(move |this, event, window, cx| {
            if action
                .as_ref()
                .is_some_and(|action| action.permits(&this.state.read(cx).calls().view()))
            {
                listener(this, event, window, cx);
            }
        })
    }

    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_window(state, false, window, cx)
    }

    fn new_window(
        state: Entity<AppState>,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = Field::state(text::ADDRESS_PLACEHOLDER, window, cx);
        let name = Field::state(text::NAME_FIELD, window, cx);
        let code = Field::state("", window, cx);
        let invite_find = Field::state(text::FIND_A_MEMBER, window, cx);
        let composer = composer::Composer::new(window, cx);
        let thread_composer = composer::Composer::new(window, cx);
        let fork_name = Field::state(text::FORK_NAME, window, cx);
        let contact_part = Field::state(text::CONTACT_PART, window, cx);
        let search_field = Field::state(text::SEARCH, window, cx);
        let quiet_from = Field::state(text::QUIET_FROM, window, cx);
        let quiet_until = Field::state(text::QUIET_UNTIL, window, cx);
        let quiet_zone_search = Field::state(text::QUIET_ZONE_SEARCH, window, cx);
        let feed = feed::FeedList::new(Pane::Main);
        feed.list.set_scroll_handler(cx.listener(|this, _, _, cx| {
            this.feed_scrolled(Pane::Main, cx);
        }));
        let thread_feed = feed::FeedList::new(Pane::Thread);
        thread_feed
            .list
            .set_scroll_handler(cx.listener(|this, _, _, cx| {
                this.feed_scrolled(Pane::Thread, cx);
            }));
        let (admin, admin_subscriptions) = admin::AdminUi::new(window, cx);
        let (transcript_edit, transcript_subscription) =
            transcript_edit::TranscriptEdit::new(window, cx);
        let own_entity = cx.entity_id();
        let (account, account_subscriptions) = account::AccountUi::new(window, cx);
        let (linking, linking_subscriptions) = linking::LinkingUi::new(window, cx);
        let book = book::BookUi::new(window, cx);
        let mut subscriptions = vec![
            cx.on_release(move |this: &mut Self, cx| {
                this.state
                    .update(cx, |state, _| state.window_talking(own_entity, false));
            }),
            Self::observe_composer(Pane::Main, &composer.input, window, cx),
            Self::observe_composer(Pane::Thread, &thread_composer.input, window, cx),
            Field::observe(
                &contact_part,
                window,
                cx,
                |this: &mut Self, event, _, cx| {
                    if event == FieldEvent::Changed {
                        this.contact_part_changed(cx);
                    }
                },
            ),
            Field::observe(
                &fork_name,
                window,
                cx,
                |this: &mut Self, event, window, cx| match event {
                    FieldEvent::Changed => this.fork_name_changed(cx),
                    FieldEvent::Submitted => this.submit_fork(window, cx),
                },
            ),
            Field::observe(
                &search_field,
                window,
                cx,
                |this: &mut Self, event, window, cx| match event {
                    FieldEvent::Changed => cx.notify(),
                    FieldEvent::Submitted => this.submit_search(window, cx),
                },
            ),
            Field::observe(&quiet_from, window, cx, |this: &mut Self, _, _, cx| {
                this.quiet_refused = false;
                cx.notify();
            }),
            Field::observe(&quiet_until, window, cx, |this: &mut Self, _, _, cx| {
                this.quiet_refused = false;
                cx.notify();
            }),
            // The list of time zones follows the typed text.
            Field::observe(&quiet_zone_search, window, cx, |_: &mut Self, _, _, cx| {
                cx.notify();
            }),
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
                // The rules of a system notification read whether the person looks at
                // the window (ADR 0742).
                let active = window.is_window_active();
                if this.detached {
                    this.state
                        .update(cx, |state, cx| state.set_call_window_active(active, cx));
                } else {
                    this.state
                        .update(cx, |state, cx| state.set_window_active(active, cx));
                }
                if !window.is_window_active() {
                    this.set_talking(false, cx);
                } else {
                    this.modifiers_changed(window.modifiers(), cx);
                    if let Some(view) = this.view.clone() {
                        this.feed_changed(&view, window, cx);
                    }
                }
            }),
            cx.observe_in(&state, window, |this, state, window, cx| {
                // A share that is being chosen belongs to one call, in both windows.
                this.drop_stale_share_picker(window, cx);
                // The call window shows the place of the call, with its chat (ADR 0772).
                let view = if this.detached {
                    state.read(cx).call_view()
                } else {
                    if let Some((submitted, result)) =
                        state.update(cx, |state, _| state.take_name_result())
                    {
                        this.name_was_decided(&submitted, result, window, cx);
                    }
                    state.read(cx).view()
                };
                // A layer whose server is gone, or whose work is done, has nothing to show.
                this.drop_stale_layer(&view, window, cx);
                // The list of the members who can be invited belongs to the call that
                // the window shows.
                if view.lists().is_none_or(|lists| lists.call.is_none()) {
                    this.invite_open = false;
                }
                // A search that ended empties its field.
                let searched = this
                    .view
                    .as_ref()
                    .is_some_and(|view| view.search_text.is_some());
                if searched && view.search_text.is_none() {
                    this.type_search("", window, cx);
                }
                this.view = Some(view.clone());
                this.take_handed_drafts(window, cx);
                this.feed_changed(&view, window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&state, window, |this, _, event: &OpenAdmin, window, cx| {
                this.open_admin(event.0, window, cx);
            }),
            cx.subscribe(&state, |this, state, _: &LevelsChanged, cx| {
                if let Some(view) = &mut this.view {
                    view.show_levels(state.read(cx).shown_levels());
                    cx.notify();
                }
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
        subscriptions.extend(admin_subscriptions);
        subscriptions.push(transcript_subscription);
        subscriptions.extend(account_subscriptions);
        subscriptions.extend(linking_subscriptions);
        let quiet_zone = state.read(cx).system_zone_name().to_owned();
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
            invitations_open: false,
            invitations_scroll: ScrollHandle::new(),
            volumes: HashMap::new(),
            device_menu_closed_by_press: false,
            detached,
            software_video: window
                .gpu_specs()
                .is_some_and(|gpu| gpu.is_software_emulated),
            camera_focus: false,
            hovered_message: None,
            focused_camera: std::cell::Cell::new(None),
            pictures: share::ShownPictures::default(),
            cameras: HashMap::new(),
            drawing_call: false,
            picture_tasks: Vec::new(),
            pictures_prepared: None,
            pictures_again: std::sync::Arc::default(),
            pictures_converting: false,
            main_taken: None,
            #[cfg(feature = "test-support")]
            draws: 0,
            #[cfg(feature = "test-support")]
            picture_draws: 0,
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
            directs_scroll: ScrollHandle::new(),
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
            code_tokens: feed::CodeTokens::default(),
            composer,
            thread_feed,
            thread_composer,
            pins_scroll: ScrollHandle::new(),
            fork_name,
            contact_part,
            contacts_scroll: ScrollHandle::new(),
            notices_scroll: ScrollHandle::new(),
            search_scroll: ScrollHandle::new(),
            transcript_list: recorded::TranscriptList::new(),
            transcript_edit,
            recording_files_closed_by_press: false,
            recording_parts_closed_by_press: false,
            record_language_closed_by_press: false,
            search_field,
            filter_closed_by_press: false,
            quiet_from,
            quiet_until,
            quiet_refused: false,
            quiet_zone,
            quiet_zone_search,
            quiet_zone_open: false,
            admin,
            account,
            linking,
            book,
            #[cfg(feature = "test-support")]
            drawn: None,
            _subscriptions: subscriptions,
        };
        shell.follow_pictures(window, cx);
        if detached {
            // The text that a call window holds when it closes goes to the main window
            // (ADR 0772). The state closes the window and tells the main window.
            let held = cx.weak_entity();
            shell.state.update(cx, |state, _| {
                state.on_call_window_closing(move |cx| {
                    if let Some(shell) = held.upgrade() {
                        shell.update(cx, |shell, cx| {
                            shell.hand_over_drafts(composer::HandTo::Main, cx);
                        });
                    }
                });
            });
            shell.take_handed_drafts(window, cx);
        }
        let active = window.is_window_active();
        if detached {
            shell
                .state
                .update(cx, |state, cx| state.set_call_window_active(active, cx));
        } else {
            shell
                .state
                .update(cx, |state, cx| state.set_window_active(active, cx));
        }
        if !detached && shell.state.read(cx).preset_name().is_empty() {
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
        let id = cx.entity_id();
        self.state
            .update(cx, |state, _| state.window_talking(id, talking));
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

    /// How often the window was drawn for any cause, how often received pictures asked
    /// for a drawing, and the width of the picture of the watched share that the window
    /// holds.
    #[cfg(feature = "test-support")]
    pub fn draws(&self) -> (usize, usize, Option<u32>) {
        let width = (self.pictures.main.as_ref()).map(|(_, picture)| picture.width());
        (self.draws, self.picture_draws, width)
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

    /// The text of the address field of the server book.
    pub fn typed_address(&self, cx: &App) -> String {
        self.address.read(cx).value().to_string()
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
            Open::SearchFilter(kind) => {
                self.open_layer(Layer::SearchFilter(kind), window, cx);
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
        if matches!(self.layer, Layer::Admin) {
            // A code or a token that the layer showed is gone with it.
            self.reset_admin_ui(window, cx);
            self.state.update(cx, |state, cx| state.close_admin(cx));
        }
        if matches!(self.layer, Layer::Account) {
            // The link code that the layer held is gone with it.
            self.reset_account_ui(window, cx);
            self.state.update(cx, |state, cx| state.close_account(cx));
        }
        if matches!(self.layer, Layer::NewDirect) {
            self.state.update(cx, |state, cx| state.close_contacts(cx));
        }
        if matches!(self.layer, Layer::LinkAll) {
            // The request with its link codes is gone with the layer.
            self.reset_linking_ui(window, cx);
            self.state.update(cx, |state, cx| state.close_link_all(cx));
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

/// The height of the window in pixels of the design.
fn logical_height(window: &Window) -> f32 {
    let rem = window.rem_size();
    window.viewport_size().height / rem * theme::REM_AT_100
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "test-support")]
        {
            self.draws += 1;
        }
        self.message_pictures.drawing_starts();
        if self.detached {
            return self.render_detached(window, cx);
        }
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
        self.refresh_picture_owner(cx);
        self.sync_camera_visibility(cx);
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
        let frame = title_bar::draws_frame(window, true, cx);
        // A layer covers the columns and not the strip of the window buttons, which stay
        // usable while a dialog is open.
        let body = div()
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(self.render_rail(&view, collapsed, narrow, window, cx))
            .child(self.render_channel_column(&view, window, cx))
            .child(self.render_main(&view, narrow, window, cx))
            .when(!matches!(self.layer, Layer::None), |body| {
                body.child(self.render_layer(&view, window, cx))
            });
        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(Hsla::from(theme.colors.travertine))
            .typeface(theme.types.body, &theme)
            .font_features(tabular_numerals())
            .ink(theme.colors.ink)
            .map(|shell| Self::presses_and_escape(shell, cx))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.modifiers_changed(event.modifiers, cx);
            }))
            .when(frame, |shell| {
                shell.child(self.render_title_strip(text::APP_NAME.into(), window, cx))
            })
            .child(body)
            .when(frame, |shell| {
                shell.children(title_bar::resize_bands(window, cx))
            })
            // An invitation into a call is drawn over everything else, also over a dialog,
            // so that the person sees it whatever the window shows (ADR 0739).
            .children(self.render_invitation_cards(
                &view,
                theme.metrics.header_height + 8.0,
                self.rail_width + theme.metrics.channel_column_width,
                window,
                cx,
            ))
            .into_any_element()
    }
}

impl Shell {
    /// What both windows do after every click and at Escape.
    pub(crate) fn presses_and_escape<E: InteractiveElement>(
        element: E,
        cx: &mut Context<Self>,
    ) -> E {
        element
            // After every click: a press that closed the menu of the server is over. A press
            // ends without a release in the window when the system took it, for example a
            // press on the title bar of Windows that moves the window; the next press ends it
            // too, before a menu closes at that press.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.presses_over()))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.presses_over()),
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
    }

    fn presses_over(&mut self) {
        self.server_menu_closed_by_press = false;
        self.device_menu_closed_by_press = false;
        self.recording_details_closed_by_press = false;
        self.invite_closed_by_press = false;
        self.filter_closed_by_press = false;
        self.recording_files_closed_by_press = false;
        self.recording_parts_closed_by_press = false;
        self.record_language_closed_by_press = false;
    }

    fn drop_stale_layer(&mut self, view: &ShellView, window: &mut Window, cx: &mut Context<Self>) {
        let gone = |server_id: &ServerId| !view.servers.iter().any(|row| row.id == *server_id);
        let stale = match &self.layer {
            Layer::ConfirmRemove(server_id) | Layer::ConfirmForget(server_id) => gone(server_id),
            Layer::ServerMenu => view.selected.is_none(),
            Layer::Join(join) => join.is_stale(view),
            // The list of devices belongs to the sound settings of the call.
            Layer::DeviceMenu(_) => {
                self.call_tab != CallTab::Settings
                    || view.lists().is_none_or(|lists| lists.call.is_none())
            }
            // The observer of the state closes the dialog of a share by its call and seat.
            Layer::SharePicker(_) => false,
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
                let thread = view
                    .workspace()
                    .and_then(|workspace| workspace.thread.as_ref());
                !held(view.lists().and_then(|lists| lists.feed.as_ref()))
                    && !held(thread.and_then(|thread| thread.feed.as_ref()))
                    && thread.is_none_or(|thread| thread.root_id != *message_id)
            }
            // The main area shows no channel and no conversation.
            Layer::Emoji(EmojiTarget::Compose(Pane::Main)) => view.shown().place().is_none(),
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
            // The channel or the conversation of the menu is no longer listed.
            Layer::ChannelMenu(channel_id) => view.workspace().is_none_or(|workspace| {
                !workspace
                    .sections
                    .iter()
                    .flat_map(|section| &section.rows)
                    .any(|row| row.id == *channel_id)
            }),
            Layer::DirectMenu(conversation_id) => view
                .lists()
                .is_none_or(|lists| !lists.directs.iter().any(|row| row.id == *conversation_id)),
            // The scope of the dialog is gone.
            Layer::Notify => view.notify.is_none(),
            Layer::SearchFilter(_) => view
                .workspace()
                .is_none_or(|workspace| workspace.search.is_none()),
            // The card of the recording is no longer among the shown rows.
            Layer::ConfirmRemoveRecording(server_id, recording_id) => {
                !view.may_remove_recording(*server_id, *recording_id)
            }
            Layer::RecordingFiles(recording_id) | Layer::RecordingParts(recording_id) => !view
                .lists()
                .and_then(|lists| lists.feed.as_ref())
                .is_some_and(|feed| {
                    feed.rows.iter().any(|row| {
                        matches!(row, crate::state::feed::FeedRow::Recording(row)
                                if row.card.recording_id == *recording_id)
                    })
                }),
            // The server shows no content any more, or the channel of the open
            // settings is gone.
            Layer::Admin => self.state.read(cx).admin_view().is_none(),
            // The server left the server book, or it shows no account or another one.
            Layer::Account => self.state.read(cx).account_view().is_none(),
            // The languages belong to the button that records, which a call of this
            // device without a recording shows to a person who may record.
            Layer::RecordLanguage => !self.records_now(view),
            Layer::NewDirect => self.state.read(cx).contacts_view().is_none(),
            Layer::None | Layer::Welcome | Layer::Settings | Layer::LinkAll => false,
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
            Layer::None
            | Layer::DeviceMenu(_)
            | Layer::PersonMenu(_)
            | Layer::MessageMenu(_)
            | Layer::ChannelMenu(_)
            | Layer::DirectMenu(_)
            | Layer::SearchFilter(_)
            | Layer::RecordingFiles(_)
            | Layer::RecordingParts(_)
            | Layer::RecordLanguage => div().into_any_element(),
            Layer::ConfirmRemoveRecording(server_id, recording_id) => {
                self.render_confirm_remove_recording(*server_id, *recording_id, cx)
            }
            Layer::Notify => self.render_notify_dialog(view, cx),
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
            Layer::Admin => self.render_admin(window, cx),
            Layer::Account => self.render_account(window, cx),
            Layer::LinkAll => self.render_link_all(window, cx),
            Layer::NewDirect => self.render_new_direct(cx),
        }
    }
}

/// A measure in design pixels that differs between systems: the height of the strip at the
/// top of the server book where macOS draws the window controls. On the other systems the
/// title bar is above the content: the system's, or the strip of `title_bar`.
pub(crate) fn window_controls_height(cx: &App) -> f32 {
    if cfg!(target_os = "macos") {
        theme(cx).metrics.window_controls_height
    } else {
        10.0
    }
}
