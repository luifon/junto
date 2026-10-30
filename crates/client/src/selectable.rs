mod history;

pub(crate) use history::Key as MessageKey;
use history::{Anchor, History, Paint};
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, IntoElement, KeyDownEvent, LayoutId, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, TextLayout, Window, fill, point,
    px, size, with_text_paint_observer,
};

#[derive(Clone)]
struct Part {
    text: String,
    layout: TextLayout,
    visible: Bounds<Pixels>,
    targets: Option<CopyTargets>,
    message: Option<(MessageKey, usize)>,
}

struct TextCopy {
    links: Vec<(Range<usize>, String)>,
    replacements: Vec<(Range<usize>, String)>,
}

type CopyTargets = Rc<TextCopy>;

impl Part {
    fn target(&self, byte: usize) -> Option<&(Range<usize>, String)> {
        self.targets
            .as_ref()?
            .links
            .iter()
            .find(|(range, _)| range.contains(&byte))
    }

    fn copy_bounds(&self, byte: usize) -> Bounds<Pixels> {
        if let Some((range, _)) = self.target(byte)
            && let (Some(from), Some(to)) = (
                self.layout.position_for_index(range.start),
                self.layout.position_for_index(range.end),
            )
        {
            if from.y == to.y {
                return Bounds::new(
                    from,
                    size((to.x - from.x).max(px(1.0)), self.layout.line_height()),
                )
                .intersect(&self.visible);
            }
            return Bounds::new(
                point(self.visible.left(), from.y),
                size(
                    self.visible.size.width,
                    to.y - from.y + self.layout.line_height(),
                ),
            )
            .intersect(&self.visible);
        }
        self.visible
    }
    fn source_byte(&self, byte: usize) -> usize {
        let mut copied = 0;
        let mut at = 0;
        if let Some(targets) = &self.targets {
            for (shown, source) in &targets.replacements {
                if shown.end <= at || shown.start >= byte {
                    continue;
                }
                copied += shown.start.saturating_sub(at) + source.len();
                at = shown.end.min(byte);
            }
        }
        copied + byte.saturating_sub(at)
    }

    fn copied(&self, range: Range<usize>) -> String {
        let mut out = String::new();
        let mut at = range.start;
        if let Some(targets) = &self.targets {
            for (shown, source) in &targets.replacements {
                if shown.end <= at || shown.start >= range.end {
                    continue;
                }
                if at < shown.start {
                    out.push_str(self.text.get(at..shown.start).unwrap_or_default());
                }
                out.push_str(source);
                at = shown.end.min(range.end);
            }
        }
        out.push_str(self.text.get(at..range.end).unwrap_or_default());
        out
    }
}

thread_local! {
    static MESSAGE: RefCell<Option<Rc<Paint>>> = const { RefCell::new(None) };
    static COPY_TARGETS: RefCell<Option<CopyTargets>> = const { RefCell::new(None) };
}

#[derive(Clone, Default)]
pub struct TextSelection(Rc<RefCell<Selection>>);

#[derive(Default)]
struct Selection {
    scope: Option<String>,
    parts: Vec<Part>,
    start: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
    pressed: bool,
    origin: Option<Point<Pixels>>,
    dragging: bool,
    hovered: Option<(usize, usize)>,
    focus: Option<FocusHandle>,
    control: Option<Bounds<Pixels>>,
    history: History,
    pointer: Option<Point<Pixels>>,
    scrolling: bool,
    autoscrolled: Option<Point<Pixels>>,
}

