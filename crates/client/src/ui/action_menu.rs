use super::{ContextMenuExt, Menu, MenuItem};
use crate::{assets::Icon, text};
use gpui::{
    App, Bounds, ClickEvent, Context, Div, ElementId, Entity, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Render, SharedString, Stateful, Styled,
    WeakFocusHandle, Window, canvas, div, prelude::FluentBuilder,
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
    pub(crate) shortcut: Option<junto_client_core::keybinds::KeyAction>,
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
            shortcut: None,
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
            && self.shortcut == other.shortcut
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
    bind_menu(frame, id.into(), actions, false, None, window, cx)
}

/// The focus handle of a row of a list that has a menu button, kept under the row's
/// identifier, so that the row can tell whether it holds the keyboard focus.
pub fn row_focus(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state((id.into(), "row-focus"), cx, |_, cx| {
            cx.focus_handle().tab_stop(true)
        })
        .read(cx)
        .clone()
}

/// Where the menu button of a row lies (ADR 0731).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowMenuPlace {
    /// Over the top right of the row, on the ground of the hovered row.
    Overlay,
    /// After the other children of the row, in its flow.
    End,
}

/// The menu button that a row with a menu shows: it opens the popup that the binding owns,
/// so it is created by the binding.
#[derive(Clone)]
pub struct RowMenu {
    pub place: RowMenuPlace,
    /// The row holds the keyboard focus.
    pub focused: bool,
    /// The row is on the basalt of the server book.
    pub dark: bool,
    /// The name of the button for the tests; `menu-button:<identifier>` by default.
    pub selector: Option<SharedString>,
    /// The distance of an overlaid button from the top of the row, in design pixels.
    pub top: f32,
}

impl RowMenu {
    pub fn overlay(focused: bool) -> Self {
        Self {
            place: RowMenuPlace::Overlay,
            focused,
            dark: false,
            selector: None,
            top: 6.0,
        }
    }

    pub fn end(focused: bool) -> Self {
        Self {
            place: RowMenuPlace::End,
            ..Self::overlay(focused)
        }
    }

    pub fn dark(mut self) -> Self {
        self.dark = true;
        self
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn top(mut self, top: f32) -> Self {
        self.top = top;
        self
    }
}

/// The same for a row of a list: the row also shows the menu button (`RowMenu`), while the
/// pointer is over the row, while the row holds the keyboard focus and while the menu is
/// open.
pub fn bind_row_action_menu(
    frame: Stateful<Div>,
    id: impl Into<ElementId>,
    row: RowMenu,
    actions: Vec<RowAction>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    bind_menu(frame, id.into(), actions, false, Some(row), window, cx)
}

pub fn bind_lazy_row_action_menu<T: 'static>(
    frame: Stateful<Div>,
    id: impl Into<ElementId>,
    row: Option<RowMenu>,
    window: &mut Window,
    cx: &mut Context<T>,
    make: impl FnOnce(&mut Window, &mut Context<T>) -> Vec<RowAction>,
) -> Stateful<Div> {
    lazy_menu(frame, id.into(), row, window, cx, make)
}

pub fn bind_lazy_action_menu<T: 'static>(
    frame: Stateful<Div>,
    id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut Context<T>,
    make: impl FnOnce(&mut Window, &mut Context<T>) -> Vec<RowAction>,
) -> Stateful<Div> {
    lazy_menu(frame, id.into(), None, window, cx, make)
}

fn lazy_menu<T: 'static>(
    frame: Stateful<Div>,
    id: ElementId,
    row: Option<RowMenu>,
    window: &mut Window,
    cx: &mut Context<T>,
    make: impl FnOnce(&mut Window, &mut Context<T>) -> Vec<RowAction>,
) -> Stateful<Div> {
    let shown = popup_state(&id, window, cx).read(cx).shown;
    let actions = if shown { make(window, cx) } else { Vec::new() };
    bind_menu(frame, id, actions, true, row, window, cx)
}

fn with_row_button(
    frame: Stateful<Div>,
    id: &ElementId,
    row: RowMenu,
    popup: &Entity<Popup>,
    cx: &App,
) -> Stateful<Div> {
    let colors = crate::theme::theme(cx).colors;
    let opener = popup.clone();
    let button = super::MenuButton::new("row-menu")
        .selector(
            row.selector
                .unwrap_or_else(|| format!("menu-button:{id}").into()),
        )
        .shown(row.focused || popup.read(cx).shown)
        .when(row.dark, super::MenuButton::dark)
        .on_open(move |window, cx| opener.update(cx, |popup, cx| popup.open(window, cx)));
    match row.place {
        // The button is taller than the line of a header: in a box without height it adds
        // nothing to the height of the row, so the rows under it do not move.
        RowMenuPlace::End => frame.child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(gpui::px(0.0))
                .child(button),
        ),
        RowMenuPlace::Overlay => frame.relative().child(
            div()
                .absolute()
                .top(crate::theme::u(row.top))
                .right(crate::theme::u(6.0))
                .child(button.ground(colors.limestone_hover)),
        ),
    }
}

fn popup_state(id: &ElementId, window: &mut Window, cx: &mut App) -> Entity<Popup> {
    window.use_keyed_state((id.clone(), "actions"), cx, |_, cx| Popup {
        focus: cx.focus_handle(),
        return_focus: None,
        shown: false,
        actions: Vec::new(),
        position: Point::default(),
        bounds: Rc::new(Cell::new(Bounds::default())),
        placement: Default::default(),
    })
}

fn bind_menu(
    frame: Stateful<Div>,
    id: ElementId,
    actions: Vec<RowAction>,
    lazy: bool,
    row: Option<RowMenu>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let popup = popup_state(&id, window, cx);
    let frame = match row {
        Some(row) => with_row_button(frame, &id, row, &popup, cx),
        None => frame,
    };
    let has_actions = lazy || !actions.is_empty();
    let bounds = popup.update(cx, |popup, cx| {
        let changed = popup.actions.len() != actions.len()
            || popup
                .actions
                .iter()
                .zip(&actions)
                .any(|(old, new)| !old.same_presentation(new));
        popup.actions = actions;
        if popup.shown && popup.actions.is_empty() {
            popup.close(window, cx);
        } else if popup.shown && changed {
            cx.notify();
        }
        popup.bounds.clone()
    });
    let by_open = popup.clone();
    frame
        .group(super::MENU_ROW)
        .when(has_actions, |frame| {
            frame.on_context_menu(move |window, cx| {
                by_open.update(cx, |popup, cx| popup.open(window, cx));
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
    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown {
            self.return_focus = window.focused(cx).map(|focus| focus.downgrade());
        }
        let bounds = self.bounds.get();
        self.position = if window.last_input_was_keyboard() {
            gpui::point(bounds.left(), bounds.bottom())
        } else {
            window.mouse_position()
        };
        self.placement = Default::default();
        self.shown = true;
        self.focus.focus(window, cx);
        cx.notify();
    }

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
                    .when_some(action.shortcut, |item, keys| item.shortcut(keys))
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
