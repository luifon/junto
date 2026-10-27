//! The call window shares the application's call connection and controls.
use super::Shell;
use crate::{
    state::AppState,
    text,
    theme::{theme, u},
    ui::{Button, ButtonKind},
};
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Styled, Window, WindowBounds, WindowKind, WindowOptions, div, point,
    prelude::FluentBuilder, px, size,
};
use gpui_base::Root;
use junto_client_core::call_window::CallWindowSettings;
use junto_client_core::settings::Panel;

/// On Linux the window manager takes the request to keep a window above the windows of
/// other programs at any time, and the window kind of gpui does not make that request
/// (ADR 0507): the call window keeps its kind and is not opened again for the choice. On
/// the other systems the kind of the window decides, and the window is opened again with
/// the other kind (ADR 0772).
const ASKS_WINDOW_MANAGER: bool = cfg!(target_os = "linux");

/// The height of the first line of the header of the call window, in pixels of the
/// design: the window controls and the buttons of the panel.
const HEADER_HEIGHT: f32 = 48.0;

/// The height of the second line of the header, which holds the recording control of a
/// call of a voice channel in a narrow window, in pixels of the design.
const RECORDING_LINE_HEIGHT: f32 = 40.0;

/// The narrowest call window, in pixels of the design, whose first line of the header
/// holds the recording control between the window controls and the buttons of the panel
/// (ADR 0772). The controls and the buttons take 284 of them; the rest holds the mark of
/// a running recording with its words and the action after it.
const RECORDING_BESIDE_CONTROLS: f32 = 600.0;

/// The narrowest stage that the call window draws beside the panel of the call, in pixels
/// of the design. A narrower window draws the panel over the stage.
const STAGE_BESIDE_PANEL: f32 = 480.0;

