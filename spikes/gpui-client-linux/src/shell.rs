use std::collections::HashMap;

use gpui::{
    AnyView, AppContext as _, Context, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement,
    ParentElement as _, Render, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::{
    NextGate, PreviousGate, ScaleDown, ScaleUp,
    a11y::AccessibilityGate,
    apply_scale,
    call_window::CallGate,
    composer::ComposerGate,
    dnd::DragDropGate,
    keyboard::KeyboardGate,
    layout::LayoutGate,
    long_list::LongListGate,
    notify::NotifyGate,
    rich::RichGate,
    video::VideoGate,
    support::{EventLog, Palette, note, tabular_numerals, u},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    Composer,
    RichText,
    LongList,
    Layout,
    DragDrop,
    SecondWindow,
    Video,
    Notifications,
    Accessibility,
    Keyboard,
}

impl Gate {
    pub const ALL: [Gate; 10] = [
        Gate::Composer,
        Gate::RichText,
        Gate::LongList,
        Gate::Layout,
        Gate::DragDrop,
        Gate::SecondWindow,
        Gate::Video,
        Gate::Notifications,
        Gate::Accessibility,
        Gate::Keyboard,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|gate| gate.code().eq_ignore_ascii_case(name))
    }

    pub fn code(self) -> &'static str {
        match self {
            Gate::Composer => "C2",
            Gate::RichText => "C3",
            Gate::LongList => "C4",
            Gate::Layout => "C5",
            Gate::DragDrop => "C7",
            Gate::SecondWindow => "C8",
            Gate::Video => "C9",
            Gate::Notifications => "C10",
            Gate::Accessibility => "C11",
            Gate::Keyboard => "C12",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Gate::Composer => "Composer",
            Gate::RichText => "Rich text",
            Gate::LongList => "Long list",
            Gate::Layout => "Layout",
            Gate::DragDrop => "Drag and drop",
            Gate::SecondWindow => "Second window",
            Gate::Video => "Video tiles",
            Gate::Notifications => "Notifications",
            Gate::Accessibility => "Accessibility",
            Gate::Keyboard => "Keyboard",
        }
    }
}

pub const SCALES: [u32; 5] = [90, 100, 110, 125, 150];

pub struct Shell {
    gate: Gate,
    scale: u32,
    views: HashMap<Gate, (AnyView, Option<FocusHandle>)>,
    focus_handle: FocusHandle,
}

impl Shell {
    pub fn new(gate: Gate, scale: u32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<EventLog>(|_, cx| cx.notify()).detach();
        let mut shell = Self {
            gate,
            scale,
            views: HashMap::new(),
            focus_handle: cx.focus_handle(),
        };
        shell.ensure_view(window, cx);
        shell.focus_gate(window, cx);
        shell
    }

    fn ensure_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.views.contains_key(&self.gate) {
            return;
        }
        let view: (AnyView, Option<FocusHandle>) = match self.gate {
            Gate::Composer => (cx.new(|cx| ComposerGate::new(window, cx)).into(), None),
            Gate::RichText => (cx.new(|cx| RichGate::new(window, cx)).into(), None),
            Gate::LongList => (cx.new(|cx| LongListGate::new(window, cx)).into(), None),
            Gate::Layout => {
                let gate = cx.new(|cx| LayoutGate::new(window, cx));
                let focus = gate.focus_handle(cx);
                (gate.into(), Some(focus))
            }
            Gate::DragDrop => (cx.new(|cx| DragDropGate::new(window, cx)).into(), None),
            Gate::SecondWindow => (cx.new(|cx| CallGate::new(window, cx)).into(), None),
            Gate::Video => (cx.new(|cx| VideoGate::new(window, cx)).into(), None),
            Gate::Notifications => (cx.new(|cx| NotifyGate::new(window, cx)).into(), None),
            Gate::Accessibility => {
                (cx.new(|cx| AccessibilityGate::new(window, cx)).into(), None)
            }
            Gate::Keyboard => {
                let gate = cx.new(|cx| KeyboardGate::new(window, cx));
                let focus = gate.focus_handle(cx);
                (gate.into(), Some(focus))
            }
        };
        self.views.insert(self.gate, view);
    }

    fn focus_gate(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, Some(focus))) = self.views.get(&self.gate) {
            focus.focus(window, cx);
        }
    }

    fn select(&mut self, gate: Gate, window: &mut Window, cx: &mut Context<Self>) {
        self.gate = gate;
        self.ensure_view(window, cx);
        self.focus_gate(window, cx);
        note(cx, format!("gate selected: {} {}", gate.code(), gate.title()));
        cx.notify();
    }

    fn step_gate(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let index = Gate::ALL.iter().position(|gate| *gate == self.gate).unwrap_or(0) as isize;
        let count = Gate::ALL.len() as isize;
        let next = Gate::ALL[((index + delta + count) % count) as usize];
        self.select(next, window, cx);
    }

    fn set_scale(&mut self, scale: u32, cx: &mut Context<Self>) {
        self.scale = scale;
        apply_scale(scale, cx);
        note(cx, format!("interface scale set to {scale}%"));
        cx.notify();
    }

    fn step_scale(&mut self, delta: isize, cx: &mut Context<Self>) {
        let index = SCALES.iter().position(|scale| *scale == self.scale).unwrap_or(1) as isize;
        let next = (index + delta).clamp(0, SCALES.len() as isize - 1) as usize;
        self.set_scale(SCALES[next], cx);
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let last_lines: Vec<_> = cx
            .try_global::<EventLog>()
            .map(|log| log.lines.iter().rev().take(2).rev().cloned().collect())
            .unwrap_or_default();
        let rem = window.rem_size();

        v_flex()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NextGate, window, cx| this.step_gate(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousGate, window, cx| this.step_gate(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ScaleUp, _, cx| this.step_scale(1, cx)))
            .on_action(cx.listener(|this, _: &ScaleDown, _, cx| this.step_scale(-1, cx)))
            .size_full()
            .bg(Palette::stone())
            .text_color(Palette::ink())
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .gap(u(4.0))
                    .px(u(8.0))
                    .py(u(6.0))
                    .bg(Palette::rail())
                    .children(Gate::ALL.into_iter().map(|gate| {
                        Button::new(("gate", gate as usize))
                            .small()
                            .label(gate.code())
                            .tooltip(gate.title())
                            .when(gate == self.gate, |button| button.primary())
                            .selected(gate == self.gate)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(gate, window, cx)
                            }))
                    }))
                    .child(div().flex_1())
                    .children(SCALES.into_iter().map(|scale| {
                        Button::new(("scale", scale as usize))
                            .small()
                            .label(format!("{scale} %"))
                            .when(scale == self.scale, |button| button.primary())
                            .on_click(cx.listener(move |this, _, _, cx| this.set_scale(scale, cx)))
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .when_some(self.views.get(&self.gate).cloned(), |this, (view, _)| {
                        this.child(view)
                    }),
            )
            .child(
                v_flex()
                    .flex_none()
                    .w_full()
                    .px(u(8.0))
                    .py(u(4.0))
                    .bg(Palette::rail())
                    .text_color(Palette::rail_text())
                    .text_size(u(12.0))
                    .font_features(tabular_numerals())
                    .child(format!(
                        "{} {} · scale {} % · rem {:?} · window scale factor {} · viewport {:?}",
                        self.gate.code(),
                        self.gate.title(),
                        self.scale,
                        rem,
                        window.scale_factor(),
                        window.viewport_size()
                    ))
                    .children(last_lines.into_iter().map(|line| div().truncate().child(line))),
            )
    }
}
