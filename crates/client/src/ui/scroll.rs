//! A list that scrolls, with the thin scroll bar of the design at its right edge.

use gpui::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use gpui_base::{Scrollbar, ScrollbarMode};

use crate::theme::theme;

#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    handle: ScrollHandle,
    label: SharedString,
    dark: bool,
    selector: Option<&'static str>,
    children: Vec<AnyElement>,
}

impl ScrollArea {
    /// A list that keeps its position in `handle`. `label` names the list in the
    /// accessibility tree.
    pub fn new(
        id: impl Into<ElementId>,
        handle: &ScrollHandle,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            handle: handle.clone(),
            label: label.into(),
            dark: false,
            selector: None,
            children: Vec::new(),
        }
    }

    /// A list of the server book, on basalt.
    pub fn dark(mut self) -> Self {
        self.dark = true;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = theme(cx).colors;
        let thumb = Hsla::from(if self.dark {
            colors.basalt_rule
        } else {
            colors.rule
        });
        let held = Hsla::from(if self.dark {
            colors.bone_3
        } else {
            colors.ink_3
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id(self.id)
                    .role(Role::List)
                    .aria_label(self.label)
                    .when_some(self.selector, |list, selector| {
                        list.debug_selector(|| selector.to_owned())
                    })
                    .flex()
                    .flex_col()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.handle)
                    .children(self.children),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&self.handle)
                        .viewport_from_layout()
                        .mode(ScrollbarMode::Always)
                        .styles(|styles| {
                            styles
                                .track(|track| track.bg(gpui::transparent_black()).width(px(10.0)))
                                .track_hover(|track| track.bg(gpui::transparent_black()))
                                .track_active(|track| track.bg(gpui::transparent_black()))
                                .thumb(|bar| {
                                    bar.bg(thumb).width(px(4.0)).radius(px(2.0)).inset(px(3.0))
                                })
                                .thumb_hover(|bar| bar.bg(held))
                                .thumb_active(|bar| bar.bg(held))
                        }),
                ),
            )
    }
}
