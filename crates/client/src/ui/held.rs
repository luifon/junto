use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, ElementId, InteractiveElement, IntoElement,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Refineable, Rems,
    RenderOnce, Role, ScrollHandle, Size, StatefulInteractiveElement, StyleRefinement, Styled,
    Window, canvas, div, prelude::FluentBuilder,
};

use crate::text;

/// The controls that a change of the height of a part moves (ADR 0730).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moves {
    /// The controls under the top of the part, between its left and right edges: the part
    /// is in a region whose top does not move.
    Below,
    /// Every control between the left and right edges of the part: the part is in a region
    /// whose content is centred, such as a dialog or the stage of a call.
    Column,
}

impl Moves {
    fn covers(self, part: Bounds<Pixels>, at: Point<Pixels>) -> bool {
        let beside = at.x < part.left() || at.x > part.right();
        match self {
            Self::Below => !beside && at.y >= part.top(),
            Self::Column => !beside,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Heights {
    /// The height that the part has on the screen; none before its first drawing.
    shown: Option<Pixels>,
    /// The height of its content at the last drawing.
    content: Pixels,
    /// The size of the window at the last drawing.
    window: Option<Size<Pixels>>,
    /// The frames in a row in which the height changed.
    changes: u8,
    /// A button of the pointer is pressed: a change waits until it is released.
    pressed: bool,
}

/// The most frames in a row in which a part asks for a new frame to change its height. Two
/// parts with one element id share their heights and would change them in turn at each
/// frame; the part then stops asking, and a build for the tests records it.
const MOST_CHANGES: u8 = 3;

/// A part whose content arrives at a time that the person does not choose, such as a notice
/// or the result of a request: it has the height of its content, up to `most`, and scrolls
/// beyond it. An empty part has no height (ADR 0730).
///
/// A change of the height of its content moves the controls under it or around it. The
/// change waits while the pointer is where it moves controls ([`Moves`]) and takes effect
/// when the pointer leaves that place or the size of the window changes, so a press never
/// lands on a control that moved under the pointer.
#[derive(IntoElement)]
pub struct Held {
    id: ElementId,
    label: &'static str,
    children: Vec<AnyElement>,
    style: StyleRefinement,
    most: Option<Rems>,
    moves: Moves,
    scroll: Option<ScrollHandle>,
    selector: Option<&'static str>,
    grows_at_once: bool,
}

pub fn held(id: impl Into<ElementId>) -> Held {
    Held {
        id: id.into(),
        label: text::NOTIFICATIONS,
        children: Vec::new(),
        style: StyleRefinement::default(),
        most: None,
        moves: Moves::Below,
        scroll: None,
        selector: None,
        grows_at_once: false,
    }
}

impl Held {
    /// The greatest height of the part; its content scrolls beyond it.
    pub fn most(mut self, height: Rems) -> Self {
        self.most = Some(height);
        self
    }

    pub fn moves(mut self, moves: Moves) -> Self {
        self.moves = moves;
        self
    }

    /// The name of the list of the part in the accessibility tree.
    pub fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    /// Keeps the position of the content in `handle`; without it the part keeps it.
    pub fn scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// The part grows at once and only a smaller height waits: its content arrives as a
    /// part of a change that the person made, such as the step that a press opened.
    pub fn grows_at_once(mut self) -> Self {
        self.grows_at_once = true;
        self
    }

    /// The name of the list of the part for the tests.
    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }
}

impl ParentElement for Held {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Held {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Held {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (heights, scroll) = window.with_element_namespace(self.id.clone(), |window| {
            let heights = window
                .use_keyed_state("held-heights", cx, |_, _| {
                    Rc::new(Cell::new(Heights::default()))
                })
                .read(cx)
                .clone();
            let scroll = window
                .use_keyed_state("held-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone();
            (heights, scroll)
        });
        let scroll = self.scroll.unwrap_or(scroll);
        let measured = heights.clone();
        let content = div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .children(self.children)
            .child(
                canvas(
                    move |bounds, _, _| {
                        let mut now = measured.get();
                        now.content = bounds.size.height;
                        measured.set(now);
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            );
        let (most, moves, grows_at_once) = (self.most, self.moves, self.grows_at_once);
        let wanted = move |heights: Heights, rem: Pixels| match most {
            Some(most) => heights.content.min(most.to_pixels(rem)),
            None => heights.content,
        };
        #[cfg(feature = "test-support")]
        let id = self.id.clone();
        let decided = heights.clone();
        let waiting = heights.clone();
        let mut frame = div()
            .id(self.id)
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .overflow_hidden()
            .when_some(heights.get().shown, |frame, shown| frame.h(shown))
            .when_some(most, |frame, most| frame.max_h(most))
            // One structure in every frame: before its first measure the part has the
            // height of its content, after it the list of its content scrolls in it.
            .child(
                div()
                    .id("held-content")
                    .role(Role::List)
                    .aria_label(self.label)
                    .when_some(self.selector, |list, selector| {
                        list.debug_selector(move || selector.to_owned())
                    })
                    .flex()
                    .flex_col()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(content),
            )
            .child(super::scroll::scroll_bar(&scroll, false, cx))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let mut now = decided.get();
                        let target = wanted(now, window.rem_size());
                        let size = window.viewport_size();
                        let resized = now.window.is_some_and(|before| before != size);
                        now.window = Some(size);
                        // A change waits while a button of the pointer is pressed: a press
                        // ends on the control where it started.
                        if now.shown == Some(target) {
                            now.changes = 0;
                        } else if now.shown.is_none()
                            || resized
                            || (grows_at_once && now.shown.is_some_and(|shown| target > shown))
                            || (!now.pressed && !moves.covers(bounds, window.mouse_position()))
                        {
                            now.shown = Some(target);
                            now.changes = now.changes.saturating_add(1);
                            // At its first drawing the part was laid out at the height of
                            // its content, which needs no further frame.
                            if bounds.size.height == target {
                                now.changes = 0;
                            } else if now.changes <= MOST_CHANGES {
                                // A refresh during the drawing is lost; the next frame draws
                                // the new height.
                                window.defer(cx, |window, _| window.refresh());
                            } else {
                                #[cfg(feature = "test-support")]
                                record::unsettled(&id);
                            }
                        }
                        decided.set(now);
                        bounds
                    },
                    move |_, bounds, window, _| {
                        let pressing = waiting.clone();
                        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, _| {
                            if phase == DispatchPhase::Capture {
                                let mut now = pressing.get();
                                now.pressed = true;
                                pressing.set(now);
                            }
                        });
                        let releasing = waiting.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _| {
                            if phase == DispatchPhase::Capture {
                                let mut now = releasing.get();
                                now.pressed = false;
                                releasing.set(now);
                                window.refresh();
                            }
                        });
                        let now = waiting.get();
                        if now.shown == Some(wanted(now, window.rem_size())) {
                            return;
                        }
                        let waiting = waiting.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                            let mut now = waiting.get();
                            let target = wanted(now, window.rem_size());
                            if phase == DispatchPhase::Bubble
                                && now.shown != Some(target)
                                && event.pressed_button.is_none()
                                && !moves.covers(bounds, event.position)
                            {
                                now.shown = Some(target);
                                waiting.set(now);
                                window.refresh();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );
        frame.style().refine(&self.style);
        frame
    }
}

#[cfg(feature = "test-support")]
mod record {
    use std::cell::RefCell;

    use gpui::ElementId;

    thread_local! {
        static UNSETTLED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn unsettled(id: &ElementId) {
        UNSETTLED.with(|unsettled| unsettled.borrow_mut().push(format!("{id}")));
    }

    /// The element ids of the parts whose height changed in too many frames in a row since
    /// the last call: two parts that share an id.
    pub fn take_unsettled() -> Vec<String> {
        UNSETTLED.with(|unsettled| std::mem::take(&mut *unsettled.borrow_mut()))
    }
}

#[cfg(feature = "test-support")]
pub use record::take_unsettled;
