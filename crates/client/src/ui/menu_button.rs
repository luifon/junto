//! The menu button of a row (ADR 0731): vertical dots that open the same menu as a right
//! click, the Menu key and Shift+F10 on the row. It is drawn while the pointer is over the
//! row, while the row holds the keyboard focus and while the menu is open.

use std::rc::Rc;

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce,
    Role, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use super::{focus_ring, icon, tooltip};
use crate::{
    assets::Icon,
    text,
    theme::{theme, u},
};

/// The name of the hover group that a row with a menu button gives itself.
pub const MENU_ROW: &str = "menu-row";

type OpenHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// A control placed on a row or on another clickable element handles its press alone: the
/// press does not reach the row, so it does not select the row, open its place, start a drag
/// of the row, move the keyboard focus to the row or start a text selection.
pub trait OwnPress: InteractiveElement + Sized {
    fn own_press(self) -> Self {
        self.on_mouse_down(MouseButton::Left, |_, window, cx| {
            crate::selectable::claim_press();
            window.prevent_default();
            cx.stop_propagation();
        })
    }

    /// The same for a control that takes the keyboard focus at a press: gpui gives the focus
    /// after the listeners of the press, which the stop would skip.
    fn own_press_focusing(self, focus: &gpui::FocusHandle) -> Self {
        let focus = focus.clone();
        self.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            crate::selectable::claim_press();
            window.focus(&focus, cx);
            window.prevent_default();
            cx.stop_propagation();
        })
    }
}

impl<T: InteractiveElement> OwnPress for T {}

#[derive(IntoElement)]
pub struct MenuButton {
    id: ElementId,
    label: SharedString,
    shown: bool,
    dark: bool,
    ground: Option<gpui::Rgba>,
    selector: Option<SharedString>,
    on_open: Option<OpenHandler>,
}

impl MenuButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: text::MORE_ACTIONS.into(),
            shown: false,
            dark: false,
            ground: None,
            selector: None,
            on_open: None,
        }
    }

    /// The accessible label and the tooltip, such as "More actions".
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Drawn without the pointer over the row: the row holds the keyboard focus or its menu
    /// is open.
    pub fn shown(mut self, shown: bool) -> Self {
        self.shown = shown;
        self
    }

    /// The button on the basalt of the server book.
    pub fn dark(mut self) -> Self {
        self.dark = true;
        self
    }

    /// The ground under the button, where it lies over what its row shows at its place.
    pub fn ground(mut self, colour: gpui::Rgba) -> Self {
        self.ground = Some(colour);
        self
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn on_open(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for MenuButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let (ink, hover, ring) = if self.dark {
            (colors.bone_label, colors.basalt_3, colors.ochre)
        } else {
            (colors.ink_2, colors.limestone, colors.terracotta_deep)
        };
        let ring = focus_ring(ring, theme.metrics.focus_ring);
        let label = self.label;
        #[cfg(feature = "test-support")]
        let placed = super::placed::placed(
            self.selector
                .clone()
                .unwrap_or_else(|| format!("{}", self.id).into()),
            Some(label.clone()),
            None,
            false,
        );
        let frame = div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(label.clone())
            .when_some(self.selector, |frame, selector| {
                frame.debug_selector(move || selector.to_string())
            })
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(u(22.0))
            .h(u(22.0))
            .rounded(u(theme.metrics.radius))
            .when_some(self.ground, |frame, ground| frame.bg(Hsla::from(ground)))
            .cursor_pointer()
            .hover(move |style| style.bg(Hsla::from(hover)))
            .focus_visible(move |style| style.shadow(ring))
            .when(!self.shown, |frame| {
                frame
                    .invisible()
                    .group_hover(MENU_ROW, |style| style.visible())
            })
            .tooltip(tooltip(label))
            .own_press()
            .when_some(self.on_open, |frame, open| {
                frame.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    open(window, cx);
                })
            })
            .child(icon(Icon::Dots, 16.0, ink));
        #[cfg(feature = "test-support")]
        let frame = frame.child(placed);
        frame
    }
}
