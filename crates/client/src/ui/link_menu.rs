use std::ops::Range;

use super::{Menu, MenuItem};
use crate::text;
use gpui::{
    AnyElement, App, Context, ElementId, FocusHandle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, Render, RenderOnce, StatefulInteractiveElement, Styled,
    TextLayout, WeakFocusHandle, Window, div, prelude::FluentBuilder,
};
use junto_client_core::link::Link;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ui, no_json, no_register)]
pub struct CopyToClipboard {
    pub text: String,
}

#[derive(IntoElement)]
pub struct LinkMenu {
    id: ElementId,
    layout: Option<TextLayout>,
    links: Vec<(Range<usize>, Link)>,
    focus: Option<FocusHandle>,
    child: AnyElement,
}

impl LinkMenu {
    pub fn text(
        id: impl Into<ElementId>,
        layout: TextLayout,
        links: Vec<(Range<usize>, Link)>,
        child: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            layout: Some(layout),
            links,
            focus: None,
            child: child.into_any_element(),
        }
    }
    pub fn control(id: impl Into<ElementId>, link: Link, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            layout: None,
            links: vec![(0..usize::MAX, link)],
            focus: None,
            child: child.into_any_element(),
        }
    }
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

impl RenderOnce for LinkMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let source_focus = self.focus;
        let popup = window.use_keyed_state((self.id.clone(), "link-menu"), cx, |_, cx| Popup {
            focus: cx.focus_handle(),
            source: source_focus.unwrap_or_else(|| cx.focus_handle().tab_stop(true)),
            return_focus: None,
            current: Vec::new(),
            shown: None,
            position: Point::default(),
        });
        let current: Vec<_> = self.links.iter().map(|(_, link)| link.clone()).collect();
        popup.update(cx, |popup, cx| {
            if popup.current != current {
                popup.current = current;
                if popup.shown.is_some() {
                    popup.close(window, cx);
                }
            }
        });
        let source = popup.read(cx).source.clone();
        let (by_mouse, by_key) = (popup.clone(), popup.clone());
        let text = self.layout.is_some();
        let (mouse_layout, key_layout) = (self.layout.clone(), self.layout);
        let (mouse_links, key_links) = (self.links.clone(), self.links);
        let look = crate::theme::theme(cx);
        let ring = super::focus_ring_inside(look.colors.terracotta_deep, look.metrics.focus_ring);
        div()
            .id(self.id)
            .relative()
            .min_w_0()
            .when(text, |frame| {
                frame
                    .role(gpui::Role::Group)
                    .aria_label(text::LINK_ACTIONS)
                    .track_focus(&source)
                    .focus_visible(move |style| style.shadow(ring))
            })
            .capture_any_mouse_down(move |event, window, cx| {
                if event.button != MouseButton::Right {
                    return;
                }
                let selected = match &mouse_layout {
                    Some(layout) => layout
                        .index_for_position(event.position)
                        .ok()
                        .and_then(|index| {
                            mouse_links.iter().find(|(range, _)| range.contains(&index))
                        })
                        .map(|(_, link)| link.clone()),
                    None => mouse_links.first().map(|(_, link)| link.clone()),
                };
                if let Some(link) = selected {
                    cx.stop_propagation();
                    by_mouse.update(cx, |popup, cx| {
                        popup.open(vec![link], event.position, window, cx)
                    });
                }
            })
            .on_key_down(move |event, window, cx| {
                if !super::menu::context_menu_key(&event.keystroke) {
                    return;
                }
                cx.stop_propagation();
                let position = key_layout
                    .as_ref()
                    .and_then(|layout| layout.position_for_index(0))
                    .unwrap_or_else(|| window.mouse_position());
                let links = key_links.iter().map(|(_, link)| link.clone()).collect();
                by_key.update(cx, |popup, cx| popup.open(links, position, window, cx));
            })
            .child(self.child)
            .child(popup)
    }
}

struct Popup {
    focus: FocusHandle,
    source: FocusHandle,
    return_focus: Option<WeakFocusHandle>,
    current: Vec<Link>,
    shown: Option<Vec<Link>>,
    position: Point<Pixels>,
}

impl Popup {
    fn open(
        &mut self,
        links: Vec<Link>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if links.is_empty() || links.iter().any(|link| !self.current.contains(link)) {
            return;
        }
        if self.shown.is_none() {
            self.return_focus = window.focused(cx).map(|focus| focus.downgrade());
        }
        self.shown = Some(links);
        self.position = position;
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = None;
        if let Some(focus) = self.return_focus.take().and_then(|focus| focus.upgrade()) {
            focus.focus(window, cx);
        }
        cx.notify();
    }
    fn choose(&mut self, link: &Link, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let current = self.current.contains(link)
            && self
                .shown
                .as_ref()
                .is_some_and(|links| links.contains(link));
        self.close(window, cx);
        current
    }
}

impl Render for Popup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(links) = self.shown.clone() else {
            return div().absolute().size_0().into_any_element();
        };
        let close = cx.listener(|this, _: &(), window, cx| this.close(window, cx));
        let mut items = Vec::new();
        for (index, link) in links.into_iter().enumerate() {
            items.push(super::menu_label(link.as_str().to_owned(), false, cx).into_any_element());
            let (opened, copied) = (link.clone(), link);
            items.push(
                div()
                    .debug_selector(move || format!("link-open:{index}"))
                    .child(
                        MenuItem::new(("link-open", index), text::OPEN_LINK).on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.choose(&opened, window, cx) {
                                    crate::opening::open_link(&opened, cx);
                                }
                            },
                        )),
                    )
                    .into_any_element(),
            );
            items.push(
                div()
                    .debug_selector(move || format!("link-copy:{index}"))
                    .child(
                        MenuItem::new(("link-copy", index), text::COPY_TARGET_LINK).on_click(
                            cx.listener(move |this, _, window, cx| {
                                if this.choose(&copied, window, cx) {
                                    window.dispatch_action(
                                        Box::new(CopyToClipboard {
                                            text: copied.as_str().to_owned(),
                                        }),
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )
                    .into_any_element(),
            );
        }
        Menu::new("link-menu", text::LINK_ACTIONS, &self.focus)
            .selector("link-menu")
            .on_close(move |window, cx| close(&(), window, cx))
            .children(items)
            .popup_at(self.position)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
