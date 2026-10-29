use gpui::{
    AnyElement, ClipboardItem, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, Window, canvas, div, px,
};

use super::Shell;
use crate::{
    selectable::SelectableText,
    text,
    theme::{theme, u},
    ui::{Button, StyleExt},
};

impl Shell {
    pub(crate) fn selection_context(&self) {
        self.text_selection.scope(format!(
            "{:?}:{:?}:{:?}:{:?}",
            self.view
                .as_ref()
                .and_then(|view| view.selected.as_ref().map(|server| server.id)),
            self.view.as_ref().and_then(|view| view.person.account_id),
            self.view
                .as_ref()
                .and_then(|view| view.lists()?.feed.as_ref().map(|feed| feed.place)),
            std::mem::discriminant(&self.layer)
        ));
    }
    pub(crate) fn copy_text(&mut self, value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied_text = self.copied_text.wrapping_add(1);
        self.copied_shown = true;
        let number = self.copied_text;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_text == number {
                    this.copied_shown = false;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_copy_control(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = theme(cx);
        let mut controls = Vec::new();
        self.text_selection.control(None);
        if let Some((value, bounds)) = self.text_selection.hovered() {
            let selection = self.text_selection.clone();
            let left = bounds
                .right()
                .min(window.viewport_size().width - u(104.0).to_pixels(window.rem_size()))
                .max(px(0.0));
            let top = bounds
                .bottom()
                .min(window.viewport_size().height - u(34.0).to_pixels(window.rem_size()))
                .max(px(0.0));
            controls.push(
                SelectableText::unobserved(
                    div()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .absolute()
                        .left(left)
                        .top(top)
                        .child(
                            canvas(
                                move |bounds, _, _| selection.control(Some(bounds)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .child(
                            Button::new("copy-text-value")
                                .label(text::COPY_TEXT)
                                .selector("copy-text-value")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy_text(value.clone(), cx);
                                })),
                        ),
                )
                .into_any_element(),
            );
        }
        if self.copied_shown {
            controls.push(
                SelectableText::unobserved(
                    div()
                        .absolute()
                        .bottom(u(18.0))
                        .right(u(18.0))
                        .px(u(12.0))
                        .py(u(6.0))
                        .bg(theme.colors.basalt)
                        .ink(theme.colors.bone)
                        .rounded(u(4.0))
                        .debug_selector(|| "copied-confirmation".to_owned())
                        .child(text::account::COPIED),
                )
                .into_any_element(),
            );
        }
        controls
    }
}
