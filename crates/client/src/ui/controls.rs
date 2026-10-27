//! The controls of the sound settings and of the call panel: the switch, the box that
//! opens a list of choices, the tab, and the volume control of a person.

use gpui::{
    App, ClickEvent, ElementId, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder, relative,
};
use gpui_base::slider::{Slider, SliderIndicator, SliderState, SliderThumb, SliderTrack};

use super::{Band, BandKind, StyleExt, focus_ring, focus_ring_inside, icon, ledge, tooltip};
use crate::assets::Icon;
use crate::theme::{theme, u};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A switch with its state as a word beside it.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    label: SharedString,
    on: bool,
    words: (SharedString, SharedString),
    /// Why the person cannot change the switch. A switch with a reason takes no click.
    held: Option<SharedString>,
    selector: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl Toggle {
    /// A switch. `label` names it in the accessibility tree, and `words` are the words
    /// for on and for off.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        on: bool,
        words: (&'static str, &'static str),
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            on,
            words: (words.0.into(), words.1.into()),
            held: None,
            selector: None,
            on_click: None,
        }
    }

    /// The switch shows its state and cannot be changed, for the reason that is given.
    pub fn held(mut self, reason: Option<impl Into<SharedString>>) -> Self {
        self.held = reason.map(Into::into);
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
        let focus = super::reveal::handle(&self.id, window, cx);
        div()
            .id(self.id)
            .role(Role::Switch)
            .aria_label(self.label.clone())
            .aria_selected(self.on)
            .when_some(self.selector, |toggle, selector| {
                toggle.debug_selector(|| selector.to_owned())
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(u(6.0))
            .mt(u(1.0))
            .rounded(u(9.0))
            .map(|toggle| match self.held {
                // The reason is the description of the switch for a screen reader too.
                Some(reason) => toggle
                    .aria_description(reason.clone())
                    .opacity(0.55)
                    .cursor_not_allowed()
                    .tooltip(tooltip(reason)),
                None => toggle
                    .cursor_pointer()
                    .track_focus(&focus)
                    .children(super::reveal::report(&focus, window))
                    .focus_visible(move |style| style.shadow(ring))
                    .when_some(self.on_click, |toggle, handler| {
                        toggle.on_click(move |event, window, cx| handler(event, window, cx))
                    }),
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(u(34.0))
                    .h(u(18.0))
                    .rounded(u(9.0))
                    .bg(Hsla::from(if self.on {
                        colors.toggle_on
                    } else {
                        colors.toggle_off
                    }))
                    .child(
                        div()
                            .absolute()
                            .top(u(2.0))
                            .left(u(if self.on { 18.0 } else { 2.0 }))
                            .size(u(14.0))
                            .rounded_full()
                            .bg(Hsla::from(colors.toggle_knob)),
                    ),
            )
            .child(
                div()
                    .w(u(22.0))
                    .typeface(theme.types.small_strong, &theme)
                    .ink(colors.ink_2)
                    .child(super::paint_text(
                        if self.on { self.words.0 } else { self.words.1 },
                        Some(self.label),
                    )),
            )
    }
}

/// A number with a button that lowers it and a button that raises it.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    lower: (SharedString, Option<ClickHandler>),
    raise: (SharedString, Option<ClickHandler>),
    /// Why the person cannot change the number.
    held: Option<SharedString>,
    selector: Option<&'static str>,
}

impl Stepper {
    /// A stepper. `label` names it in the accessibility tree, `value` is what it shows,
    /// and `lower` and `raise` name its two buttons.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
        lower: &'static str,
        raise: &'static str,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            lower: (lower.into(), None),
            raise: (raise.into(), None),
            held: None,
            selector: None,
        }
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    /// The reason why the person cannot change the number, as the description of the two
    /// buttons and in their tooltips.
    pub fn held(mut self, reason: Option<impl Into<SharedString>>) -> Self {
        self.held = reason.map(Into::into);
        self
    }

    /// Without a handler the button is drawn disabled.
    pub fn on_lower(
        mut self,
        handler: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    ) -> Self {
        self.lower.1 = handler.map(|handler| Box::new(handler) as ClickHandler);
        self
    }

    pub fn on_raise(
        mut self,
        handler: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    ) -> Self {
        self.raise.1 = handler.map(|handler| Box::new(handler) as ClickHandler);
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        let selector = self.selector;
        let held = self.held;
        let [lower, raise] = ["lower", "raise"]
            .map(|name| super::reveal::inner_handle(&self.id, &name.into(), window, cx));
        let step = |name: &'static str,
                    focus: FocusHandle,
                    shown: Icon,
                    (label, handler): (SharedString, Option<ClickHandler>)| {
            let ring = ring.clone();
            let enabled = handler.is_some();
            div()
                .id(name)
                .role(Role::Button)
                .aria_label(label.clone())
                .when_some(selector, |button, selector| {
                    button.debug_selector(move || format!("{selector}-{name}"))
                })
                .tooltip(tooltip(match (&held, enabled) {
                    (Some(reason), false) => reason.clone(),
                    _ => label,
                }))
                .when_some(held.clone().filter(|_| !enabled), |button, reason| {
                    button.aria_description(reason)
                })
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(u(26.0))
                .child(icon(
                    shown,
                    13.0,
                    if enabled { colors.ink_2 } else { colors.ink_3 },
                ))
                .map(|button| match handler {
                    Some(handler) => button
                        .cursor_pointer()
                        .track_focus(&focus)
                        .children(super::reveal::report(&focus, window))
                        .hover(move |style| style.bg(Hsla::from(colors.travertine)))
                        .focus_visible(move |style| style.shadow(ring))
                        .on_click(move |event, window, cx| handler(event, window, cx)),
                    None => button.cursor_not_allowed(),
                })
        };
        div()
            .id(self.id)
            .role(Role::Group)
            .aria_label(self.label)
            .when_some(selector, |stepper, selector| {
                stepper.debug_selector(|| selector.to_owned())
            })
            .flex()
            .flex_none()
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .overflow_hidden()
            .child(step("lower", lower, Icon::Minus, self.lower))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(u(32.0))
                    .border_l_1()
                    .border_r_1()
                    .border_color(Hsla::from(colors.rule))
                    .typeface(theme.types.mono_field, &theme)
                    .text_size(u(13.5))
                    .ink(colors.ink)
                    .child(self.value),
            )
            .child(step("raise", raise, Icon::Plus, self.raise))
    }
}

