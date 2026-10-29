use std::ops::Range;

use gpui::{
    AnyElement, App, ElementId, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    Pixels, Point, RenderOnce, SharedString, StatefulInteractiveElement, Styled, TextLayout,
    Window, div,
};

type Handler = dyn Fn(Vec<usize>, Point<Pixels>, &mut Window, &mut App);

#[derive(IntoElement)]
pub struct TextTargets {
    id: ElementId,
    label: SharedString,
    layout: TextLayout,
    ranges: Vec<Range<usize>>,
    child: AnyElement,
    focus: Option<FocusHandle>,
    handler: Option<Box<Handler>>,
}

impl TextTargets {
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        layout: TextLayout,
        ranges: Vec<Range<usize>>,
        child: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            layout,
            ranges,
            child: child.into_any_element(),
            focus: None,
            handler: None,
        }
    }

    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn on_menu(
        mut self,
        handler: impl Fn(Vec<usize>, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TextTargets {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(handler) = self.handler.filter(|_| !self.ranges.is_empty()) else {
            return div().child(self.child).into_any_element();
        };
        let handler: std::rc::Rc<Handler> = handler.into();
        let source = self.focus.unwrap_or_else(|| {
            window
                .use_keyed_state((self.id.clone(), "focus"), cx, |_, cx| {
                    cx.focus_handle().tab_stop(true)
                })
                .read(cx)
                .clone()
        });
        let keyboard = source.clone();
        let (mouse_layout, key_layout) = (self.layout.clone(), self.layout);
        let (mouse_ranges, key_ranges) = (self.ranges.clone(), self.ranges);
        let mouse_handler = handler.clone();
        let theme = crate::theme::theme(cx);
        let ring = super::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        div()
            .id(self.id)
            .min_w_0()
            .role(gpui::Role::Group)
            .aria_label(self.label)
            .track_focus(&source)
            .focus_visible(move |style| style.shadow(ring))
            .capture_any_mouse_down(move |event, window, cx| {
                if event.button != gpui::MouseButton::Right {
                    return;
                }
                let selected = mouse_layout
                    .index_for_position(event.position)
                    .ok()
                    .and_then(|index| mouse_ranges.iter().position(|range| range.contains(&index)));
                if let Some(index) = selected {
                    cx.stop_propagation();
                    mouse_handler(vec![index], event.position, window, cx);
                }
            })
            .on_key_down(move |event, window, cx| {
                if !keyboard.is_focused(window)
                    || !(super::menu::context_menu_key(&event.keystroke)
                        || matches!(event.keystroke.key.as_str(), "enter" | "space"))
                {
                    return;
                }
                cx.stop_propagation();
                let position = key_layout
                    .position_for_index(key_ranges[0].start)
                    .unwrap_or_else(|| window.mouse_position());
                handler((0..key_ranges.len()).collect(), position, window, cx);
            })
            .child(self.child)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
