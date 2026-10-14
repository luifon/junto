//! The dialog: a panel above the window content, with a flat cover behind it. It holds the
//! keyboard focus while it is open, Escape closes it, and its owner returns the focus to
//! where it was. A dialog is never taller than the window: its header stays, and its
//! content scrolls.

use gpui::{
    AnyElement, App, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, RenderOnce, Role, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder,
};
use gpui_base::FocusTrapElement as _;

use super::scroll::scroll_bar;
use super::{Button, ButtonKind, SideHeader, float_shadow};
use crate::assets::Icon;
use crate::text;
use crate::theme::{theme, u};

type CloseHandler = std::rc::Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Dialog {
    title: SharedString,
    focus: FocusHandle,
    children: Vec<AnyElement>,
    selector: Option<&'static str>,
    on_close: Option<CloseHandler>,
    scroll: Option<ScrollHandle>,
}

impl Dialog {
    /// A dialog that holds the keyboard focus through `focus`.
    pub fn new(title: impl Into<SharedString>, focus: &FocusHandle) -> Self {
        Self {
            title: title.into(),
            focus: focus.clone(),
            children: Vec::new(),
            selector: None,
            on_close: None,
            scroll: None,
        }
    }

    /// Keeps the position of the content in `handle`, so that the owner of the dialog can
    /// show a part of it. Without this the dialog holds the position itself.
    pub fn scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    /// Makes the dialog closable: by its close button, by Escape and by a click on the
    /// cover. A dialog without this stays until its content ends it.
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(std::rc::Rc::new(handler));
        self
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let metrics = theme.metrics;
        // The position of the content, for as long as the dialog is open.
        let scroll = match self.scroll {
            Some(handle) => handle,
            None => window
                .use_keyed_state("dialog-content-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone(),
        };
        let (by_button, by_key, by_cover) =
            (self.on_close.clone(), self.on_close.clone(), self.on_close);
        let header = SideHeader::new(self.title.clone()).when_some(by_button, |header, close| {
            header.trailing(
                Button::new("dialog-close")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE)
                    .selector("dialog-close")
                    .on_click(move |_, window, cx| close(window, cx)),
            )
        });
        let panel = div()
            .id("dialog")
            .role(Role::Dialog)
            .aria_label(self.title)
            .when_some(self.selector, |panel, selector| {
                panel.debug_selector(|| selector.to_owned())
            })
            .w(u(metrics.dialog_width))
            .max_w_full()
            // A dialog that is taller than the window scrolls its content.
            .max_h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(u(metrics.radius_dialog))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine))
            .shadow(float_shadow(colors.shadow_deep, 14.0, 40.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && let Some(close) = &by_key
                {
                    close(window, cx);
                    cx.stop_propagation();
                }
            })
            .child(header)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(
                        div()
                            .id("dialog-content")
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .pt(u(14.0))
                            .px(u(16.0))
                            .pb(u(16.0))
                            .children(self.children),
                    )
                    .child(scroll_bar(&scroll, false, cx)),
            )
            .focus_trap("dialog-trap", &self.focus);
        div()
            .id("dialog-cover")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(u(16.0))
            .bg(Hsla::from(colors.scrim))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(close) = &by_cover {
                    close(window, cx);
                }
            })
            .child(panel)
    }
}

/// The row of buttons at the end of a dialog.
pub fn dialog_footer(buttons: impl IntoIterator<Item = AnyElement>) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .justify_end()
        .gap(u(8.0))
        .mt(u(14.0))
        .children(buttons)
}
