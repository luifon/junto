//! The menu: a floating list of actions. It takes the keyboard focus while it is open: the
//! arrow keys and Tab move between its items, Enter and Space choose one, and Escape closes
//! it. A click outside closes it.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Pixels, Point, RenderOnce, Role, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_base::FocusTrapElement as _;

use super::{StyleExt, float_shadow, focus_ring_inside, icon, move_focus_inside};
use crate::assets::Icon;
use crate::theme::{theme, u};

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;
type ClickHandler = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The pointer and keyboard triggers of a target's context menu.
pub trait ContextMenuExt: InteractiveElement {
    fn on_context_menu(self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        let handler = std::rc::Rc::new(handler);
        let by_click = handler.clone();
        self.on_mouse_down(MouseButton::Right, move |_, window, cx| {
            cx.stop_propagation();
            by_click(window, cx);
        })
        .on_key_down(move |event, window, cx| {
            let keys = &event.keystroke;
            if context_menu_key(keys) {
                cx.stop_propagation();
                handler(window, cx);
            }
        })
    }
}

pub(super) fn context_menu_key(keys: &gpui::Keystroke) -> bool {
    keys.key == "menu" || (keys.key == "f10" && keys.modifiers.shift)
}

impl<T: InteractiveElement> ContextMenuExt for T {}

#[derive(Default)]
struct PaintedMenus(Vec<(gpui::WindowId, gpui::WeakFocusHandle, gpui::Bounds<Pixels>)>);
impl gpui::Global for PaintedMenus {}

fn over_child_menu(parent: &FocusHandle, at: Point<Pixels>, window: &Window, cx: &App) -> bool {
    cx.try_global::<PaintedMenus>().is_some_and(|menus| {
        menus.0.iter().any(|(id, focus, bounds)| {
            *id == window.window_handle().window_id()
                && bounds.contains(&at)
                && focus.upgrade().is_some_and(|focus| {
                    focus != *parent
                        && parent.contains(&focus, window)
                        && focus.contains_focused(window, cx)
                })
        })
    })
}

pub(crate) fn has_focused_menu(window: &Window, cx: &App) -> bool {
    cx.try_global::<PaintedMenus>().is_some_and(|menus| {
        menus.0.iter().any(|(id, focus, _)| {
            *id == window.window_handle().window_id()
                && focus
                    .upgrade()
                    .is_some_and(|focus| focus.contains_focused(window, cx))
        })
    })
}

pub(crate) fn over_focused_menu(at: Point<Pixels>, window: &Window, cx: &App) -> bool {
    cx.try_global::<PaintedMenus>().is_some_and(|menus| {
        menus.0.iter().any(|(id, focus, bounds)| {
            *id == window.window_handle().window_id()
                && bounds.contains(&at)
                && focus
                    .upgrade()
                    .is_some_and(|focus| focus.contains_focused(window, cx))
        })
    })
}

#[derive(Default)]
pub struct MenuPlacement(std::cell::Cell<Option<(Point<Pixels>, gpui::Size<Pixels>)>>);

#[derive(IntoElement)]
struct StableMenu {
    menu: Menu,
    anchor: Point<Pixels>,
    absolute: bool,
    placement: std::rc::Rc<MenuPlacement>,
}

impl RenderOnce for StableMenu {
    fn render(mut self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let viewport = window.viewport_size();
        let origin = self
            .placement
            .0
            .get()
            .filter(|(_, size)| *size == viewport)
            .map(|(origin, _)| origin);
        if origin.is_none() {
            self.placement.0.set(None);
        }
        self.menu.placement = Some(self.placement);
        match origin {
            Some(origin) => {
                let margin = u(8.0).to_pixels(window.rem_size());
                self.menu.height_limit =
                    Some((viewport.height - origin.y - margin).max(gpui::px(0.0)));
                self.menu.popup_at(origin)
            }
            None if self.absolute => self.menu.popup_at(self.anchor),
            None => self.menu.popup(self.anchor),
        }
    }
}

