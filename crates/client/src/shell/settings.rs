//! The first start, where the person chooses a name, and the settings of the application:
//! a list of sections and one section at a time (ADR 0703, ADR 0717).

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, Window,
    div,
};
use gpui_base::slider::{SliderEvent, SliderState};

use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::settings::ThemeChoice;
use junto_client_core::voice::{
    InputMode, SHARE_FRAME_RATES, ShareResolution, THRESHOLD_DEFAULT, THRESHOLD_RANGE, TalkKey,
};

use super::account::Surface;
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::ProcessingPart;
use crate::state::view::CallRoom;
use crate::text;
use crate::theme::{self, Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, ChannelRow, Dialog, Field, LevelSlider, Notice, Segmented, StyleExt,
    Toggle, dialog_footer,
};

/// The themes that the setting offers, in its order.
const THEMES: [ThemeChoice; 3] = [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System];

/// The sections of the settings, in the order of their list. One is shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    Profile,
    Picture,
    Interface,
    VoiceAndVideo,
    /// The settings of the room of the call of this device (ADR 0739).
    Call,
    /// The key bindings (ADR 0777).
    Keybinds,
    Quiet,
    Notifications,
    Sounds,
    Devices,
    Permissions,
    About,
}

impl SettingsSection {
    pub const ALL: [Self; 12] = [
        Self::Profile,
        Self::Picture,
        Self::Interface,
        Self::VoiceAndVideo,
        Self::Call,
        Self::Keybinds,
        Self::Quiet,
        Self::Notifications,
        Self::Sounds,
        Self::Devices,
        Self::Permissions,
        Self::About,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Profile => text::PROFILE,
            Self::Picture => text::PICTURE,
            Self::Interface => text::INTERFACE,
            Self::VoiceAndVideo => text::VOICE_AND_VIDEO,
            Self::Call => text::CALL_SECTION,
            Self::Keybinds => text::KEYBINDS,
            Self::Quiet => text::QUIET,
            Self::Notifications => text::NOTIFICATIONS,
            Self::Sounds => text::SOUNDS,
            Self::Devices => text::account::DEVICES_SECTION,
            Self::Permissions => text::permissions::PERMISSIONS,
            Self::About => text::ABOUT,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Profile => Icon::Person,
            Self::Picture => Icon::Picture,
            Self::Interface => Icon::AppWindow,
            Self::VoiceAndVideo => Icon::Microphone,
            Self::Call => Icon::Phone,
            Self::Keybinds => Icon::Keyboard,
            Self::Quiet => Icon::Clock,
            Self::Notifications => Icon::Bell,
            Self::Sounds => Icon::Speaker,
            Self::Devices => Icon::Devices,
            Self::Permissions => Icon::Lock,
            Self::About => Icon::Document,
        }
    }

    /// The name of the row of the section in the list, for a test.
    pub fn selector(self) -> &'static str {
        match self {
            Self::Profile => "settings-section-profile",
            Self::Picture => "settings-section-picture",
            Self::Interface => "settings-section-interface",
            Self::VoiceAndVideo => "settings-section-voice-and-video",
            Self::Call => "settings-section-call",
            Self::Keybinds => "settings-section-keybinds",
            Self::Quiet => "settings-section-quiet",
            Self::Notifications => "settings-section-notifications",
            Self::Sounds => "settings-section-sounds",
            Self::Devices => "settings-section-devices",
            Self::Permissions => "settings-section-permissions",
            Self::About => "settings-section-about",
        }
    }
}

/// The name of the switch of a kind of sounds, for a test.
pub fn sound_kind_selector(kind: junto_client_core::voice::SoundKind) -> &'static str {
    use junto_client_core::voice::SoundKind;
    match kind {
        SoundKind::Messages => "sound-kind-messages",
        SoundKind::Mentions => "sound-kind-mentions",
        SoundKind::Ring => "sound-kind-ring",
        SoundKind::CallJoined => "sound-kind-call-joined",
        SoundKind::CallLeft => "sound-kind-call-left",
        SoundKind::Moved => "sound-kind-moved",
        SoundKind::ConnectionLost => "sound-kind-connection-lost",
        SoundKind::PersonJoined => "sound-kind-person-joined",
        SoundKind::PersonLeft => "sound-kind-person-left",
        SoundKind::MicrophoneOff => "sound-kind-microphone-off",
        SoundKind::MicrophoneOn => "sound-kind-microphone-on",
        SoundKind::SoundOff => "sound-kind-sound-off",
        SoundKind::SoundOn => "sound-kind-sound-on",
        SoundKind::TalkStarts => "sound-kind-talk-starts",
        SoundKind::TalkEnds => "sound-kind-talk-ends",
        SoundKind::ShareStarted => "sound-kind-share-started",
        SoundKind::ShareEnded => "sound-kind-share-ended",
        SoundKind::ViewerJoined => "sound-kind-viewer-joined",
        SoundKind::ViewerLeft => "sound-kind-viewer-left",
        SoundKind::Invitation => "sound-kind-invitation",
        SoundKind::Request => "sound-kind-request",
        SoundKind::RecordingStarted => "sound-kind-recording-started",
        SoundKind::RecordingStopped => "sound-kind-recording-stopped",
        SoundKind::HandRaised => "sound-kind-hand-raised",
    }
}

/// What a press on a switch of the sounds does.
type SwitchPress = Box<dyn Fn(&mut Shell, &mut Context<Shell>)>;

