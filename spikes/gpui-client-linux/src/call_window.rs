use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, TitlebarOptions, Window,
    WindowBounds, WindowKind, WindowOptions, div, point, prelude::FluentBuilder as _, px, size,
};
use gpui_component::{
    Root, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::support::{Palette, note, tabular_numerals, u};

const PEOPLE: [&str; 5] = ["Ana", "Bruno", "Carla", "Luca", "Marta"];

pub struct CallState {
    started: Instant,
    muted: bool,
    hand_raised: bool,
    speaking: usize,
    updates: u64,
    detached: Option<AnyWindowHandle>,
    detached_kind: &'static str,
    _tick: Task<()>,
}

pub struct SharedCall(pub Entity<CallState>);

impl Global for SharedCall {}

impl CallState {
    pub fn install(cx: &mut App) {
        let state = cx.new(|cx: &mut Context<CallState>| {
            let tick = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let updated = this.update(cx, |state: &mut CallState, cx| {
                        state.updates += 1;
                        if state.updates % 4 == 0 {
                            state.speaking = (state.speaking + 1) % PEOPLE.len();
                        }
                        cx.notify();
                    });
                    if updated.is_err() {
                        break;
                    }
                }
            });
            CallState {
                started: Instant::now(),
                muted: false,
                hand_raised: false,
                speaking: 0,
                updates: 0,
                detached: None,
                detached_kind: "none",
                _tick: tick,
            }
        });
        cx.set_global(SharedCall(state));
    }

    fn timer(&self) -> SharedString {
        let seconds = self.started.elapsed().as_secs();
        format!("{:02}:{:02}:{:02}", seconds / 3600, (seconds / 60) % 60, seconds % 60).into()
    }
}

fn state_summary(state: &CallState) -> impl IntoElement + use<> {
    v_flex()
        .gap(u(6.0))
        .child(
            h_flex()
                .gap(u(10.0))
                .child(div().text_size(u(20.0)).child("Daily"))
                .child(
                    div()
                        .px(u(8.0))
                        .rounded(u(4.0))
                        .bg(Palette::red())
                        .text_color(Palette::white())
                        .font_features(tabular_numerals())
                        .child(format!("REC {}", state.timer())),
                ),
        )
        .child(format!(
            "shared updates: {} · microphone: {} · hand: {}",
            state.updates,
            if state.muted { "muted" } else { "open" },
            if state.hand_raised { "raised" } else { "down" }
        ))
        .child(h_flex().gap(u(6.0)).children(PEOPLE.iter().enumerate().map(
            |(index, name)| {
                div()
                    .px(u(10.0))
                    .py(u(6.0))
                    .rounded(u(6.0))
                    .border_2()
                    .border_color(if index == state.speaking {
                        Palette::green()
                    } else {
                        Palette::border()
                    })
                    .bg(Palette::white())
                    .child(*name)
            },
        )))
}

fn controls(state: &Entity<CallState>, scope: &'static str) -> impl IntoElement + use<> {
    let for_mute = state.clone();
    let for_hand = state.clone();
    h_flex()
        .gap(u(8.0))
        .child(
            Button::new(SharedString::from(format!("{scope}-mute")))
                .small()
                .label("Toggle mute")
                .on_click(move |_, _, cx| {
                    for_mute.update(cx, |state, cx| {
                        state.muted = !state.muted;
                        cx.notify();
                    });
                    note(cx, format!("second window: mute toggled from the {scope} window"));
                }),
        )
        .child(
            Button::new(SharedString::from(format!("{scope}-hand")))
                .small()
                .label("Raise hand")
                .on_click(move |_, _, cx| {
                    for_hand.update(cx, |state, cx| {
                        state.hand_raised = !state.hand_raised;
                        cx.notify();
                    });
                    note(cx, format!("second window: hand toggled from the {scope} window"));
                }),
        )
}

fn return_call(state: &Entity<CallState>, current: &mut Window, cx: &mut App) {
    let handle = state.update(cx, |state, cx| {
        state.detached_kind = "none";
        cx.notify();
        state.detached.take()
    });
    let Some(handle) = handle else {
        return;
    };
    if handle == current.window_handle() {
        current.remove_window();
    } else {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
    note(cx, "second window: call returned to the main window");
}

pub struct CallWindow {
    state: Entity<CallState>,
}

impl CallWindow {
    fn new(state: Entity<CallState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.on_release(|this, cx| {
            this.state.update(cx, |state, cx| {
                if state.detached.take().is_some() {
                    state.detached_kind = "none";
                    cx.notify();
                }
            });
            note(cx, "second window: call window released");
        })
        .detach();
        Self { state }
    }
}

impl Render for CallWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        v_flex()
            .size_full()
            .p(u(16.0))
            .gap(u(12.0))
            .bg(Palette::rail())
            .text_color(Palette::rail_text())
            .child(div().text_color(Palette::ink()).child(
                div()
                    .p(u(12.0))
                    .rounded(u(8.0))
                    .bg(Palette::stone())
                    .child(state_summary(self.state.read(cx))),
            ))
            .child(controls(&self.state, "call"))
            .child(
                Button::new("return")
                    .small()
                    .primary()
                    .label("Return")
                    .on_click(move |_, window, cx| return_call(&state, window, cx)),
            )
    }
}

pub struct CallGate {
    state: Entity<CallState>,
}

impl CallGate {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.global::<SharedCall>().0.clone();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state }
    }

    fn detach(&mut self, kind: WindowKind, label: &'static str, cx: &mut Context<Self>) {
        if self.state.read(cx).detached.is_some() {
            return;
        }
        let state = self.state.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(780.0), px(70.0)),
                size(px(660.0), px(330.0)),
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Daily · Acme Studio".into()),
                ..Default::default()
            }),
            kind,
            ..Default::default()
        };
        let opened = cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| CallWindow::new(state, cx));
            cx.new(|cx| Root::new(view, window, cx))
        });
        match opened {
            Ok(handle) => {
                self.state.update(cx, |state, cx| {
                    state.detached = Some(handle.into());
                    state.detached_kind = label;
                    cx.notify();
                });
                note(cx, format!("second window: opened as {label}"));
            }
            Err(error) => note(cx, format!("second window: open failed: {error:#}")),
        }
    }
}

impl Render for CallGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let detached = state.detached.is_some();
        let kind = state.detached_kind;
        let for_return = self.state.clone();
        v_flex()
            .size_full()
            .p(u(16.0))
            .gap(u(12.0))
            .child(state_summary(state))
            .child(controls(&self.state, "main"))
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(
                        Button::new("detach")
                            .small()
                            .label("Detach call (normal window)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.detach(WindowKind::Normal, "normal", cx)
                            })),
                    )
                    .child(
                        Button::new("detach-floating")
                            .small()
                            .label("Detach call (floating, always on top)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.detach(WindowKind::Floating, "floating", cx)
                            })),
                    )
                    .when(detached, |this| {
                        this.child(
                            Button::new("return-main")
                                .small()
                                .primary()
                                .label("Return")
                                .on_click(move |_, window, cx| return_call(&for_return, window, cx)),
                        )
                    }),
            )
            .child(format!(
                "call window: {}",
                if detached { kind } else { "not detached" }
            ))
    }
}