impl Selection {
    fn hit(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        self.parts
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, part)| {
                part.visible.contains(&position).then(|| {
                    let byte = part
                        .layout
                        .index_for_position(position)
                        .unwrap_or_else(|byte| byte);
                    (index, byte.min(part.text.len()))
                })
            })
    }

    fn anchor(&self, index: usize, byte: usize) -> Option<Anchor> {
        let part = self.parts.get(index)?;
        let (key, offset) = part.message?;
        Some(Anchor {
            key,
            byte: offset + part.source_byte(byte),
        })
    }

    fn nearest(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        self.hit(position)
            .filter(|(index, _)| {
                self.history.start.is_none() || self.parts[*index].message.is_some()
            })
            .or_else(|| {
                self.parts
                    .iter()
                    .enumerate()
                    .filter(|(_, part)| {
                        self.history.start.is_none_or(|start| {
                            part.message
                                .is_some_and(|(key, _)| key.lane == start.key.lane)
                        })
                    })
                    .min_by_key(|(_, part)| (part.visible.center() - position).magnitude() as i64)
                    .map(|(index, part)| {
                        (
                            index,
                            part.layout
                                .index_for_position(position)
                                .unwrap_or_else(|byte| byte)
                                .min(part.text.len()),
                        )
                    })
            })
    }

    fn range(&self, index: usize, len: usize) -> Range<usize> {
        if self.history.start.is_some() {
            let part = &self.parts[index];
            let Some((key, offset)) = part.message else {
                return 0..0;
            };
            let selected = self.history.range(key, offset + part.source_byte(len));
            let mut from = None;
            let mut to = 0;
            for (byte, character) in part.text.char_indices() {
                if selected.contains(&(offset + part.source_byte(byte))) {
                    from.get_or_insert(byte);
                    to = byte + character.len_utf8();
                }
            }
            return from.unwrap_or(0)..to;
        }

        let (Some(start), Some(end)) = (self.start, self.end) else {
            return 0..0;
        };
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        if index < start.0 || index > end.0 {
            return 0..0;
        }
        let from = if index == start.0 {
            start.1.min(len)
        } else {
            0
        };
        let to = if index == end.0 { end.1.min(len) } else { len };
        from..to
    }

    fn text(&self) -> String {
        if let Some(text) = self.history.text() {
            return text;
        }
        self.parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                let range = self.range(index, part.text.len());
                (!range.is_empty()).then(|| part.copied(range))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl TextSelection {
    #[cfg(feature = "test-support")]
    pub(crate) fn painted_text(&self) -> Vec<String> {
        self.0
            .borrow()
            .parts
            .iter()
            .map(|part| part.text.clone())
            .collect()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn text_point(&self, text: &str, byte: usize) -> Option<Point<Pixels>> {
        let held = self.0.borrow();
        let part = held.parts.iter().find(|part| part.text.contains(text))?;
        let start = part.text.find(text)?;
        let position = part.layout.position_for_index(start.checked_add(byte)?)?;
        let position = position + point(px(2.0), part.layout.line_height() / 2.0);
        part.visible.contains(&position).then_some(position)
    }

    pub(crate) fn history(
        &self,
        lane: u8,
        list: ListState,
        rows: Arc<Vec<crate::state::feed::FeedRow>>,
        names: Arc<crate::state::feed::Names>,
        messages: impl FnOnce() -> Vec<(MessageKey, String)>,
    ) {
        let mut held = self.0.borrow_mut();
        held.history.lists.insert(lane, list);
        if held
            .history
            .revisions
            .get(&lane)
            .is_some_and(|(old_rows, old_names)| {
                Arc::ptr_eq(old_rows, &rows) && Arc::ptr_eq(old_names, &names)
            })
        {
            return;
        }
        let active = held.history.start.is_some();
        held.history.seed(lane, messages());
        held.history.revisions.insert(lane, (rows, names));
        if active && held.history.start.is_none() {
            held.start = None;
            held.end = None;
            held.pressed = false;
            held.pointer = None;
        }
    }

    pub(crate) fn message(&self, child: impl IntoElement, key: MessageKey) -> SelectableText {
        let mut element = SelectableText::unobserved(child);
        element.message = self.0.borrow().history.paint(key);
        element.observed_message = true;
        element
    }

    fn scroll(&self, window: &mut Window) {
        let mut held = self.0.borrow_mut();
        if held.scrolling || !held.pressed || !held.dragging || held.history.start.is_none() {
            return;
        }
        let Some(pointer) = held.pointer else {
            return;
        };
        let Some(start) = held.history.start else {
            return;
        };
        let Some(list) = held.history.lists.get(&start.key.lane).cloned() else {
            return;
        };
        let bounds = list.viewport_bounds();
        if pointer.x < bounds.left() || pointer.x > bounds.right() {
            return;
        }
        let distance = if pointer.y <= bounds.top() + px(32.0) {
            -px(28.0)
        } else if pointer.y >= bounds.bottom() - px(32.0) {
            px(28.0)
        } else {
            return;
        };
        held.scrolling = true;
        let scope = held.scope.clone();
        drop(held);
        let selection = self.clone();
        window.on_next_frame(move |window, _| {
            let mut held = selection.0.borrow_mut();
            if held.scope != scope {
                return;
            }
            held.scrolling = false;
            if !held.pressed
                || !held.dragging
                || held.pointer != Some(pointer)
                || held.history.start != Some(start)
            {
                return;
            }
            held.autoscrolled = Some(pointer);
            drop(held);
            let maximum = list.max_offset_for_scrollbar().y;
            let current = list
                .scroll_px_offset_for_scrollbar()
                .y
                .max(-maximum)
                .min(px(0.0));
            list.set_follow_mode(gpui::FollowMode::Normal);
            list.set_offset_from_scrollbar(point(
                px(0.0),
                (current - distance).max(-maximum).min(px(0.0)),
            ));
            window.refresh();
        });
    }

    pub fn control(&self, bounds: Option<Bounds<Pixels>>) {
        self.0.borrow_mut().control = bounds;
    }
    pub fn scope(&self, scope: String) {
        let mut held = self.0.borrow_mut();
        if held.scope.as_ref() != Some(&scope) {
            *held = Selection {
                scope: Some(scope),
                focus: held.focus.clone(),
                ..Selection::default()
            };
        }
    }
    pub fn focus(&self, focus: FocusHandle) {
        self.0.borrow_mut().focus = Some(focus);
    }
    pub fn hovered(&self) -> Option<(String, Bounds<Pixels>)> {
        let held = self.0.borrow();
        if held.pressed || held.dragging {
            return None;
        }
        let (index, byte) = held.hovered?;
        let part = held.parts.get(index)?;
        let value = part.target(byte).map_or_else(
            || part.copied(0..part.text.len()),
            |(_, value)| value.clone(),
        );
        Some((value, part.copy_bounds(byte)))
    }

    pub(crate) fn stop_drag(&self) {
        let mut held = self.0.borrow_mut();
        held.pressed = false;
        held.dragging = false;
        held.pointer = None;
    }

    pub fn clear(&self) {
        let mut held = self.0.borrow_mut();
        held.start = None;
        held.end = None;
        held.hovered = None;
        held.history.clear();
        held.pointer = None;
    }
}

/// Enables selection for the read-only text of a subtree. Fields and action labels use
/// `unobserved`, so that their own input and activation retain control of the pointer.
pub struct SelectableText {
    child: AnyElement,
    selection: Option<TextSelection>,
    targets: Option<CopyTargets>,
    covered: Option<TextSelection>,
    message: Option<Rc<Paint>>,
    observed_message: bool,
}

impl SelectableText {
    pub fn new(child: impl IntoElement, selection: TextSelection) -> Self {
        Self {
            child: child.into_any_element(),
            selection: Some(selection),
            targets: None,
            covered: None,
            message: None,
            observed_message: false,
        }
    }

    pub fn unobserved(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            selection: None,
            targets: None,
            covered: None,
            message: None,
            observed_message: false,
        }
    }

    pub fn rich(
        child: impl IntoElement,
        links: Vec<(Range<usize>, String)>,
        replacements: Vec<(Range<usize>, String)>,
    ) -> Self {
        Self {
            child: child.into_any_element(),
            selection: None,
            targets: Some(Rc::new(TextCopy {
                links,
                replacements,
            })),
            covered: None,
            message: None,
            observed_message: false,
        }
    }

    pub fn value(child: impl IntoElement, value: String) -> Self {
        Self::rich(child, vec![(0..usize::MAX, value)], Vec::new())
    }

    pub fn cover(child: impl IntoElement, selection: TextSelection) -> Self {
        Self {
            child: child.into_any_element(),
            selection: None,
            targets: None,
            covered: Some(selection),
            message: None,
            observed_message: false,
        }
    }
}

impl IntoElement for SelectableText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.observed_message {
            struct Restore(Option<Rc<Paint>>);
            impl Drop for Restore {
                fn drop(&mut self) {
                    MESSAGE.set(self.0.take());
                }
            }
            if let Some(message) = &self.message {
                message.cursor.set(0);
            }
            let _restore = Restore(MESSAGE.replace(self.message.clone()));
            return self.child.paint(window, cx);
        }
        if let Some(covered) = &self.covered {
            covered.0.borrow_mut().parts.clear();
            return self.child.paint(window, cx);
        }
        if let Some(targets) = &self.targets {
            struct Restore(Option<CopyTargets>);
            impl Drop for Restore {
                fn drop(&mut self) {
                    COPY_TARGETS.set(self.0.take());
                }
            }
            let _restore = Restore(COPY_TARGETS.replace(Some(targets.clone())));
            return self.child.paint(window, cx);
        }
        let Some(selection) = self.selection.clone() else {
            return with_text_paint_observer(None, || self.child.paint(window, cx));
        };
        let down = selection.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            let mut held = down.0.borrow_mut();
            if held
                .control
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                return;
            }
            held.history.clear();
            held.pointer = Some(event.position);
            held.start = held.hit(event.position);
            held.end = held.start;
            held.origin = Some(event.position);
            held.pressed = held.start.is_some();
            held.dragging = false;
            if event.click_count >= 2
                && let Some((index, byte)) = held.start
            {
                let text = &held.parts[index].text;
                let range = if event.click_count >= 3 {
                    0..text.len()
                } else {
                    let from = text
                        .char_indices()
                        .take_while(|(at, _)| *at < byte)
                        .filter(|(_, character)| character.is_whitespace())
                        .map(|(at, character)| at + character.len_utf8())
                        .last()
                        .unwrap_or(0);
                    let to = text
                        .char_indices()
                        .find(|(at, character)| *at >= byte && character.is_whitespace())
                        .map_or(text.len(), |(at, _)| at);
                    from..to
                };
                held.start = Some((index, range.start));
                held.end = Some((index, range.end));
                if let Some(focus) = &held.focus {
                    focus.focus(window, cx);
                }
            }
            held.history.start = held
                .start
                .and_then(|(index, byte)| held.anchor(index, byte));
            held.history.end = held.end.and_then(|(index, byte)| held.anchor(index, byte));
            window.refresh();
        });
        let moved = selection.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let mut held = moved.0.borrow_mut();
            if held
                .control
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                return;
            }
            if cx.has_active_drag() {
                held.pressed = false;
                held.dragging = false;
                return;
            }
            if held.pressed && event.pressed_button == Some(MouseButton::Left) {
                if held
                    .origin
                    .is_some_and(|from| (event.position - from).magnitude() > 3.0)
                {
                    held.dragging = true;
                    if let Some(focus) = &held.focus {
                        focus.focus(window, cx);
                    }
                    held.pointer = Some(event.position);
                    held.autoscrolled = None;
                    held.end = held.nearest(event.position);
                    if let Some(anchor) =
                        held.end.and_then(|(index, byte)| held.anchor(index, byte))
                    {
                        held.history.extend(anchor);
                    }
                    window.refresh();
                    drop(held);
                    moved.scroll(window);
                    cx.stop_propagation();
                }
            } else {
                if event.pressed_button != Some(MouseButton::Left) {
                    held.pressed = false;
                    held.pointer = None;
                }
                let hovered = held.hit(event.position);
                let hover_bounds = held.hovered.and_then(|(index, byte)| {
                    held.parts.get(index).map(|part| part.copy_bounds(byte))
                });
                let keep = hover_bounds.is_some_and(|bounds| {
                    held.control
                        .as_ref()
                        .map_or(bounds, |control| bounds.union(control))
                        .contains(&event.position)
                });
                let on_text = hovered.is_some();
                if hovered != held.hovered && (on_text || !keep) {
                    held.hovered = hovered;
                    window.refresh();
                }
            }
        });
        let released = selection.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                let mut held = released.0.borrow_mut();
                if held.pressed && held.dragging {
                    held.end = held.nearest(event.position);
                    if let Some(anchor) =
                        held.end.and_then(|(index, byte)| held.anchor(index, byte))
                    {
                        held.history.extend(anchor);
                    }
                }
                held.pressed = false;
                held.pointer = None;
                if held.dragging {
                    cx.stop_propagation();
                }
            }
        });
        let by_key = selection.clone();
        window.on_key_event(move |event: &KeyDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let keys = &event.keystroke;
            if keys.key == "c" && (keys.modifiers.platform || keys.modifiers.control) {
                let copied = by_key.0.borrow().text();
                if !copied.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied));
                    cx.stop_propagation();
                    window.refresh();
                }
            } else if !matches!(keys.key.as_str(), "shift" | "control" | "alt" | "cmd") {
                by_key.clear();
            }
        });
        let mut painted = Vec::new();
        std::mem::swap(&mut painted, &mut selection.0.borrow_mut().parts);
        let next = selection.clone();
        with_text_paint_observer(
            Some(Rc::new(move |layout, text, window, cx| {
                let visible = layout.bounds().intersect(&window.content_mask().bounds);
                if visible.size.width <= px(0.0) || visible.size.height <= px(0.0) {
                    return;
                }
                let mut held = next.0.borrow_mut();
                let mut part = Part {
                    text: text.to_owned(),
                    layout: layout.clone(),
                    visible,
                    targets: COPY_TARGETS.with_borrow(Clone::clone),
                    message: None,
                };
                MESSAGE.with_borrow(|source| {
                    if let Some(source) = source {
                        let copied = part.copied(0..part.text.len());
                        if !copied.is_empty()
                            && let Some(offset) = source
                                .text
                                .get(source.cursor.get()..)
                                .and_then(|rest| rest.find(&copied))
                        {
                            let offset = source.cursor.get() + offset;
                            part.message = Some((source.key, offset));
                            source.cursor.set(offset + copied.len());
                        }
                    }
                });
                held.parts.push(part);
                let index = held.parts.len() - 1;
                let range = held.range(index, text.len());
                let color = crate::theme::theme(cx).colors.target_wash;
                for (byte, character) in
                    text.char_indices().filter(|(byte, _)| range.contains(byte))
                {
                    if character == '\n' {
                        continue;
                    }
                    let Some(from) = layout.position_for_index(byte) else {
                        continue;
                    };
                    let to = layout.position_for_index(byte + character.len_utf8());
                    let right = to
                        .filter(|to| to.y == from.y)
                        .map_or(layout.bounds().right(), |to| to.x);
                    window.paint_quad(fill(
                        Bounds::new(
                            from,
                            size((right - from.x).max(px(1.0)), layout.line_height()),
                        ),
                        color,
                    ));
                }
            })),
            || self.child.paint(window, cx),
        );
        {
            let mut held = selection.0.borrow_mut();
            if let Some((index, _)) = held.hovered
                && painted.get(index).map(|part| &part.text)
                    != held.parts.get(index).map(|part| &part.text)
            {
                held.hovered = None;
            }
            if held.history.start.is_none()
                && let (Some(start), Some(end)) = (held.start, held.end)
            {
                let from = start.0.min(end.0);
                let to = start.0.max(end.0);
                if (from..=to).any(|index| {
                    painted.get(index).map(|part| &part.text)
                        != held.parts.get(index).map(|part| &part.text)
                }) {
                    held.start = None;
                    held.end = None;
                    held.dragging = false;
                    held.pressed = false;
                }
            }
            if held.pressed
                && held.dragging
                && let Some(pointer) = held
                    .autoscrolled
                    .take()
                    .filter(|pointer| Some(*pointer) == held.pointer)
                && let Some(anchor) = held
                    .nearest(pointer)
                    .and_then(|(index, byte)| held.anchor(index, byte))
                && !held.history.extend(anchor)
            {
                held.pointer = None;
            }
        }
        selection.scroll(window);
    }
}
