//! The segmented control: one choice of a few, side by side in one frame.

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use super::{StyleExt, focus_ring_inside};
use crate::theme::{theme, u};

type SelectHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    label: SharedString,
    options: Vec<SharedString>,
    selected: Option<usize>,
    selector: Option<&'static str>,
    on_select: Option<SelectHandler>,
}

impl Segmented {
    /// A control with these options. `label` names it in the accessibility tree.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        options: impl IntoIterator<Item = SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            options: options.into_iter().collect(),
            selected: None,
            selector: None,
            on_select: None,
        }
    }

    pub fn selected(mut self, index: Option<usize>) -> Self {
        self.selected = index;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let selector = self.selector;
        let focus: Vec<_> = (0..self.options.len())
            .map(|index| {
                super::reveal::inner_handle(&self.id, &("segment", index).into(), window, cx)
            })
            .collect();
        let segments = self.options.into_iter().enumerate().map(|(index, option)| {
            let on = self.selected == Some(index);
            let handler = self.on_select.clone();
            let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
            div()
                .id(("segment", index))
                .role(Role::RadioButton)
                .aria_label(option.clone())
                .aria_selected(on)
                .when_some(selector, |segment, selector| {
                    segment.debug_selector(move || format!("{selector}-{index}"))
                })
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .h(u(26.0))
                .typeface(
                    if on {
                        theme.types.control_strong
                    } else {
                        theme.types.control
                    },
                    &theme,
                )
                .text_size(u(13.0))
                .ink(if on { colors.ink } else { colors.ink_2 })
                .when(on, |segment| segment.bg(Hsla::from(colors.limestone_lo)))
                .when(index > 0, |segment| {
                    segment.border_l_1().border_color(Hsla::from(colors.rule))
                })
                .cursor_pointer()
                .track_focus(&focus[index])
                .child(super::reveal::report(&focus[index]))
                .when(!on, |segment| {
                    segment.hover(move |style| style.bg(Hsla::from(colors.travertine)))
                })
                .focus_visible(move |style| style.shadow(ring))
                .when_some(handler, |segment, handler| {
                    segment.on_click(move |_, window, cx| handler(index, window, cx))
                })
                .child(option)
        });
        div()
            .id(self.id)
            .role(Role::RadioGroup)
            .aria_label(self.label)
            .flex()
            .flex_1()
            .overflow_hidden()
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .children(segments)
    }
}
