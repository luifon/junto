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
    ParentElement, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowKind,
    WindowOptions, div, point, prelude::FluentBuilder, px, size,
};
use gpui_base::Root;
use junto_client_core::call_window::CallWindowSettings;

/// On Linux the window manager takes the request to keep a window above the windows of
/// other programs at any time, and the window kind of gpui does not make that request
/// (ADR 0507): the call window keeps its kind and is not opened again for the choice. On
/// the other systems the kind of the window decides, and the window is opened again with
/// the other kind (ADR 0772).
const ASKS_WINDOW_MANAGER: bool = cfg!(target_os = "linux");

/// Asks the system to keep the call window above other windows, or to stop. Nothing of the
/// call depends on the answer.
fn ask_above(window: &mut Window, above: bool, cx: &mut App) {
    match crate::window_level::keep_above(window, above, cx) {
        crate::window_level::KeepAbove::Applied => {}
        crate::window_level::KeepAbove::Unsupported => {
            tracing::warn!("the window system keeps no window above the others");
        }
        crate::window_level::KeepAbove::Failed(failure) => {
            tracing::warn!(%failure, "the call window was not kept above the others");
        }
    }
}

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
            ..Default::default()
        },
        move |window, cx| {
            let closing = held.clone();
            window.on_window_should_close(cx, move |_, cx| {
                closing.update(cx, |state, cx| state.closed_call_window(cx));
                true
            });
            if ASKS_WINDOW_MANAGER && saved.keep_on_top {
                ask_above(window, true, cx);
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
        Err(error) => tracing::warn!(%error, "the call window did not open"),
    }
}
impl Shell {
    pub(crate) fn detach_call(&self, cx: &mut Context<Self>) {
        open(self.state.clone(), cx);
    }
    pub(crate) fn return_call_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("return-call")
            .icon(crate::assets::Icon::Collapse)
            .tooltip(text::RETURN_CALL)
            .selector("return-call")
            .on_click(cx.listener(|this, _, window, cx| {
                if this.detached {
                    this.state
                        .update(cx, |state, cx| state.closed_call_window(cx));
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
        window.set_window_title(&view.call_title());
        self.refresh_picture_owner(cx);
        self.sync_camera_visibility(cx);
        let theme = theme(cx);
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
        let header = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(u(6.0))
            .p(u(8.0))
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
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let state = state.clone();
                        if ASKS_WINDOW_MANAGER {
                            state.update(cx, |state, cx| state.set_call_on_top(!top, cx));
                            ask_above(window, !top, cx);
                            return;
                        }
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
                    .icon(crate::assets::Icon::Expand)
                    .tooltip(text::FULLSCREEN)
                    .on_click(cx.listener(|_, _, window, _| window.toggle_fullscreen())),
            );
        let header = if let Some(panel) = view
            .workspace()
            .and_then(|workspace| workspace.call.as_ref())
        {
            header.children(self.render_recording_control(&panel.recording, &theme, cx))
        } else {
            header
        };
        body = body.child(header);
        if let Some(workspace) = view.workspace()
            && let (Some(channel), Some(panel)) = (&workspace.channel, &workspace.call)
        {
            body = body.child(match &panel.stage {
                Some(shares) if share_visible(panel.phase) => {
                    self.render_share_stage(channel, panel, shares, &theme, cx)
                }
                _ => self.render_stage(channel, panel, &theme, cx),
            });
        } else if let Some(call) = self.state.read(cx).media_calls().call
            && let junto_client_core::protocol::call::CallPlace::Direct {
                low_account_id: low,
                high_account_id: high,
            } = call.target.place
        {
            let own = view.person.account_id;
            body = body.child(
                div()
                    .id("direct-call-stage")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.render_direct_call(
                        call.target.server_id,
                        if own == Some(low) { high } else { low },
                        cx,
                    )),
            );
        }
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
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |state, _| state.toggle_microphone())
                })),
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, _| state.toggle_sound())
                    })),
            );
        let controls = if let Some(panel) = view.workspace().and_then(|w| w.call.as_ref()) {
            controls.children(self.render_room_controls(&panel.room, &theme, cx))
        } else {
            controls
        };
        body = body.child(controls);
        if let Some(strip) = &view.call {
            body = body.child(self.render_call_strip(strip, &theme, cx));
        }
        body.when(!matches!(self.layer, super::Layer::None), |body| {
            body.child(self.render_layer(&view, window, cx))
        })
        .on_modifiers_changed(
            cx.listener(|this, event: &gpui::ModifiersChangedEvent, _, cx| {
                this.modifiers_changed(event.modifiers, cx)
            }),
        )
        .into_any_element()
    }
}

fn share_visible(phase: junto_media::calls::Phase) -> bool {
    phase == junto_media::calls::Phase::Connected
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_detached_reconnecting_call_does_not_show_its_retained_share() {
        use junto_media::calls::Phase;
        assert!(share_visible(Phase::Connected));
        assert!(!share_visible(Phase::Reconnecting));
        assert!(!share_visible(Phase::Connecting));
        assert!(!share_visible(Phase::Entering));
    }
}