pub fn open(state: Entity<AppState>, cx: &mut App) {
    if state.read(cx).is_call_detached() {
        return;
    }
    let saved = state.read(cx).call_window_settings();
    let displays = cx.displays();
    let origin = saved
        .x
        .zip(saved.y)
        .map(|(x, y)| point(px(x as f32), px(y as f32)));
    let display = displays
        .iter()
        .find(|display| saved.display.is_some() && display.uuid().ok() == saved.display)
        .or_else(|| displays.first());
    let mut bounds = Bounds::centered(
        display.map(|display| display.id()),
        size(px(saved.width as f32), px(saved.height as f32)),
        cx,
    );
    if let Some(display) = display {
        let screen = display.bounds();
        bounds.size.width = bounds.size.width.min(screen.size.width);
        bounds.size.height = bounds.size.height.min(screen.size.height);
        if let Some(origin) =
            origin.filter(|_| display.uuid().ok() == saved.display && saved.display.is_some())
        {
            bounds.origin = origin;
        }
        bounds.origin.x = bounds.origin.x.clamp(
            screen.origin.x,
            screen.origin.x + screen.size.width - bounds.size.width,
        );
        bounds.origin.y = bounds.origin.y.clamp(
            screen.origin.y,
            screen.origin.y + screen.size.height - bounds.size.height,
        );
    }
    let held = state.clone();
    let title = state.read(cx).call_view().call_title();
    let result = cx.open_window(
        WindowOptions {
            display_id: display.map(|display| display.id()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(480.0), px(320.0))),
            kind: if saved.keep_on_top && !ASKS_WINDOW_MANAGER {
                WindowKind::Floating
            } else {
                WindowKind::Normal
            },
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            // The identifier and the icon of the application, as for the main window: the
            // desktop shows this window with the icon of Junto (ADR 0756).
            app_id: Some(crate::app::APP_ID.to_owned()),
            icon: crate::window_icon::icon(),
            ..Default::default()
        },
        move |window, cx| {
            let closing = held.clone();
            window.on_window_should_close(cx, move |_, cx| {
                closing.update(cx, |state, cx| state.closed_call_window(cx));
                true
            });
            if ASKS_WINDOW_MANAGER && saved.keep_on_top {
                crate::window_level::keep_above(window, true, cx);
            }
            let shell = cx.new(|cx| {
                let mut shell = Shell::new_window(held, true, window, cx);
                shell._subscriptions.push(cx.observe_window_bounds(
                    window,
                    move |this, window, cx| {
                        if let WindowBounds::Windowed(bounds) = window.window_bounds() {
                            let settings = CallWindowSettings {
                                display: window.display(cx).and_then(|display| display.uuid().ok()),
                                x: Some(f32::from(bounds.origin.x) as i32),
                                y: Some(f32::from(bounds.origin.y) as i32),
                                width: f32::from(bounds.size.width) as u32,
                                height: f32::from(bounds.size.height) as u32,
                                // The choice can change while this window is open.
                                keep_on_top: this.state.read(cx).call_window_settings().keep_on_top,
                            };
                            this.state
                                .update(cx, |state, cx| state.save_call_window(settings, cx));
                        }
                    },
                ));
                shell
            });
            cx.new(|cx| Root::new(shell, window, cx))
        },
    );
    match result {
        Ok(handle) => state.update(cx, |state, cx| state.opened_call_window(handle, cx)),
        Err(error) => {
            tracing::warn!(%error, "the call window did not open");
            // The text that a call window gave to this one goes to the main window.
            super::composer::HandedDrafts::to_main(cx);
            state.update(cx, |_, cx| cx.notify());
        }
    }
}
impl Shell {
    pub(crate) fn detach_call(&self, cx: &mut Context<Self>) {
        open(self.state.clone(), cx);
    }
    pub(crate) fn return_call_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("return-call")
            .icon(crate::assets::Icon::Shrink)
            .tooltip(text::RETURN_CALL)
            .selector("return-call")
            .on_click(cx.listener(|this, _, window, cx| {
                if this.detached {
                    this.hand_over_drafts(super::composer::HandTo::Main, cx);
                    this.state
                        .update(cx, |state, cx| state.call_window_returned(cx));
                    window.remove_window();
                } else {
                    this.state.update(cx, |state, cx| state.return_call(cx));
                }
            }))
    }
    pub(crate) fn render_detached(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        window.set_rem_size(crate::theme::rem_size(
            self.state.read(cx).settings().scale_percent,
        ));
        let Some(view) = self.view.clone() else {
            return div().into_any_element();
        };
        #[cfg(feature = "test-support")]
        {
            self.drawn = Some(view.clone());
        }
        window.set_window_title(&view.call_title());
        self.refresh_picture_owner(cx);
        self.sync_camera_visibility(cx);
        // The frame sets this again when it draws the list of messages.
        self.feed.drawn = false;
        let theme = theme(cx);
        let panel = view.lists().and_then(|lists| lists.call.as_ref());
        // The panel of the call lies beside the stage when both fit, and over it otherwise.
        self.window_width = super::logical_width(window);
        let beside = self.window_width >= self.call_panel_width(cx) + STAGE_BESIDE_PANEL;
        let mut body = div()
            .relative()
            .track_focus(&self.focus)
            .key_context("Shell")
            .flex()
            .flex_col()
            .size_full()
            .bg(Hsla::from(theme.colors.basalt));
        let state = self.state.clone();
        let top = self.state.read(cx).call_window_settings().keep_on_top;
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(6.0))
            // The choice is offered only where the system can keep a window above the
            // others (ADR 0507).
            .children(crate::window_level::offered(cx).then(|| {
                Button::new("call-on-top")
                    .icon(crate::assets::Icon::Pin)
                    .tooltip(text::KEEP_ON_TOP)
                    .kind(if top {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Default
                    })
                    .selector("call-on-top")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let state = state.clone();
                        if ASKS_WINDOW_MANAGER {
                            state.update(cx, |state, cx| state.set_call_on_top(!top, cx));
                            crate::window_level::keep_above(window, !top, cx);
                            return;
                        }
                        // The window is opened again with the other kind. The text that
                        // its composer holds goes to the new window.
                        this.hand_over_drafts(super::composer::HandTo::CallWindow, cx);
                        cx.defer(move |cx| {
                            state.update(cx, |state, cx| {
                                state.set_call_on_top(!top, cx);
                                state.return_call(cx);
                            });
                            open(state, cx);
                        });
                    }))
            }))
            .child(self.return_call_button(cx))
            .child(
                Button::new("call-fullscreen")
                    .selector("call-fullscreen")
                    .icon(crate::assets::Icon::FullScreen)
                    .tooltip(text::FULLSCREEN)
                    .on_click(cx.listener(|_, _, window, _| window.toggle_fullscreen())),
            );
        // The call of a voice channel has a recording control. It lies between the window
        // controls and the buttons of the panel in a wide window, and on a second line of
        // its own in a narrow one. The form depends on the width and on the kind of the
        // call only, so a change of the recording moves nothing (ADR 0772).
        let records = view
            .call
            .as_ref()
            .is_some_and(|strip| strip.channel_id.is_some());
        let recording_beside = super::logical_width(window) >= RECORDING_BESIDE_CONTROLS;
        let recording = records.then(|| {
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .items_center()
                .gap(u(6.0))
                .children(
                    panel
                        .and_then(|panel| panel.channel())
                        .map(|channel| {
                            self.render_recording_control(&channel.recording, &theme, true, cx)
                        })
                        .into_iter()
                        .flatten(),
                )
        });
        // The cards of the invitations start under the header, and under the line of the
        // recording control when it has a line of its own (ADR 0739).
        let cards_top = HEADER_HEIGHT
            + if records && !recording_beside {
                RECORDING_LINE_HEIGHT
            } else {
                0.0
            }
            + 8.0;
        let (in_header, under_header) = if recording_beside {
            (recording, None)
        } else {
            (None, recording)
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(u(6.0))
            .h(u(HEADER_HEIGHT))
            .px(u(8.0))
            .child(controls)
            .child(in_header.unwrap_or_else(|| div().flex_1()))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(u(6.0))
                    .when(panel.is_some(), |buttons| {
                        buttons.children(self.render_call_panel_buttons(cx))
                    })
                    .child(self.render_call_notices_button(&view, cx)),
            );
        body = body.child(header);
        if let Some(recording) = under_header {
            body = body.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(u(RECORDING_LINE_HEIGHT))
                    .px(u(8.0))
                    .child(recording),
            );
        }
        // What the main window tells about the call is told here too: the person can use
        // this window alone (ADR 0772).
        let meeting = panel
            .and_then(|panel| panel.room())
            .and_then(|room| room.meeting.as_ref())
            .map(|meeting| self.render_meeting_bar(meeting, &theme, cx));
        let notices = self.render_call_notices(&view, cx);
        let stage = match panel {
            Some(panel) => self.render_call_stage(panel, &theme, cx),
            None => div().flex_1().into_any_element(),
        };
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(stage)
            .child(
                crate::ui::notice_slot("call-notice-area")
                    .most(128.0)
                    .centred()
                    .children(meeting)
                    .children(notices)
                    .children(
                        view.controls
                            .warning
                            .as_ref()
                            .map(|warning| self.render_sound_warning(warning, &theme, cx)),
                    )
                    .children(self.render_camera_notice(cx)),
            );
        self.call_panel_overlay = !beside;
        let shown = self.call_panel_shown(!beside, cx);
        let side = match panel {
            Some(panel) => shown.then(|| self.render_call_panel(&view, panel, true, window, cx)),
            // Before the state holds the conversation of a direct-message call there is no
            // panel of the call, but the bell opens the notices of its server (ADR 0772).
            None => shown
                .then(|| self.render_call_notices_alone(&view, cx))
                .flatten(),
        };
        body = body
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(column)
                    .when_some(side, |middle, side| {
                        // The panel holds its close button beside the tabs (`PANEL_CLOSE_WIDTH`).
                        let width = self.call_panel_width(cx) + super::call::PANEL_CLOSE_WIDTH;
                        let edge = self.panel_edge(
                            Panel::CallWindow,
                            super::edges::Side::Left,
                            width,
                            theme.metrics.call_panel_width,
                            window,
                            cx,
                        );
                        if beside {
                            middle.child(side).child(edge)
                        } else {
                            middle
                                .child(Self::side_cover(&theme, cx, |this, _| {
                                    this.call_panel_open = false;
                                }))
                                .child(Self::side_overlay(&theme, side))
                                .child(edge)
                        }
                    }),
            )
            .children(self.edge_drag_tracking(cx));
        let controls = div()
            .flex()
            .flex_wrap()
            .gap(u(8.0))
            .p(u(8.0))
            .child(
                match view.controls.microphone {
                    crate::state::view::MicrophoneButton::Open => {
                        Button::new("detached-microphone")
                            .kind(ButtonKind::CallOn)
                            .icon(crate::assets::Icon::Microphone)
                            .tooltip(text::CLOSE_MICROPHONE)
                    }
                    crate::state::view::MicrophoneButton::Muted => {
                        Button::new("detached-microphone")
                            .kind(ButtonKind::Call)
                            .icon(crate::assets::Icon::MicrophoneOff)
                            .tooltip(text::OPEN_MICROPHONE)
                    }
                    crate::state::view::MicrophoneButton::Held => {
                        Button::new("detached-microphone")
                            .kind(ButtonKind::SquareHeld)
                            .icon(crate::assets::Icon::MicrophoneHeld)
                            .tooltip(text::MICROPHONE_HELD)
                    }
                }
                .selector("detached-microphone")
                .on_click({
                    // The control does what it shows now, also when the call
                    // is another one at the click (ADR 0788).
                    let action = view.controls.microphone_action.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, _| state.use_microphone(&action));
                    })
                }),
            )
            .child(
                Button::new("detached-sound")
                    .kind(if view.controls.sound_off {
                        ButtonKind::Call
                    } else {
                        ButtonKind::CallOn
                    })
                    .tooltip(if view.controls.sound_off {
                        text::SOUND_OUTPUT_OFF
                    } else {
                        text::SOUND_OUTPUT_ON
                    })
                    .icon(if view.controls.sound_off {
                        crate::assets::Icon::PhonesOff
                    } else {
                        crate::assets::Icon::Phones
                    })
                    .selector("detached-sound")
                    .on_click({
                        let off = !view.controls.sound_off;
                        cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, _| state.set_sound_off(off));
                        })
                    }),
            );
        let controls = match panel.and_then(|panel| panel.room()) {
            Some(room) => controls.children(self.render_room_controls(room, &theme, cx)),
            None => controls,
        };
        body = body.child(controls);
        if let Some(strip) = &view.call {
            body = body.child(self.render_call_strip(strip, &theme, cx));
        }
        let invitations = self.render_invitation_cards(&view, cards_top, 0.0, window, cx);
        let body = body
            .when(!matches!(self.layer, super::Layer::None), |body| {
                body.child(self.render_layer(&view, window, cx))
            })
            .children(invitations)
            .map(|body| Self::presses_and_escape(body, cx))
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, _, cx| {
                    this.modifiers_changed(event.modifiers, cx)
                },
            ));
        // The system draws the title bar of this window, except on a Wayland compositor
        // without server decorations. A layer covers the call and not that strip.
        if !super::title_bar::draws_frame(window, false, cx) {
            return body.into_any_element();
        }
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_title_strip(view.call_title().into(), window, cx))
            .child(body.flex_1().min_h_0())
            .children(super::title_bar::resize_bands(window, cx))
            .into_any_element()
    }
}
