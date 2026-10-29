use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, TextLayout, Window, fill, point, px, size,
    with_text_paint_observer,
};

#[derive(Clone)]
struct Part {
    text: String,
    layout: TextLayout,
    visible: Bounds<Pixels>,
    targets: Option<CopyTargets>,
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

    fn range(&self, index: usize, len: usize) -> Range<usize> {
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

    pub fn clear(&self) {
        let mut held = self.0.borrow_mut();
        held.start = None;
        held.end = None;
        held.hovered = None;
    }
}

/// Enables selection for the read-only text of a subtree. Fields and action labels use
/// `unobserved`, so that their own input and activation retain control of the pointer.
pub struct SelectableText {
    child: AnyElement,
    selection: Option<TextSelection>,
    targets: Option<CopyTargets>,
    covered: Option<TextSelection>,
}

impl SelectableText {
    pub fn new(child: impl IntoElement, selection: TextSelection) -> Self {
        Self {
            child: child.into_any_element(),
            selection: Some(selection),
            targets: None,
            covered: None,
        }
    }

    pub fn unobserved(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            selection: None,
            targets: None,
            covered: None,
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
                    held.end = held.hit(event.position).or_else(|| {
                        held.parts
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, part)| {
                                let distance = (part.visible.center() - event.position).magnitude();
                                distance as i64
                            })
                            .map(|(index, part)| {
                                (
                                    index,
                                    part.layout
                                        .index_for_position(event.position)
                                        .unwrap_or_else(|byte| byte)
                                        .min(part.text.len()),
                                )
                            })
                    });
                    window.refresh();
                    cx.stop_propagation();
                }
            } else {
                let hovered = held.hit(event.position);
                let hover_bounds = held.hovered.and_then(|(index, byte)| {
                    held.parts.get(index).map(|part| part.copy_bounds(byte))
                });
                let keep = hover_bounds.is_some_and(|bounds| {
                    Bounds::new(
                        bounds.origin,
                        size(bounds.size.width + px(104.0), bounds.size.height + px(34.0)),
                    )
                    .contains(&event.position)
                });
                let on_text = hovered.is_some()
                    && hover_bounds.is_some_and(|bounds| event.position.y < bounds.bottom());
                if hovered != held.hovered && (on_text || !keep) {
                    held.hovered = hovered;
                    window.refresh();
                }
            }
        });
        let released = selection.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                let mut held = released.0.borrow_mut();
                held.pressed = false;
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
                let index = held.parts.len();
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
                held.parts.push(Part {
                    text: text.to_owned(),
                    layout: layout.clone(),
                    visible,
                    targets: COPY_TARGETS.with_borrow(Clone::clone),
                });
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
            if let (Some(start), Some(end)) = (held.start, held.end) {
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
        }
    }
}