#[derive(IntoElement)]
pub struct Menu {
    id: ElementId,
    label: SharedString,
    focus: FocusHandle,
    dark: bool,
    width: Option<f32>,
    children: Vec<AnyElement>,
    header: Option<AnyElement>,
    selector: Option<&'static str>,
    on_close: Option<CloseHandler>,
    placement: Option<std::rc::Rc<MenuPlacement>>,
    height_limit: Option<Pixels>,
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
            header: None,
            selector: None,
            on_close: None,
            placement: None,
            height_limit: None,
        }
    }

    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
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

    /// Floats beside its target, within the window, above scrolled content.
    pub fn popup(self, offset: Point<Pixels>) -> AnyElement {
        self.floating(gpui::anchored().offset(offset))
    }

    /// Floats at a position in window coordinates, such as an input's caret.
    pub fn popup_at(self, position: Point<Pixels>) -> AnyElement {
        self.floating(gpui::anchored().position(position))
    }

    /// Keeps the first painted origin while contents change. Supply a fresh placement for each opening.
    pub fn popup_stable(
        self,
        offset: Point<Pixels>,
        placement: std::rc::Rc<MenuPlacement>,
    ) -> AnyElement {
        StableMenu {
            menu: self,
            anchor: offset,
            absolute: false,
            placement,
        }
        .into_any_element()
    }

    /// Keeps the first painted origin of a menu opened at window coordinates.
    pub fn popup_at_stable(
        self,
        position: Point<Pixels>,
        placement: std::rc::Rc<MenuPlacement>,
    ) -> AnyElement {
        StableMenu {
            menu: self,
            anchor: position,
            absolute: true,
            placement,
        }
        .into_any_element()
    }

    fn floating(self, anchor: gpui::Anchored) -> AnyElement {
        gpui::deferred(anchor.snap_to_window_with_margin(gpui::px(8.0)).child(self))
            .with_priority(1)
            .into_any_element()
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let metrics = theme.metrics;
        let close = self.on_close.map(std::rc::Rc::new);
        let (escape, outside, wheel) = (close.clone(), close.clone(), close);
        let items =
            super::reveal::inner_handle(&self.id, &"menu-items".into(), window, cx).tab_stop(false);
        let layer = items.clone();
        let outside_focus = self.focus.clone();
        let painted_focus = self.focus.clone();
        let margin = u(8.0).to_pixels(window.rem_size());
        let available = window.viewport_size();
        let scroll = window
            .use_keyed_state((self.id.clone(), "scroll"), cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        let revealed = window
            .use_keyed_state((self.id.clone(), "revealed"), cx, |_, _| {
                std::rc::Rc::new(std::cell::RefCell::new(super::reveal::Revealed::default()))
            })
            .read(cx)
            .clone();
        let scrolling = scroll.clone();
        let max_height = (available.height - margin * 2.0).max(gpui::px(0.0));
        let max_height = self
            .height_limit
            .map_or(max_height, |limit| limit.min(max_height));
        let placement = self.placement;
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
            .max_w((available.width - margin * 2.0).max(gpui::px(0.0)))
            .max_h(max_height)
            .overflow_y_scroll()
            .track_scroll(&scroll)
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
                if !over_child_menu(&outside_focus, window.mouse_position(), window, cx)
                    && let Some(close) = &outside
                {
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
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| {
                        let id = window.window_handle().window_id();
                        let menus = cx.default_global::<PaintedMenus>();
                        menus.0.retain(|(_, focus, _)| focus.upgrade().is_some());
                        if let Some((_, _, bounds)) = menus.0.iter_mut().find(|(held, focus, _)| {
                            *held == id && focus.upgrade().as_ref() == Some(&painted_focus)
                        }) {
                            *bounds = scrolling.bounds();
                        } else {
                            menus
                                .0
                                .push((id, painted_focus.downgrade(), scrolling.bounds()));
                        }
                        if let Some(placement) = &placement
                            && placement.0.get().is_none()
                        {
                            placement
                                .0
                                .set(Some((scrolling.bounds().origin, window.viewport_size())));
                        }
                        let (wheel, scrolling) = (wheel.clone(), scrolling.clone());
                        let parent = painted_focus.clone();
                        window.on_mouse_event(
                            move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                                if phase == gpui::DispatchPhase::Capture
                                    && !scrolling.bounds().contains(&event.position)
                                    && !over_child_menu(&parent, event.position, window, cx)
                                    && let Some(close) = &wheel
                                {
                                    close(window, cx);
                                }
                            },
                        );
                    },
                )
                .absolute()
                .size_0(),
            )
            .child(super::reveal::start())
            .children(self.header.map(|header| div().flex_none().child(header)))
            .child(
                div()
                    .id("menu-items")
                    .track_focus(&items)
                    .flex()
                    .flex_col()
                    .flex_none()
                    .children(self.children),
            )
            .child(super::reveal::end(&scroll, revealed))
            .focus_trap("menu-trap", &self.focus)
    }
}

/// One action of a menu on stone.
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
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }
}

impl IntoElement for MenuItem {
    type Element = gpui::ViewElement<Self>;
    fn into_element(self) -> Self::Element {
        super::action_menu::record(|| {
            if self.on_click.is_none() && !self.disabled {
                return None;
            }
            Some(super::RowAction {
                label: self.title.clone(),
                icon: self.icon,
                disabled: self.disabled,
                detail: self.detail.clone(),
                handler: self.on_click.clone(),
            })
        });
        gpui::ViewElement::new(self)
    }
}

impl RenderOnce for MenuItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = super::reveal::handle(&self.id, window, cx);
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        div()
            .id(self.id)
            .role(Role::MenuItem)
            .aria_label(self.title.clone())
            .aria_disabled(self.disabled)
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
            .relative()
            .track_focus(&focus)
            .children(super::reveal::report(&focus, window))
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
pub fn menu_label(text: impl Into<SharedString>, dark: bool, cx: &App) -> gpui::Div {
    let text: SharedString = text.into();
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
        .min_w_0()
        .child(div().truncate().child(text))
}

/// A line between two groups of items of a menu.
pub fn menu_separator(cx: &App) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .h(gpui::px(1.0))
        .my(u(4.0))
        .bg(Hsla::from(theme(cx).colors.rule_soft))
}

#[cfg(test)]
mod tests;
