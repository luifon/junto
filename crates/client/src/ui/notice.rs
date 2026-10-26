use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, Refineable, Rems, RenderOnce, StyleRefinement, Styled, Window, div,
};

use super::held::{Moves, held};
use crate::theme::u;

/// The place of a late result, such as a notice or the answer of a request (ADR 0730). It
/// has the height of its notices and none while it has no notice; a change of its height
/// waits while the pointer is on a control that the change moves ([`super::Held`]).
#[derive(IntoElement)]
pub struct NoticeSlot {
    id: ElementId,
    children: Vec<AnyElement>,
    style: StyleRefinement,
    at_end: bool,
    most: Option<Rems>,
    moves: Moves,
}

pub fn notice_slot(id: impl Into<ElementId>) -> NoticeSlot {
    NoticeSlot {
        id: id.into(),
        children: Vec::new(),
        style: StyleRefinement::default(),
        at_end: false,
        most: None,
        moves: Moves::Below,
    }
}

impl NoticeSlot {
    /// The slot follows every control of a region whose top does not move, such as a
    /// section of the settings: a notice that arrives there moves no control, so the slot
    /// takes the height of its notices at once (ADR 0730).
    pub fn at_end(mut self) -> Self {
        self.at_end = true;
        self
    }

    /// The slot is in a region whose content is centred, such as a dialog: a change of its
    /// height moves every control of the region.
    pub fn centred(mut self) -> Self {
        self.moves = Moves::Column;
        self
    }

    /// The greatest height of the slot in design pixels; its notices scroll beyond it.
    pub fn most(mut self, height: f32) -> Self {
        self.most = Some(u(height));
        self
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

/// The greatest height of a slot, in design pixels, unless its owner gives another.
const MOST: f32 = 128.0;
/// The space above the notices of a slot and between them, in design pixels.
const GAP: f32 = 6.0;

impl RenderOnce for NoticeSlot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        // The space above the notices exists only while there is a notice.
        let notices = (!self.children.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(u(GAP))
                .pt(u(GAP))
                .children(self.children)
        });
        if self.at_end {
            let mut frame = div()
                .id(self.id)
                .flex()
                .flex_col()
                .flex_none()
                .children(notices);
            frame.style().refine(&self.style);
            return frame.into_any_element();
        }
        let mut slot = held(self.id)
            .moves(self.moves)
            .most(self.most.unwrap_or_else(|| u(MOST)))
            .children(notices);
        slot.style().refine(&self.style);
        slot.into_any_element()
    }
}
