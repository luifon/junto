//! The dialog: a panel above the window content, with a flat cover behind it. It holds the
//! keyboard focus while it is open, Escape closes it, and its owner returns the focus to
//! where it was. A dialog is never taller than the window: its header stays, and its
//! content scrolls, and a control of the content that takes the keyboard focus outside
//! the view is scrolled into it.

use gpui::{
    AnyElement, App, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, RenderOnce, Role, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder,
};
use gpui_base::FocusTrapElement as _;

use super::reveal;
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
        // The control of the content that the dialog brought into view last.
        let revealed = window
            .use_keyed_state("dialog-content-revealed", cx, |_, _| {
                std::rc::Rc::new(std::cell::RefCell::new(reveal::Revealed::default()))
            })
            .read(cx)
            .clone();
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
                            .debug_selector(|| "dialog-content".to_owned())
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .pt(u(14.0))
                            .px(u(16.0))
                            .pb(u(16.0))
                            // Tab to a control outside the view scrolls it into view.
                            .child(reveal::start())
                            .children(self.children)
                            .child(reveal::end(&scroll, revealed)),
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

#[cfg(test)]
mod tests {
    use gpui::{
        Context, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render,
        Styled, TestAppContext, VisualTestContext, Window, div, px,
    };

    use super::Dialog;
    use crate::ui::Button;

    gpui::actions!(dialog_test, [Next, Previous]);

    const BUTTONS: usize = 40;

    struct Host {
        trap: FocusHandle,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let buttons = (0..BUTTONS).map(|index| {
                let selector: &'static str = Box::leak(format!("b{index}").into_boxed_str());
                div()
                    .py(px(4.0))
                    .child(Button::new(("b", index)).label("Choose").selector(selector))
            });
            div()
                .key_context("TestRoot")
                .on_action(cx.listener(|_, _: &Next, window, cx| window.focus_next(cx)))
                .on_action(cx.listener(|_, _: &Previous, window, cx| window.focus_prev(cx)))
                .relative()
                .w(px(600.0))
                .h(px(400.0))
                .child(Dialog::new("Settings", &self.trap).children(buttons))
        }
    }

    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..50 {
            cx.run_until_parked();
            if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
                cx.run_until_parked();
                return;
            }
        }
        panic!("the window asks for frames without end");
    }

    /// The bounds of the content of the dialog and of button `index`, when the button is
    /// drawn whole in the view of the content.
    fn in_view(cx: &mut VisualTestContext, index: usize) -> Result<(), String> {
        let view = cx.debug_bounds("dialog-content").unwrap();
        let selector: &'static str = Box::leak(format!("b{index}").into_boxed_str());
        match cx.debug_bounds(selector) {
            Some(button) if button.top() >= view.top() && button.bottom() <= view.bottom() => {
                Ok(())
            }
            button => Err(format!("{button:?} in {view:?}")),
        }
    }

    /// Tab and Shift+Tab through a dialog taller than its place scroll each focused control
    /// into the view of the content.
    #[gpui::test]
    fn tab_in_a_tall_dialog_shows_the_focused_control(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::theme::install(crate::theme::Theme::stone(), cx);
            cx.bind_keys([
                KeyBinding::new("tab", Next, Some("TestRoot")),
                KeyBinding::new("shift-tab", Previous, Some("TestRoot")),
            ]);
        });
        let (host, cx) = cx.add_window_view(|_, cx| Host {
            trap: cx.focus_handle(),
        });
        settle(cx);
        assert!(
            in_view(cx, BUTTONS - 1).is_err(),
            "the dialog does not scroll"
        );
        let trap = host.read_with(cx, |host, _| host.trap.clone());
        cx.update(|window, cx| window.focus(&trap, cx));
        settle(cx);
        for index in 0..BUTTONS {
            cx.simulate_keystrokes("tab");
            settle(cx);
            in_view(cx, index).unwrap_or_else(|bounds| panic!("tab to button {index}: {bounds}"));
        }
        for index in (0..BUTTONS - 1).rev() {
            cx.simulate_keystrokes("shift-tab");
            settle(cx);
            in_view(cx, index)
                .unwrap_or_else(|bounds| panic!("shift-tab to button {index}: {bounds}"));
        }
    }
}
