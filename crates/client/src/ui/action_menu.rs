use super::{ContextMenuExt, Menu, MenuItem};
use crate::{assets::Icon, text};
use gpui::{
    App, Bounds, ClickEvent, Context, Div, ElementId, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Point, Render, SharedString, Stateful, Styled, WeakFocusHandle, Window,
    canvas, div, prelude::FluentBuilder,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Handler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone)]
/// One button action collected while its target is built.
pub struct RowAction {
    pub(crate) label: SharedString,
    pub(crate) icon: Option<Icon>,
    pub(crate) disabled: bool,
    pub(crate) detail: Option<SharedString>,
    pub(crate) handler: Option<Handler>,
}

impl RowAction {
    pub fn new(
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon: None,
            disabled: false,
            detail: None,
            handler: Some(Rc::new(handler)),
        }
    }

    /// Adds an action without a visible button to the current collection scope.
    pub fn register(self) {
        record(|| Some(self));
    }

    fn same_presentation(&self, other: &Self) -> bool {
        self.label == other.label
            && self.icon == other.icon
            && self.disabled == other.disabled
            && self.detail == other.detail
    }
}

thread_local! { static SCOPES: RefCell<Vec<Vec<RowAction>>> = const { RefCell::new(Vec::new()) }; }

struct Scope(bool);

impl Drop for Scope {
    fn drop(&mut self) {
        if self.0 {
            SCOPES.with(|scopes| {
                scopes.borrow_mut().pop();
            });
        }
    }
}

pub(super) fn record(make: impl FnOnce() -> Option<RowAction>) {
    SCOPES.with(|scopes| {
        if let Some(actions) = scopes.borrow_mut().last_mut()
            && let Some(action) = make()
        {
            actions.push(action);
        }
    });
}

/// Builds one target and retains its button actions for `bind_action_menu`.
pub fn collect_actions<T>(build: impl FnOnce() -> T) -> (T, Vec<RowAction>) {
    SCOPES.with(|scopes| scopes.borrow_mut().push(Vec::new()));
    let mut scope = Scope(true);
    let built = build();
    let actions = SCOPES.with(|scopes| scopes.borrow_mut().pop().unwrap_or_default());
    scope.0 = false;
    (built, actions)
}

/// Adds a menu without replacing the target's focus handle or its form.
pub fn bind_action_menu(
    frame: Stateful<Div>,
    id: impl Into<ElementId>,
    actions: Vec<RowAction>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let popup = window.use_keyed_state((id, "actions"), cx, |_, cx| Popup {
        focus: cx.focus_handle(),
        return_focus: None,
        shown: false,
        actions: Vec::new(),
        position: Point::default(),
        bounds: Rc::new(Cell::new(Bounds::default())),
        placement: Default::default(),
    });
    let has_actions = !actions.is_empty();
    let bounds = popup.update(cx, |popup, cx| {
        let changed = popup.actions.len() != actions.len()
            || popup
                .actions
                .iter()
                .zip(&actions)
                .any(|(old, new)| !old.same_presentation(new));
        popup.actions = actions;
        if popup.shown && changed {
            cx.notify();
        }
        popup.bounds.clone()
    });
    let by_open = popup.clone();
    frame
        .when(has_actions, |frame| {
            frame.on_context_menu(move |window, cx| {
                by_open.update(cx, |popup, cx| {
                    if !popup.shown {
                        popup.return_focus = window.focused(cx).map(|focus| focus.downgrade());
                    }
                    let bounds = popup.bounds.get();
                    popup.position = if window.last_input_was_keyboard() {
                        gpui::point(bounds.left(), bounds.bottom())
                    } else {
                        window.mouse_position()
                    };
                    popup.placement = Default::default();
                    popup.shown = true;
                    popup.focus.focus(window, cx);
                    cx.notify();
                });
            })
        })
        .child(
            canvas(move |value, _, _| bounds.set(value), |_, (), _, _| {})
                .absolute()
                .inset_0(),
        )
        .child(popup)
}

struct Popup {
    focus: FocusHandle,
    return_focus: Option<WeakFocusHandle>,
    shown: bool,
    actions: Vec<RowAction>,
    position: Point<Pixels>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    placement: Rc<super::MenuPlacement>,
}

impl Popup {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        if let Some(focus) = self.return_focus.take().and_then(|focus| focus.upgrade()) {
            focus.focus(window, cx);
        }
        cx.notify();
    }
}

impl Render for Popup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.shown || self.actions.is_empty() {
            return div().absolute().size_0().into_any_element();
        }
        let close = cx.listener(|this, _: &(), window, cx| this.close(window, cx));
        let items = self.actions.iter().enumerate().map(|(index, action)| {
            let expected = action.label.clone();
            let selector = format!("context-action:{}", action.label);
            div().debug_selector(move || selector).child(
                MenuItem::new(("context-action", index), action.label.clone())
                    .disabled(action.disabled)
                    .when_some(action.icon, |item, icon| item.icon(icon))
                    .when_some(action.detail.clone(), |item, detail| item.detail(detail))
                    .on_click(cx.listener(move |this, event, window, cx| {
                        let action = this.actions.get(index).cloned();
                        if !this.shown {
                            return;
                        }
                        this.close(window, cx);
                        if let Some(action) = action
                            && action.label == expected
                            && !action.disabled
                            && let Some(handler) = action.handler
                        {
                            handler(event, window, cx);
                        }
                    })),
            )
        });
        Menu::new("row-context-menu", text::ROW_ACTIONS, &self.focus)
            .selector("row-context-menu")
            .on_close(move |window, cx| close(&(), window, cx))
            .children(items)
            .popup_at_stable(self.position, self.placement.clone())
    }
}

#[cfg(test)]
mod tests;
