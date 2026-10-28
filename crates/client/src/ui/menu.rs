//! The menu: a floating list of actions. It takes the keyboard focus while it is open: the
//! arrow keys and Tab move between its items, Enter and Space choose one, and Escape closes
//! it. A click outside closes it.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, RenderOnce, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_base::FocusTrapElement as _;

use super::{StyleExt, float_shadow, focus_ring_inside, icon, move_focus_inside};
use crate::assets::Icon;
use crate::theme::{theme, u};

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Menu {
    id: ElementId,
    label: SharedString,
    focus: FocusHandle,
    dark: bool,
    width: Option<f32>,
    children: Vec<AnyElement>,
    selector: Option<&'static str>,
    on_close: Option<CloseHandler>,
}

impl Menu {
    /// A menu that holds the keyboard focus through `focus`. `label` names it in the
    /// accessibility tree.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        focus: &FocusHandle,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            focus: focus.clone(),
            dark: false,
            width: None,
            children: Vec::new(),
            selector: None,
            on_close: None,
        }
    }

    /// The menu of the server book, on basalt.
    pub fn dark(mut self) -> Self {
        self.dark = true;
        self
    }

    /// The width in design pixels.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    /// Runs when Escape is pressed in the menu and when a click lands outside it.
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Menu {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Menu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let metrics = theme.metrics;
        let close = self.on_close.map(std::rc::Rc::new);
        let (escape, outside) = (close.clone(), close);
        let layer = self.focus.clone();
        let frame = div()
            .id(self.id)
            .role(Role::Menu)
            .aria_label(self.label)
            .when_some(self.selector, |menu, selector| {
                menu.debug_selector(|| selector.to_owned())
            })
            .occlude()
            .relative()
            // A menu is a layer: the areas that it lies over are not measured (ADR 0730).
            .child(super::cover())
            .flex()
            .flex_col()
            .w(u(self.width.unwrap_or(if self.dark {
                metrics.fly_width
            } else {
                metrics.menu_width
            })))
            .overflow_hidden()
            .border_1()
            .typeface(theme.types.control, &theme)
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "down" => move_focus_inside(&layer, true, window, cx),
                    "up" => move_focus_inside(&layer, false, window, cx),
                    "escape" => {
                        if let Some(close) = &escape {
                            close(window, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            })
            .on_mouse_down_out(move |_, window, cx| {
                if let Some(close) = &outside {
                    close(window, cx);
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let frame = if self.dark {
            frame
                .pb(u(6.0))
                .rounded(u(metrics.radius_dialog))
                .bg(Hsla::from(colors.basalt_2))
                .border_color(Hsla::from(colors.basalt_float_rule))
                .shadow(float_shadow(colors.shadow_deep, 10.0, 28.0))
                .ink(colors.bone)
        } else {
            frame
                .pt(u(4.0))
                .rounded(u(metrics.radius_menu))
                .bg(Hsla::from(colors.travertine_hi))
                .border_color(Hsla::from(colors.rule))
                .shadow(float_shadow(colors.shadow_deep, 8.0, 22.0))
                .ink(colors.ink)
        };
        frame
            .children(self.children)
            .focus_trap("menu-trap", &self.focus)
    }
}

/// One action of a menu on stone.
#[derive(IntoElement)]
pub struct MenuItem {
    id: ElementId,
    title: SharedString,
    detail: Option<SharedString>,
    detail_icon: Option<Icon>,
    icon: Option<Icon>,
    swatch: Option<Hsla>,
    end_icon: Option<Icon>,
    trailing: Option<SharedString>,
    disabled: bool,
    selector: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl MenuItem {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            detail_icon: None,
            icon: None,
            swatch: None,
            end_icon: None,
            trailing: None,
            disabled: false,
            selector: None,
            on_click: None,
        }
    }

    /// An item that does nothing now. It stays a stop of the keyboard, so that the focus
    /// does not leave an item that an action disabled; its second line says why.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A square of a colour in the place of the icon, for the choice of a colour.
    pub fn swatch(mut self, colour: Hsla) -> Self {
        self.swatch = Some(colour);
        self
    }

    /// A second line under the title.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A small icon before the second line, such as the key of a permission.
    pub fn detail_icon(mut self, icon: Icon) -> Self {
        self.detail_icon = Some(icon);
        self
    }

    /// An icon at the end of the item, such as the mark of an item that opens a list.
    pub fn end_icon(mut self, icon: Icon) -> Self {
        self.end_icon = Some(icon);
        self
    }

    /// A short state at the end of the item, such as "On".
    pub fn trailing(mut self, text: impl Into<SharedString>) -> Self {
        self.trailing = Some(text.into());
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for MenuItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        div()
            .id(self.id)
            .role(Role::MenuItem)
            .aria_label(self.title.clone())
            .when_some(self.selector, |item, selector| {
                item.debug_selector(|| selector.to_owned())
            })
            .flex()
            .flex_none()
            .items_start()
            .gap(u(9.0))
            .py(u(5.0))
            .px(u(12.0))
            .line_height(u(19.0))
            .focusable()
            .tab_stop(true)
            .focus(move |style| style.bg(Hsla::from(colors.limestone_lo)))
            .focus_visible(move |style| style.shadow(ring))
            .when(self.disabled, |item| item.cursor_not_allowed())
            .when(!self.disabled, |item| {
                item.cursor_pointer()
                    .hover(move |style| style.bg(Hsla::from(colors.limestone_lo)))
                    .when_some(self.on_click, |item, handler| {
                        item.on_click(move |event, window, cx| handler(event, window, cx))
                    })
            })
            .when_some(self.icon, |item, shown| {
                item.child(div().mt(u(2.0)).child(icon(shown, 15.0, colors.ink_2)))
            })
            .when_some(self.swatch, |item, colour| {
                item.child(
                    div()
                        .flex_none()
                        .mt(u(3.0))
                        .size(u(13.0))
                        .rounded(u(3.0))
                        .border_1()
                        .border_color(Hsla::from(colors.rule))
                        .bg(colour),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .when(self.disabled, |title| title.ink(colors.ink_3))
                            .child(self.title),
                    )
                    .when_some(self.detail, |column, detail| {
                        column.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(u(4.0))
                                .typeface(theme.types.small, &theme)
                                .ink(colors.ink_2)
                                .when_some(self.detail_icon, |line, shown| {
                                    line.child(icon(shown, 12.0, colors.bronze))
                                })
                                // A detail longer than the menu is wide wraps.
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .when_some(self.selector, |text, selector| {
                                            text.debug_selector(move || {
                                                format!("{selector}-detail")
                                            })
                                        })
                                        .child(detail),
                                ),
                        )
                    }),
            )
            .when_some(self.end_icon, |item, shown| {
                item.child(
                    div()
                        .flex_none()
                        .mt(u(2.0))
                        .child(icon(shown, 15.0, colors.ink_2)),
                )
            })
            .when_some(self.trailing, |item, text| {
                item.child(
                    div()
                        .flex_none()
                        .typeface(theme.types.small_strong, &theme)
                        .line_height(u(19.0))
                        .ink(colors.ink_2)
                        .child(text),
                )
            })
    }
}

/// A label above a group of items of a menu.
pub fn menu_label(text: &'static str, dark: bool, cx: &App) -> impl IntoElement + use<> {
    let theme = theme(cx);
    div()
        .flex_none()
        .px(u(12.0))
        .pt(u(if dark { 8.0 } else { 4.0 }))
        .pb(u(if dark { 3.0 } else { 1.0 }))
        .typeface(theme.types.small_strong, &theme)
        .ink(if dark {
            theme.colors.bone_label
        } else {
            theme.colors.bronze
        })
        .child(text)
}

/// A line between two groups of items of a menu.
pub fn menu_separator(cx: &App) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .h(gpui::px(1.0))
        .my(u(4.0))
        .bg(Hsla::from(theme(cx).colors.rule_soft))
}