/// The box that shows one choice of a list and opens the list.
#[derive(IntoElement)]
pub struct SelectBox {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    selector: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl SelectBox {
    /// `label` names the box in the accessibility tree, and `value` is the choice of now.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            selector: None,
            on_click: None,
        }
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SelectBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
        let focus = super::reveal::handle(&self.id, window, cx);
        div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(format!("{}: {}", self.label, self.value))
            .when_some(self.selector, |select, selector| {
                select.debug_selector(|| selector.to_owned())
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(u(8.0))
            .h(u(30.0))
            .pl(u(10.0))
            .pr(u(8.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .shadow(ledge(colors.rule))
            .typeface(theme.types.control, &theme)
            .font_weight(gpui::FontWeight::NORMAL)
            .ink(colors.ink)
            .cursor_pointer()
            .track_focus(&focus)
            .children(super::reveal::report(&focus, window))
            .hover(move |style| style.bg(Hsla::from(colors.travertine)))
            .focus_visible(move |style| style.shadow(ring))
            .when_some(self.on_click, |select, handler| {
                select.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(div().flex_1().min_w_0().truncate().child(self.value))
            .child(icon(Icon::ChevronDown, 14.0, colors.ink_2))
    }
}

/// One tab of the row of tabs above a panel.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    icon: Icon,
    label: SharedString,
    on: bool,
    selector: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            on: false,
            selector: None,
            on_click: None,
        }
    }

    /// The tab whose pane is shown.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        let focus = super::reveal::handle(&self.id, window, cx);
        #[cfg(feature = "test-support")]
        let placed = super::placed::placed(match self.selector {
            Some(selector) => selector.into(),
            None => format!("{}", self.id).into(),
        });
        let tab = div();
        #[cfg(feature = "test-support")]
        let tab = tab.child(placed);
        tab.id(self.id)
            .role(Role::Tab)
            .aria_label(self.label.clone())
            .aria_selected(self.on)
            .when_some(self.selector, |tab, selector| {
                tab.debug_selector(|| selector.to_owned())
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(7.0))
            .h_full()
            .px(u(13.0))
            .typeface(
                if self.on {
                    theme.types.control_strong
                } else {
                    theme.types.control
                },
                &theme,
            )
            .ink(if self.on { colors.ink } else { colors.ink_2 })
            .when(self.on, |tab| {
                tab.bg(Hsla::from(colors.plaster))
                    .border_l_1()
                    .border_r_1()
                    .border_color(Hsla::from(colors.rule))
                    // The open tab joins its pane: it covers the line under the row.
                    .shadow(ledge(colors.plaster))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(u(3.0))
                            .overflow_hidden()
                            .child(Band::new(BandKind::Tab)),
                    )
            })
            .cursor_pointer()
            .track_focus(&focus)
            .children(super::reveal::report(&focus, window))
            .focus_visible(move |style| style.shadow(ring))
            .when_some(self.on_click, |tab, handler| {
                tab.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(icon(
                self.icon,
                15.0,
                if self.on {
                    colors.terracotta_deep
                } else {
                    colors.ink_2
                },
            ))
            .child(self.label)
    }
}

/// The lowest and the highest volume of a person, in percent, and the step of one key.
pub const VOLUME_RANGE: (f32, f32) = (0.0, 200.0);
pub const VOLUME_STEP: f32 = 10.0;

/// The volume control of one person: a track from 0 to 200 % with a mark at 100 %, and the
/// value beside it. The arrow keys change it while it holds the keyboard focus.
#[derive(IntoElement)]
pub struct VolumeSlider {
    id: ElementId,
    state: Entity<SliderState>,
    focus: FocusHandle,
    label: SharedString,
    selector: Option<SharedString>,
}

impl VolumeSlider {
    /// `label` names the control in the accessibility tree.
    pub fn new(
        id: impl Into<ElementId>,
        state: &Entity<SliderState>,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            focus: focus.clone(),
            label: label.into(),
            selector: None,
        }
    }

    /// The state of a volume control at `percent`.
    pub fn state(percent: u16) -> SliderState {
        SliderState::new()
            .min(VOLUME_RANGE.0)
            .max(VOLUME_RANGE.1)
            .step(1.0)
            .default_value(f32::from(percent))
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}

impl RenderOnce for VolumeSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let value = self.state.read(cx).value().end();
        let share = (value - VOLUME_RANGE.0) / (VOLUME_RANGE.1 - VOLUME_RANGE.0);
        let ring = focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
        let state = self.state.clone();
        div()
            .id(self.id)
            .aria_label(self.label)
            .when_some(self.selector, |slider, selector| {
                slider.debug_selector(move || selector.to_string())
            })
            .flex()
            .flex_1()
            .min_w(u(60.0))
            .items_center()
            .gap(u(8.0))
            .rounded(u(theme.metrics.radius))
            .track_focus(&self.focus)
            .tab_stop(true)
            .children(super::reveal::report(&self.focus, window))
            .focus_visible(move |style| style.shadow(ring))
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let step = match event.keystroke.key.as_str() {
                    "right" | "up" => VOLUME_STEP,
                    "left" | "down" => -VOLUME_STEP,
                    _ => return,
                };
                state.update(cx, |state, cx| {
                    let next = (state.value().end() + step).clamp(VOLUME_RANGE.0, VOLUME_RANGE.1);
                    state.set_value(next, window, cx);
                    cx.emit(gpui_base::slider::SliderEvent::Change(state.value()));
                    cx.emit(gpui_base::slider::SliderEvent::Release(state.value()));
                });
                cx.stop_propagation();
            })
            .child(
                Slider::new(&self.state).flex().flex_1().child(
                    SliderTrack::new(&self.state)
                        .flex()
                        .flex_1()
                        .items_center()
                        .h(u(14.0))
                        .cursor_pointer()
                        .child(
                            SliderIndicator::new(&self.state)
                                .relative()
                                .w_full()
                                .h(u(4.0))
                                .rounded(u(2.0))
                                .bg(Hsla::from(colors.rule))
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .h_full()
                                        .w(relative(share))
                                        .rounded(u(2.0))
                                        .bg(Hsla::from(colors.cypress)),
                                )
                                // The mark of 100 %.
                                .child(
                                    div()
                                        .absolute()
                                        .left(relative(0.5))
                                        .top(u(-4.0))
                                        .w(gpui::px(1.0))
                                        .h(u(12.0))
                                        .bg(Hsla::from(colors.ink_3)),
                                )
                                .child(
                                    SliderThumb::new(&self.state)
                                        .absolute()
                                        .left(relative(share))
                                        .top(u(-4.0))
                                        .ml(u(-5.0))
                                        .w(u(10.0))
                                        .h(u(12.0))
                                        .rounded(u(2.0))
                                        .border_1()
                                        .border_color(Hsla::from(colors.ink_2))
                                        .bg(Hsla::from(colors.travertine_hi)),
                                ),
                        ),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .w(u(38.0))
                    .text_right()
                    .typeface(theme.types.mono_field, &theme)
                    .ink(colors.ink)
                    .child(format!("{}%", value.round() as i32)),
            )
    }
}

