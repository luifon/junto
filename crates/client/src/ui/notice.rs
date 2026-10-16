use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, Refineable,
    RenderOnce, ScrollHandle, StyleRefinement, Styled, Window, div,
};

use super::ScrollArea;
use crate::{text, theme::u};

/// Space for a late result without moving the controls around it (ADR 0730).
#[derive(IntoElement)]
pub struct NoticeSlot {
    id: ElementId,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

pub fn notice_slot(id: impl Into<ElementId>) -> NoticeSlot {
    NoticeSlot {
        id: id.into(),
        children: Vec::new(),
        style: StyleRefinement::default(),
    }
}

impl ParentElement for NoticeSlot {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for NoticeSlot {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for NoticeSlot {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scroll = window.with_element_namespace(self.id.clone(), |window| {
            window
                .use_keyed_state("notice-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone()
        });
        let mut frame = div()
            .id(self.id)
            .flex()
            .flex_col()
            .flex_none()
            .h(u(64.0))
            .child(
                ScrollArea::new("notice-content", &scroll, text::NOTIFICATIONS)
                    .children(self.children),
            );
        frame.style().refine(&self.style);
        frame
    }
}