fn row_label(label: &'static str, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .w(u(84.0))
        .typeface(theme.types.control, theme)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .ink(theme.colors.ink)
        .child(label)
}

pub(super) fn group_label(label: &'static str, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .mb(u(5.0))
        .typeface(theme.types.label, theme)
        .ink(theme.colors.bronze)
        .child(label)
}

pub(super) fn help(sentence: &'static str, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .mt(u(5.0))
        .typeface(theme.types.small, theme)
        .ink(theme.colors.ink_2)
        .child(sentence)
}

pub(super) fn help_owned(sentence: String, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .mt(u(5.0))
        .typeface(theme.types.small, theme)
        .ink(theme.colors.ink_2)
        .child(sentence)
}

impl Shell {
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.state.read(cx).preset_name().to_owned();
        self.name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.name_error = None;
        self.name_saved = false;
        self.show_quiet_times(window, cx);
        self.settings_section = SettingsSection::Profile;
        self.open_layer(Layer::Settings, window, cx);
    }

    /// Stores the typed name as the name of the profile preset. The result arrives through
    /// the state of the application, in [`Shell::name_was_decided`].
    pub fn submit_name(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        self.state
            .update(cx, |state, cx| state.set_preset_name(&name, cx));
    }

    /// A name that was submitted was stored or refused.
    pub(crate) fn name_was_decided(
        &mut self,
        submitted: &str,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.name_error = None;
                if matches!(self.layer, Layer::Welcome) {
                    self.close_layer(window, cx);
                    if let Some(link) = self.pending_link.take() {
                        self.open_app_link(link, window, cx);
                    }
                } else {
                    // The field can hold other text by now: only the text that was
                    // stored is marked as saved.
                    let shown =
                        junto_client_core::profile::account_name(&self.name.read(cx).value());
                    self.name_saved = shown.is_ok_and(|shown| shown == submitted);
                }
            }
            Err(error) => {
                self.name_error = Some(error);
                self.name_saved = false;
            }
        }
        cx.notify();
    }

    pub(crate) fn render_welcome(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        Dialog::new(text::WELCOME_TITLE, self.layer_focus())
            .selector("welcome-dialog")
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::WELCOME),
            )
            .child(
                div()
                    .mt(u(12.0))
                    .child(Field::new(&self.name, text::NAME_FIELD).selector("name-field")),
            )
            .child(dialog_footer([Button::new("continue")
                .kind(ButtonKind::Primary)
                .label(text::CONTINUE)
                .selector("continue")
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(|this, _, window, cx| this.submit_name(window, cx)))
                .into_any_element()]))
            .child(
                crate::ui::notice_slot("name-error-slot").children(self.name_error.clone().map(
                    |error| {
                        div()
                            .mt(u(12.0))
                            .child(Notice::error(error).selector("name-error"))
                    },
                )),
            )
    }

    /// Shows one section of the settings. The page starts at its top.
    pub fn open_settings_section(
        &mut self,
        section: SettingsSection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section != section {
            if section != SettingsSection::Keybinds {
                self.stop_keybind_recording(cx);
            }
            self.settings_section = section;
            self.settings_scroll().set_offset(gpui::Point::default());
        }
        self.device_ui.menu = None;
        // The microphone is open for its level while the Voice section is shown (ADR 0733).
        // The permissions are read again when a section shows them (ADR 0708).
        self.state.update(cx, |state, cx| {
            state.set_device_checks(section == SettingsSection::VoiceAndVideo, cx);
            if section == SettingsSection::Interface {
                state.refresh_startup(cx);
            }
            if matches!(
                section,
                SettingsSection::Permissions | SettingsSection::Notifications
            ) {
                state.read_permissions(cx);
            }
        });
        cx.notify();
    }

    /// Opens the settings of the application at "Voice and video", from the settings button
    /// of a call. The call window has no settings: there the main window opens them and comes
    /// to the front (ADR 0772).
    pub fn open_voice_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detached {
            crate::app::in_main_window(
                |shell, window, cx| shell.open_voice_settings(window, cx),
                cx,
            );
            return;
        }
        if !matches!(self.layer, Layer::Settings) {
            self.open_settings(window, cx);
        }
        self.open_settings_section(SettingsSection::VoiceAndVideo, window, cx);
    }

    /// Opens the settings of the application at "Call", the settings of the room of the
    /// call. The call window opens them in the main window, as `open_voice_settings` does.
    pub fn open_room_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detached {
            crate::app::in_main_window(
                |shell, window, cx| shell.open_room_settings(window, cx),
                cx,
            );
            return;
        }
        if !matches!(self.layer, Layer::Settings) {
            self.open_settings(window, cx);
        }
        self.open_settings_section(SettingsSection::Call, window, cx);
    }

    /// The settings of the room of the call of this device: they apply to everyone in the
    /// call, apart from the personal settings at "Voice and video" (ADR 0739). Without a call,
    /// and in a call without a room, the section states it and shows no control.
    fn render_call_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut parts = vec![group_label(text::CALL_SECTION, theme).into_any_element()];
        let room = self.view.as_ref().and_then(|view| view.call_room.clone());
        // The statement of each case has a name of its own, for a test.
        let statement = |case: &'static str, sentence: &'static str| {
            div()
                .debug_selector(move || format!("call-section-statement:{case}"))
                .child(help(sentence, theme))
                .into_any_element()
        };
        match room {
            None => parts.push(statement("no-call", text::CALL_SECTION_NO_CALL)),
            Some(CallRoom::Direct) => parts.push(statement("no-room", text::CALL_SECTION_NO_ROOM)),
            Some(CallRoom::Unknown) => {
                parts.push(statement("not-read", text::CALL_SECTION_NOT_READ))
            }
            Some(CallRoom::Channel(room)) => {
                parts.push(
                    div()
                        .debug_selector({
                            let name = room.name.clone();
                            move || format!("call-section-channel:{name}")
                        })
                        .mt(u(4.0))
                        .typeface(theme.types.name, theme)
                        .ink(theme.colors.ink)
                        .child(text::call_section_heading(&room.name))
                        .into_any_element(),
                );
                parts.push(help(text::CALL_SECTION_FOR_EVERYONE, theme).into_any_element());
                parts.push(
                    div()
                        .debug_selector(|| "room-settings".to_owned())
                        .mt(u(8.0))
                        .flex()
                        .flex_col()
                        .children(self.render_room_settings(&room, theme, cx))
                        .into_any_element(),
                );
            }
        }
        parts
    }

    /// Whether this shell draws the call window (ADR 0772).
    pub fn is_detached(&self) -> bool {
        self.detached
    }

    pub fn settings_section(&self) -> SettingsSection {
        self.settings_section
    }

    pub(crate) fn render_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let section = self.settings_section;
        let mut nav = div().flex().flex_col().pt(u(6.0)).pb(u(10.0));
        for each in SettingsSection::ALL {
            let name = each.name();
            nav = nav.child(
                ChannelRow::new(each.selector(), each.icon(), name)
                    .selected(each == section)
                    .selector(each.selector())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_settings_section(each, window, cx);
                    })),
            );
        }
        let parts: Vec<AnyElement> = match section {
            SettingsSection::Profile => self.render_profile_section(&theme, cx),
            SettingsSection::Picture => {
                let mut parts = vec![group_label(text::PICTURE, &theme).into_any_element()];
                parts.extend(self.render_preset_picture(&theme, cx));
                parts
            }
            SettingsSection::Interface => self.render_interface_section(&theme, cx),
            SettingsSection::VoiceAndVideo => {
                let mut parts = vec![group_label(text::VOICE_AND_VIDEO, &theme).into_any_element()];
                parts.push(self.render_device_checks(&theme, cx));
                parts.extend(self.render_voice_settings(&theme, window, cx));
                parts.extend(self.render_share_and_view_settings(&theme, cx));
                parts
            }
            SettingsSection::Call => self.render_call_section(&theme, cx),
            SettingsSection::Keybinds => self.render_keybinds_section(&theme, cx),
            SettingsSection::Quiet => {
                let quiet = self
                    .view
                    .as_ref()
                    .map(|view| view.quiet.clone())
                    .unwrap_or_default();
                let mut parts = vec![group_label(text::QUIET, &theme).into_any_element()];
                parts.extend(self.render_quiet_settings(&quiet, &theme, cx));
                parts
            }
            SettingsSection::Notifications => self.render_notifications_section(&theme, cx),
            SettingsSection::Sounds => self.render_sounds_section(&theme, cx),
            SettingsSection::Devices => self.render_devices_settings(&theme, cx),
            SettingsSection::Permissions => self.render_permissions_section(&theme, cx),
            SettingsSection::About => self.render_about_section(&theme, window, cx),
        };
        let page = div()
            .flex()
            .flex_col()
            .debug_selector(move || format!("settings-page-{}", section.selector()))
            .children(parts)
            .into_any_element();
        let surface = Surface {
            id: "settings-dialog",
            title: text::SETTINGS,
            icon: Icon::Gear,
            note: SharedString::default(),
            close_selector: "dialog-close",
            nav: Some((
                nav.into_any_element(),
                text::SETTINGS_SECTIONS,
                self.book.settings_nav_scroll.clone(),
            )),
            page,
            page_scroll: self.settings_scroll().clone(),
        };
        self.render_surface(surface, window, cx)
    }

    /// The name of the profile preset.
    fn render_profile_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            group_label(text::PROFILE, theme).into_any_element(),
            div()
                .flex()
                .items_center()
                .gap(u(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Field::new(&self.name, text::NAME_FIELD).selector("name-field")),
                )
                .child(
                    Button::new("save-name")
                        .label(if self.name_saved {
                            text::SAVED
                        } else {
                            text::SAVE
                        })
                        .disabled(self.name_saved)
                        .selector("save-name")
                        .h(u(32.0))
                        .px(u(12.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.submit_name(window, cx);
                        })),
                )
                .into_any_element(),
            help(text::PROFILE_NAME_HELP, theme).into_any_element(),
            crate::ui::notice_slot("name-error-slot")
                .at_end()
                .children(
                    self.name_error
                        .clone()
                        .map(|error| Notice::error(error).selector("name-error")),
                )
                .into_any_element(),
        ]
    }

    /// The interface scale and the theme.
    fn render_interface_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (scale, chosen, settings_error) = {
            let state = self.state.read(cx);
            (
                state.settings().scale_percent,
                state.settings().theme,
                state.settings_error().map(str::to_owned),
            )
        };
        let selected = theme::SCALES.iter().position(|option| *option == scale);
        let startup = self.state.read(cx).startup().clone();
        let unavailable = match startup.status {
            Some(junto_client_core::startup::Status::Unavailable(
                junto_client_core::startup::Unavailable::SystemVersion,
            )) => Some(text::LOGIN_SYSTEM_VERSION),
            Some(junto_client_core::startup::Status::Unavailable(_)) => {
                Some(text::LOGIN_NOT_INSTALLED)
            }
            _ => None,
        };
        vec![
            group_label(text::INTERFACE, theme).into_any_element(),
            Toggle::new(
                "start-at-login",
                text::start_at_login(startup.profile.as_deref()),
                startup
                    .status
                    .is_some_and(junto_client_core::startup::Status::requested),
                (text::ON, text::OFF),
            )
            .selector("start-at-login")
            .held(if startup.busy || startup.status.is_none() {
                Some(text::LOGIN_CHECKING)
            } else {
                unavailable
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    let enabled = !state
                        .startup()
                        .status
                        .is_some_and(junto_client_core::startup::Status::requested);
                    state.set_start_at_login(enabled, cx);
                });
            }))
            .into_any_element(),
            help(unavailable.unwrap_or(text::START_AT_LOGIN_HELP), theme).into_any_element(),
            Toggle::new(
                "keep-running-after-close",
                text::KEEP_RUNNING_AFTER_CLOSE,
                self.state.read(cx).settings().keep_running_after_close,
                (text::ON, text::OFF),
            )
            .selector("keep-running-after-close")
            .on_click(cx.listener(|this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.set_keep_running_after_close(
                        !state.settings().keep_running_after_close,
                        cx,
                    );
                });
            }))
            .into_any_element(),
            help(text::KEEP_RUNNING_AFTER_CLOSE_HELP, theme).into_any_element(),
            div()
                .flex()
                .items_center()
                .gap(u(10.0))
                .child(row_label(text::SCALE, theme))
                .child(
                    Segmented::new(
                        "scale",
                        text::SCALE,
                        theme::SCALES
                            .iter()
                            .map(|option| SharedString::from(text::scale_percent(*option))),
                    )
                    .selected(selected)
                    .selector("scale")
                    .on_select({
                        let select = cx.listener(|this, index: &usize, _, cx| {
                            if let Some(scale) = theme::SCALES.get(*index).copied() {
                                this.state
                                    .update(cx, |state, cx| state.set_scale(scale, cx));
                            }
                        });
                        move |index, window, cx| select(&index, window, cx)
                    }),
                )
                .into_any_element(),
            help(text::SCALE_HELP, theme).into_any_element(),
            div()
                .mt(u(12.0))
                .flex()
                .items_center()
                .gap(u(10.0))
                .child(row_label(text::CLOCK_FORMAT, theme))
                .child(
                    Segmented::new(
                        "clock-format",
                        text::CLOCK_FORMAT,
                        text::CLOCK_FORMAT_OPTIONS.map(SharedString::from),
                    )
                    .selected(
                        junto_client_core::clock::ClockFormat::ALL
                            .iter()
                            .position(|format| {
                                *format == self.state.read(cx).settings().clock_format
                            }),
                    )
                    .selector("clock-format")
                    .on_select({
                        let select = cx.listener(|this, index: &usize, _, cx| {
                            if let Some(format) = junto_client_core::clock::ClockFormat::ALL
                                .get(*index)
                                .copied()
                            {
                                this.state
                                    .update(cx, |state, cx| state.set_clock_format(format, cx));
                            }
                        });
                        move |index, window, cx| select(&index, window, cx)
                    }),
                )
                .into_any_element(),
            div()
                .mt(u(12.0))
                .flex()
                .items_center()
                .gap(u(10.0))
                .child(row_label(text::THEME, theme))
                .child(
                    Segmented::new(
                        "theme",
                        text::THEME,
                        THEMES.map(|choice| SharedString::from(text::theme_choice(choice))),
                    )
                    .selected(THEMES.iter().position(|choice| *choice == chosen))
                    .selector("theme")
                    .on_select({
                        let select = cx.listener(|this, index: &usize, _, cx| {
                            if let Some(choice) = THEMES.get(*index).copied() {
                                this.state
                                    .update(cx, |state, cx| state.set_theme(choice, cx));
                            }
                        });
                        move |index, window, cx| select(&index, window, cx)
                    }),
                )
                .into_any_element(),
            help(text::THEME_HELP, theme).into_any_element(),
            crate::ui::notice_slot("startup-result-slot")
                .at_end()
                .children(startup.error.map(|error| {
                    Notice::error(format!("{} {error}", text::LOGIN_FAILED))
                        .selector("startup-error")
                        .action(
                            Button::new("startup-retry")
                                .label(text::TRY_AGAIN)
                                .selector("startup-retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |state, cx| state.retry_startup(cx))
                                })),
                        )
                }))
                .children(
                    (startup.status == Some(junto_client_core::startup::Status::RequiresApproval))
                        .then(|| {
                            Notice::new(text::LOGIN_APPROVAL)
                                .selector("startup-approval")
                                .action(
                                    Button::new("startup-open-approval")
                                        .label(text::LOGIN_OPEN_SETTINGS)
                                        .selector("startup-open-approval")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.open_login_approval(cx)
                                            })
                                        })),
                                )
                        }),
                )
                .into_any_element(),
            crate::ui::notice_slot("settings-error-slot")
                .at_end()
                .children(
                    settings_error.map(|error| Notice::error(error).selector("settings-error")),
                )
                .into_any_element(),
        ]
    }

    /// Whether the system shows the notifications of Junto. What a server notifies is
    /// chosen in the menu of that server.
    /// The switch of all sounds and one switch for each kind (ADR 0707). A kind is drawn
    /// as off while all sounds are off, and keeps its own choice.
    fn render_sounds_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let sounds = self.state.read(cx).voice().sounds.clone();
        let colors = theme.colors;
        let switch = |selector: &'static str,
                      title: &'static str,
                      sentence: &'static str,
                      on: bool,
                      held: Option<&'static str>,
                      press: SwitchPress,
                      cx: &mut Context<Shell>| {
            div()
                .debug_selector(move || format!("{selector}-row"))
                .flex()
                .items_start()
                .gap(u(10.0))
                .mt(u(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .typeface(theme.types.control, theme)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .ink(colors.ink)
                                .child(title),
                        )
                        .child(
                            div()
                                .typeface(theme.types.small, theme)
                                .ink(colors.ink_2)
                                .child(sentence),
                        ),
                )
                .child(
                    Toggle::new(selector, title, on, (text::ON, text::OFF))
                        .selector(selector)
                        .held(held)
                        .on_click(cx.listener(move |this, _, _, cx| press(this, cx))),
                )
                .into_any_element()
        };
        let mut parts = vec![group_label(text::SOUNDS, theme).into_any_element()];
        let all = sounds.all;
        parts.push(switch(
            "all-sounds",
            text::ALL_SOUNDS,
            text::ALL_SOUNDS_HELP,
            all,
            None,
            Box::new(move |this, cx| {
                this.state
                    .update(cx, |state, cx| state.set_all_sounds(!all, cx));
            }),
            cx,
        ));
        for kind in junto_client_core::voice::SoundKind::ALL {
            let (title, sentence) = text::sound_kind(kind);
            let on = sounds.kind_on(kind);
            parts.push(switch(
                sound_kind_selector(kind),
                title,
                sentence,
                all && on,
                (!all).then_some(text::ALL_SOUNDS_ARE_OFF),
                Box::new(move |this, cx| {
                    this.state
                        .update(cx, |state, cx| state.set_sound_kind(kind, !on, cx));
                }),
                cx,
            ));
        }
        parts
    }

    fn render_notifications_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let unavailable = self.state.read(cx).notifications_unavailable();
        let refused = self
            .state
            .read(cx)
            .permission_access(crate::permissions::Permission::Notifications)
            == crate::permissions::Access::Refused;
        let quiet = self
            .view
            .as_ref()
            .map(|view| view.quiet.clone())
            .unwrap_or_default();
        let mut parts = vec![
            group_label(text::NOTIFICATIONS, theme).into_any_element(),
            div()
                .typeface(theme.types.control, theme)
                .ink(theme.colors.ink)
                .debug_selector(|| "notifications-state".to_owned())
                .child(if unavailable.is_some() || refused {
                    text::NOTIFICATIONS_OFF_BY_SYSTEM
                } else {
                    text::NOTIFICATIONS_SHOWN
                })
                .into_any_element(),
            help(text::NOTIFICATIONS_HELP, theme).into_any_element(),
            div()
                .children(self.render_notification_permission(theme, cx))
                .into_any_element(),
            crate::ui::notice_slot("notifications-unavailable-slot")
                .at_end()
                .children(unavailable.map(|reason| {
                    div()
                        .mt(u(8.0))
                        .child(Notice::new(reason).selector("notifications-unavailable"))
                }))
                .into_any_element(),
        ];
        // Settings of the account on each server; the window sets them on every server of
        // the device, as the quiet times (ADR 0742).
        if let Some(hide_text) = quiet.hide_text {
            let shown = hide_text == crate::state::notices::Agreement::All(false);
            let busy = quiet.busy;
            parts.push(self.switch_row(
                theme,
                Setting {
                    selector: "notification-text",
                    title: text::NOTIFICATION_TEXT,
                    sentence: text::NOTIFICATION_TEXT_HELP,
                    on: shown,
                },
                Box::new(move |this: &mut Shell, cx: &mut Context<Shell>| {
                    if !busy {
                        this.state
                            .update(cx, |state, cx| state.set_hide_text(shown, cx));
                    }
                }),
                cx,
            ));
        }
        if let Some(hold) = quiet.hold_while_presenting {
            let on = hold == crate::state::notices::Agreement::All(true);
            let busy = quiet.busy;
            parts.push(self.switch_row(
                theme,
                Setting {
                    selector: "hold-while-presenting",
                    title: text::HOLD_WHILE_PRESENTING,
                    sentence: text::HOLD_WHILE_PRESENTING_HELP,
                    on,
                },
                Box::new(move |this: &mut Shell, cx: &mut Context<Shell>| {
                    if !busy {
                        this.state
                            .update(cx, |state, cx| state.set_hold_while_presenting(!on, cx));
                    }
                }),
                cx,
            ));
        }
        parts
    }

    /// A setting with its title, its sentence and its switch.
    fn switch_row(
        &self,
        theme: &Theme,
        setting: Setting,
        press: SwitchPress,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Setting {
            selector,
            title,
            sentence,
            on,
        } = setting;
        let colors = &theme.colors;
        div()
            .debug_selector(move || format!("{selector}-row"))
            .flex()
            .items_start()
            .gap(u(10.0))
            .mt(u(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.control, theme)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .ink(colors.ink)
                            .child(title),
                    )
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(sentence),
                    ),
            )
            .child(
                Toggle::new(selector, title, on, (text::ON, text::OFF))
                    .selector(selector)
                    .on_click(cx.listener(move |this, _, _, cx| press(this, cx))),
            )
            .into_any_element()
    }

    /// "Your screen share", the quality of the next share of this device, and "Your view",
    /// what the window of this device leaves out of a call (ADR 0735, ADR 0738). They are
    /// settings of the person and hold in every call.
    fn render_share_and_view_settings(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let voice = self.state.read(cx).voice().clone();
        let resolutions = [
            ShareResolution::P720,
            ShareResolution::P1080,
            ShareResolution::Source,
        ];
        let choice_row = |label: &'static str, control: Segmented| {
            div()
                .flex()
                .items_center()
                .gap(u(10.0))
                .mt(u(8.0))
                .child(row_label(label, theme))
                .child(control)
                .into_any_element()
        };
        let resolution = Segmented::new(
            "share-resolution",
            text::RESOLUTION,
            resolutions.map(|resolution| SharedString::from(text::share_resolution(resolution))),
        )
        .selected(
            resolutions
                .iter()
                .position(|resolution| *resolution == voice.share_resolution),
        )
        .selector("share-resolution")
        .on_select({
            let select = cx.listener(move |this, index: &usize, _, cx| {
                if let Some(resolution) = resolutions.get(*index).copied() {
                    this.state
                        .update(cx, |state, cx| state.set_share_resolution(resolution, cx));
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let frame_rate = Segmented::new(
            "share-frame-rate",
            text::FRAME_RATE,
            SHARE_FRAME_RATES.map(|rate| SharedString::from(rate.to_string())),
        )
        .selected(
            SHARE_FRAME_RATES
                .iter()
                .position(|rate| *rate == voice.share_frame_rate),
        )
        .selector("share-frame-rate")
        .on_select({
            let select = cx.listener(move |this, index: &usize, _, cx| {
                if let Some(rate) = SHARE_FRAME_RATES.get(*index).copied() {
                    this.state
                        .update(cx, |state, cx| state.set_share_frame_rate(rate, cx));
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let hide_without_video = voice.hide_people_without_video;
        let hide_own_camera = voice.hide_own_camera;
        vec![
            div()
                .mt(u(20.0))
                .child(group_label(text::YOUR_SCREEN_SHARE, theme))
                .into_any_element(),
            choice_row(text::RESOLUTION, resolution),
            choice_row(text::FRAME_RATE, frame_rate),
            help(text::SHARE_SETTINGS_HELP, theme).into_any_element(),
            div()
                .mt(u(20.0))
                .child(group_label(text::YOUR_VIEW, theme))
                .into_any_element(),
            self.switch_row(
                theme,
                Setting {
                    selector: "hide-without-video",
                    title: text::HIDE_WITHOUT_VIDEO,
                    sentence: text::HIDE_WITHOUT_VIDEO_HELP,
                    on: hide_without_video,
                },
                Box::new(move |this, cx| {
                    this.state.update(cx, |state, cx| {
                        state.set_hide_without_video(!hide_without_video, cx)
                    });
                }),
                cx,
            ),
            self.switch_row(
                theme,
                Setting {
                    selector: "hide-own-camera-setting",
                    title: text::HIDE_OWN_CAMERA,
                    sentence: text::HIDE_OWN_CAMERA_HELP,
                    on: hide_own_camera,
                },
                Box::new(move |this, cx| {
                    this.state.update(cx, |state, cx| {
                        state.set_hide_own_camera(!hide_own_camera, cx)
                    });
                }),
                cx,
            ),
        ]
    }

    /// The data directory, the version of this build and its updates.
    fn render_about_section(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let data_dir = self
            .state
            .read(cx)
            .client()
            .data_dir()
            .root()
            .display()
            .to_string();
        let mut parts = vec![
            group_label(text::DATA_DIRECTORY, theme).into_any_element(),
            div()
                .typeface(theme.types.mono_small, theme)
                .text_size(u(12.5))
                .line_height(u(17.0))
                .ink(theme.colors.ink)
                .child(data_dir)
                .into_any_element(),
            help(text::DATA_DIRECTORY_HELP, theme).into_any_element(),
        ];
        parts.extend(self.render_version_settings(theme, window, cx));
        if let Some(notes) = self.state.read(cx).release_notes_of_this_build() {
            parts.push(
                div()
                    .mt(u(8.0))
                    .child(
                        Button::new("release-notes-open")
                            .label(text::WHAT_IS_NEW)
                            .selector("release-notes-open")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_release_notes(&[notes], true, window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        parts
    }

    /// The settings ask before the switch to push to talk, for the pictures of the screens.
    pub fn ask_push_to_talk_fixture(&mut self, cx: &mut Context<Self>) {
        self.push_to_talk_question = true;
        cx.notify();
    }

    /// The state of the slider of the sensitivity, created at its first drawing.
    fn voice_threshold_control(
        &mut self,
        threshold: i16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<SliderState>, FocusHandle) {
        let range = (f32::from(THRESHOLD_RANGE.0), f32::from(THRESHOLD_RANGE.1));
        let control = self.voice_threshold.get_or_insert_with(|| {
            let state = cx.new(|_| LevelSlider::state(range, f32::from(threshold)));
            let changes = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
                let (value, release) = match event {
                    SliderEvent::Change(value) => (value.end(), false),
                    SliderEvent::Release(value) => (value.end(), true),
                };
                let threshold = value.round().clamp(range.0, range.1) as i16;
                this.state.update(cx, |state, cx| {
                    state.set_voice_threshold(Some(threshold), release, cx);
                });
            });
            super::call::VolumeControl {
                state,
                focus: cx.focus_handle(),
                _changes: changes,
            }
        });
        let shown = control.state.read(cx).value().end().round() as i16;
        if shown != threshold {
            control.state.update(cx, |state, cx| {
                state.set_value(f32::from(threshold), window, cx)
            });
        }
        (control.state.clone(), control.focus.clone())
    }

    /// The sensitivity of voice activation: off, or a threshold on the live level of the
    /// microphone (ADR 0701).
    fn render_voice_threshold(
        &mut self,
        threshold: Option<i16>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = theme.colors;
        let shown = {
            let state = self.state.read(cx);
            state.media_calls().call.is_some()
                || state.calls().input_test_runs()
                || state.calls().device_check_view().audio.kind()
                    == Some(junto_media::device_check::AudioCheckKind::Microphone)
        };
        let level = shown.then(|| {
            self.state.read(cx).shown_levels().input as f32
                / crate::state::view::INPUT_SEGMENTS as f32
        });
        let toggle = Toggle::new(
            "voice-threshold",
            text::SENSITIVITY,
            threshold.is_some(),
            (text::ON, text::OFF),
        )
        .selector("voice-threshold")
        .on_click(cx.listener(move |this, _, _, cx| {
            let next = match threshold {
                Some(_) => None,
                None => Some(THRESHOLD_DEFAULT),
            };
            this.state
                .update(cx, |state, cx| state.set_voice_threshold(next, true, cx));
        }));
        let mut column = div().flex().flex_col().gap(u(4.0)).mt(u(8.0)).child(
            div()
                .flex()
                .items_start()
                .gap(u(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .typeface(theme.types.control, theme)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .ink(colors.ink)
                                .child(text::SENSITIVITY),
                        )
                        .child(
                            div()
                                .typeface(theme.types.small, theme)
                                .ink(colors.ink_2)
                                .child(text::SENSITIVITY_HELP),
                        ),
                )
                .child(toggle),
        );
        if let Some(threshold) = threshold {
            let (state, focus) = self.voice_threshold_control(threshold, window, cx);
            column = column
                .child(
                    LevelSlider::new(
                        "voice-threshold-slider",
                        &state,
                        &focus,
                        text::SENSITIVITY,
                        (f32::from(THRESHOLD_RANGE.0), f32::from(THRESHOLD_RANGE.1)),
                    )
                    .level(level)
                    .selector("voice-threshold-slider"),
                )
                .child({
                    let failed = self.state.read(cx).calls().view().input_test_failed;
                    div()
                        .typeface(theme.types.small, theme)
                        .ink(if failed.is_some() && level.is_none() {
                            colors.cinnabar_ink
                        } else {
                            colors.ink_2
                        })
                        .debug_selector(|| "sensitivity-level-help".to_owned())
                        .child(match (level, failed) {
                            (Some(_), _) => text::SENSITIVITY_LEVEL_HELP.to_owned(),
                            (None, Some(error)) => text::microphone_test_failed(&error),
                            (None, None) => text::SENSITIVITY_NO_LEVEL.to_owned(),
                        })
                });
        }
        column.into_any_element()
    }

    /// The sound settings that hold for every call: when the microphone is sent, the
    /// processing of the microphone, and the tones of call events (ADR 0507, ADR 0701,
    /// ADR 0707).
    fn render_voice_settings(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let (voice, error) = {
            let state = self.state.read(cx);
            (
                state.voice().clone(),
                state.voice_error().map(str::to_owned),
            )
        };
        let colors = theme.colors;
        let row_label = |label: &'static str| {
            div()
                .flex_none()
                .w(u(84.0))
                .typeface(theme.types.control, theme)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .ink(colors.ink)
                .child(label)
        };
        let modes = [InputMode::VoiceActivation, InputMode::PushToTalk];
        let keys = [TalkKey::Alt, TalkKey::Control];
        let mut rows = vec![
            div()
                .flex()
                .items_center()
                .gap(u(10.0))
                .child(row_label(text::INPUT_MODE))
                .child(
                    Segmented::new(
                        "input-mode",
                        text::INPUT_MODE,
                        [text::VOICE_ACTIVATION.into(), text::PUSH_TO_TALK.into()],
                    )
                    .selected(modes.iter().position(|mode| *mode == voice.input_mode))
                    .selector("input-mode")
                    .on_select({
                        let select = cx.listener(move |this, index: &usize, _, cx| {
                            match modes.get(*index).copied() {
                                // Push to talk sends nothing until the key is held: the
                                // settings ask before the switch.
                                Some(InputMode::PushToTalk)
                                    if this.state.read(cx).voice().input_mode
                                        != InputMode::PushToTalk =>
                                {
                                    this.push_to_talk_question = true;
                                    cx.notify();
                                }
                                Some(mode) => {
                                    this.push_to_talk_question = false;
                                    this.state
                                        .update(cx, |state, cx| state.set_input_mode(mode, cx));
                                }
                                None => {}
                            }
                        });
                        move |index, window, cx| select(&index, window, cx)
                    }),
                )
                .into_any_element(),
        ];
        if self.push_to_talk_question && voice.input_mode != InputMode::PushToTalk {
            rows.push(
                div()
                    .id("push-to-talk-question")
                    .role(Role::Alert)
                    .aria_label(text::push_to_talk_question(voice.talk_key))
                    .debug_selector(|| "push-to-talk-question".to_owned())
                    .flex()
                    .flex_col()
                    .gap(u(8.0))
                    .mt(u(8.0))
                    .p(u(10.0))
                    .rounded(u(theme.metrics.radius_small))
                    .border_1()
                    .border_color(Hsla::from(colors.ochre))
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink)
                            .child(text::push_to_talk_question(voice.talk_key)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(u(8.0))
                            .child(
                                Button::new("confirm-push-to-talk")
                                    .kind(ButtonKind::Primary)
                                    .label(text::USE_PUSH_TO_TALK)
                                    .selector("confirm-push-to-talk")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.push_to_talk_question = false;
                                        this.state.update(cx, |state, cx| {
                                            state.set_input_mode(InputMode::PushToTalk, cx)
                                        });
                                    })),
                            )
                            .child(
                                Button::new("keep-voice-activation")
                                    .label(text::KEEP_VOICE_ACTIVATION)
                                    .selector("keep-voice-activation")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.push_to_talk_question = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }
        // The permissions of the channel of the call can require push-to-talk (ADR 0790):
        // the key is chosen then whatever mode the person chose.
        let required = self
            .state
            .read(cx)
            .media_calls()
            .call
            .is_some_and(|call| call.withheld.voice_activity);
        if required {
            rows.push(
                div()
                    .id("push-to-talk-required")
                    .role(Role::Status)
                    .debug_selector(|| "push-to-talk-required".to_owned())
                    .mt(u(6.0))
                    .child(help(
                        text::not_permitted(WorkspacePermission::UseVoiceActivity),
                        theme,
                    ))
                    .into_any_element(),
            );
        }
        if voice.input_mode == InputMode::PushToTalk || required {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(u(10.0))
                    .mt(u(6.0))
                    .child(row_label(text::TALK_KEY))
                    .child(
                        Segmented::new(
                            "talk-key",
                            text::TALK_KEY,
                            keys.map(|key| SharedString::from(text::talk_key(key))),
                        )
                        .selected(keys.iter().position(|key| *key == voice.talk_key))
                        .selector("talk-key")
                        .on_select({
                            let select = cx.listener(move |this, index: &usize, _, cx| {
                                if let Some(key) = keys.get(*index).copied() {
                                    this.state
                                        .update(cx, |state, cx| state.set_talk_key(key, cx));
                                }
                            });
                            move |index, window, cx| select(&index, window, cx)
                        }),
                    )
                    .into_any_element(),
            );
        }
        rows.push(help(text::INPUT_MODE_HELP, theme).into_any_element());
        // The key that mutes, push to mute and the other keys of a call are in the Keybinds
        // section (ADR 0777).
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(u(10.0))
                .mt(u(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(help(text::CALL_KEYS_IN_KEYBINDS, theme)),
                )
                .child(
                    Button::new("open-keybinds")
                        .label(text::KEYBINDS)
                        .selector("open-keybinds")
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_settings_section(SettingsSection::Keybinds, window, cx);
                        })),
                )
                .into_any_element(),
        );
        if voice.input_mode == InputMode::VoiceActivation {
            rows.push(self.render_voice_threshold(voice.voice_threshold, theme, window, cx));
        }
        for (part, on, title, sentence, selector) in [
            (
                None,
                voice.sound_off_mutes,
                text::SOUND_OFF_MUTES,
                text::SOUND_OFF_MUTES_HELP,
                "sound-off-mutes",
            ),
            (
                Some(ProcessingPart::EchoCancellation),
                voice.echo_cancellation,
                text::ECHO_CANCELLATION,
                text::ECHO_CANCELLATION_HELP,
                "echo-cancellation",
            ),
            (
                Some(ProcessingPart::NoiseSuppression),
                voice.noise_suppression,
                text::NOISE_SUPPRESSION,
                text::NOISE_SUPPRESSION_HELP,
                "noise-suppression",
            ),
            (
                Some(ProcessingPart::AutomaticGain),
                voice.automatic_gain,
                text::AUTOMATIC_GAIN,
                text::AUTOMATIC_GAIN_HELP,
                "automatic-gain",
            ),
        ] {
            rows.push(
                div()
                    .debug_selector(move || format!("{selector}-row"))
                    .flex()
                    .items_start()
                    .gap(u(10.0))
                    .mt(u(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .typeface(theme.types.control, theme)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .ink(colors.ink)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .typeface(theme.types.small, theme)
                                    .ink(colors.ink_2)
                                    .child(sentence),
                            ),
                    )
                    .child(
                        Toggle::new(selector, title, on, (text::ON, text::OFF))
                            .selector(selector)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| match part {
                                    Some(part) => state.set_processing(part, !on, cx),
                                    None => state.set_sound_off_mutes(!on, cx),
                                });
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            crate::ui::notice_slot("voice-settings-error-slot")
                .at_end()
                .children(error.map(|error| Notice::error(error).selector("voice-settings-error")))
                .into_any_element(),
        );
        rows
    }
}

/// The texts and the state of the switch of a setting.
struct Setting {
    selector: &'static str,
    title: &'static str,
    sentence: &'static str,
    on: bool,
}