/// The sensitivity of voice activation: a track over the range of the input meter, the live
/// level of the microphone drawn on it, and the threshold as the thumb. The arrow keys move
/// the threshold by 1 dB while it holds the keyboard focus.
#[derive(IntoElement)]
pub struct LevelSlider {
    id: ElementId,
    state: Entity<SliderState>,
    focus: FocusHandle,
    label: SharedString,
    range: (f32, f32),
    /// The live level as a share of the track, from 0 to 1, while it is known.
    level: Option<f32>,
    selector: Option<SharedString>,
}

impl LevelSlider {
    pub fn new(
        id: impl Into<ElementId>,
        state: &Entity<SliderState>,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        range: (f32, f32),
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            focus: focus.clone(),
            label: label.into(),
            range,
            level: None,
            selector: None,
        }
    }

    pub fn state(range: (f32, f32), value: f32) -> SliderState {
        SliderState::new()
            .min(range.0)
            .max(range.1)
            .step(1.0)
            .default_value(value)
    }

    pub fn level(mut self, level: Option<f32>) -> Self {
        self.level = level;
        self
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}

impl RenderOnce for LevelSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let (low, high) = self.range;
        let value = self.state.read(cx).value().end();
        let share = ((value - low) / (high - low)).clamp(0.0, 1.0);
        let level = self.level.unwrap_or(0.0).clamp(0.0, 1.0);
        let ring = focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
        let state = self.state.clone();
        div()
            .id(self.id)
            .aria_label(self.label)
            .when_some(self.selector, |slider, selector| {
                slider.debug_selector(move || selector.to_string())
            })
            .flex()
            .flex_1()
            .min_w(u(60.0))
            .items_center()
            .gap(u(8.0))
            .rounded(u(theme.metrics.radius))
            .track_focus(&self.focus)
            .tab_stop(true)
            .children(super::reveal::report(&self.focus, window))
            .focus_visible(move |style| style.shadow(ring))
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let step = match event.keystroke.key.as_str() {
                    "right" | "up" => 1.0,
                    "left" | "down" => -1.0,
                    _ => return,
                };
                state.update(cx, |state, cx| {
                    let next = (state.value().end() + step).clamp(low, high);
                    state.set_value(next, window, cx);
                    cx.emit(gpui_base::slider::SliderEvent::Change(state.value()));
                    cx.emit(gpui_base::slider::SliderEvent::Release(state.value()));
                });
                cx.stop_propagation();
            })
            .child(
                Slider::new(&self.state).flex().flex_1().child(
                    SliderTrack::new(&self.state)
                        .flex()
                        .flex_1()
                        .items_center()
                        .h(u(18.0))
                        .cursor_pointer()
                        .child(
                            SliderIndicator::new(&self.state)
                                .relative()
                                .w_full()
                                .h(u(8.0))
                                .rounded(u(2.0))
                                .bg(Hsla::from(colors.rule))
                                // The live level: bright above the threshold, where it is
                                // sent, and dim below it.
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .h_full()
                                        .w(relative(level))
                                        .rounded(u(2.0))
                                        .bg(Hsla::from(if level >= share {
                                            colors.olive_bright
                                        } else {
                                            colors.ink_3
                                        })),
                                )
                                .child(
                                    SliderThumb::new(&self.state)
                                        .absolute()
                                        .left(relative(share))
                                        .top(u(-4.0))
                                        .ml(u(-3.0))
                                        .w(u(6.0))
                                        .h(u(16.0))
                                        .rounded(u(2.0))
                                        .border_1()
                                        .border_color(Hsla::from(colors.ink_2))
                                        .bg(Hsla::from(colors.travertine_hi)),
                                ),
                        ),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .w(u(52.0))
                    .text_right()
                    .typeface(theme.types.mono_field, &theme)
                    .ink(colors.ink)
                    .child(format!("{} dB", value.round() as i32)),
            )
    }
}
