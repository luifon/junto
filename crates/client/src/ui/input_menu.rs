use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Point, Render, RenderOnce, Styled, Window, div,
};
use gpui_base::input::{InputBaseState, InputContextMenuCapabilities, InputModeKind};

use super::{Menu, MenuItem};
use crate::text;

#[derive(IntoElement)]
pub struct InputMenu<M: InputModeKind> {
    state: Entity<InputBaseState<M>>,
    child: AnyElement,
}

impl<M: InputModeKind> InputMenu<M> {
    pub fn new(state: &Entity<InputBaseState<M>>, child: impl IntoElement) -> Self {
        Self {
            state: state.clone(),
            child: child.into_any_element(),
        }
    }
}

impl<M: InputModeKind> RenderOnce for InputMenu<M> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let input_focus = self.state.read(cx).focus_handle(cx);
        let popup =
            window.use_keyed_state(("input-edit-popup", self.state.entity_id()), cx, |_, cx| {
                Popup {
                    focus: cx.focus_handle(),
                    input_focus,
                    shown: None,
                    position: Point::default(),
                }
            });
        let by_mouse = popup.clone();
        self.state.update(cx, |input, _| {
            input.on_context_menu(Rc::new(move |_, capabilities, position, window, cx| {
                by_mouse.update(cx, |popup, cx| {
                    popup.open(capabilities, position, window, cx)
                });
            }));
        });
        let by_key = popup.clone();
        let state = self.state;
        div()
            .relative()
            .w_full()
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_key_down(move |event, window, cx| {
                if !super::menu::context_menu_key(&event.keystroke) {
                    return;
                }
                let input = state.read(cx);
                let capabilities = input.context_menu_capabilities();
                if capabilities.is_disabled() {
                    return;
                }
                let position = input.cursor_layout().map_or_else(
                    || window.mouse_position(),
                    |(bounds, _)| gpui::point(bounds.left(), bounds.bottom()),
                );
                cx.stop_propagation();
                by_key.update(cx, |popup, cx| {
                    popup.open(capabilities, position, window, cx)
                });
            })
            .child(self.child)
            .child(popup)
    }
}

struct Popup {
    focus: FocusHandle,
    input_focus: FocusHandle,
    shown: Option<InputContextMenuCapabilities>,
    position: Point<Pixels>,
}

impl Popup {
    fn open(
        &mut self,
        capabilities: InputContextMenuCapabilities,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shown = Some(capabilities);
        self.position = position;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = None;
        self.input_focus.focus(window, cx);
        cx.notify();
    }
}

#[derive(Clone, Copy)]
enum Edit {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl Edit {
    fn action(self) -> Box<dyn gpui::Action> {
        match self {
            Self::Cut => Box::new(gpui_base::input::Cut),
            Self::Copy => Box::new(gpui_base::input::Copy),
            Self::Paste => Box::new(gpui_base::input::Paste),
            Self::SelectAll => Box::new(gpui_base::input::SelectAll),
        }
    }
}

impl Render for Popup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(capabilities) = self.shown else {
            return div().absolute().size_0().into_any_element();
        };
        let close = cx.listener(|this, _: &(), window, cx| this.close(window, cx));
        let items = [
            ("input-edit-cut", text::CUT_SELECTION, Edit::Cut),
            ("input-edit-copy", text::COPY_SELECTION, Edit::Copy),
            ("input-edit-paste", text::PASTE_TEXT, Edit::Paste),
            (
                "input-edit-select-all",
                text::SELECT_ALL_TEXT,
                Edit::SelectAll,
            ),
        ]
        .into_iter()
        .map(|(selector, title, edit)| {
            let disabled = match edit {
                Edit::Cut => !capabilities.is_editable() || !capabilities.is_copyable(),
                Edit::Copy => capabilities.is_disabled() || !capabilities.is_copyable(),
                Edit::Paste => !capabilities.is_editable(),
                Edit::SelectAll => capabilities.is_disabled(),
            };
            MenuItem::new(selector, title)
                .selector(selector)
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close(window, cx);
                    window.dispatch_action(edit.action(), cx);
                }))
        });
        Menu::new("input-edit-menu", text::EDIT_TEXT_MENU, &self.focus)
            .selector("input-edit-menu")
            .on_close(move |window, cx| close(&(), window, cx))
            .children(items)
            .popup_at(self.position)
    }
}
