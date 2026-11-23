use gpui::{
    AnyElement, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement, Styled, div,
};

use super::Shell;
use crate::{
    selectable::SelectableText,
    text,
    theme::{theme, u},
    ui::{Button, ButtonKind, StyleExt},
};

impl Shell {
    pub(crate) fn selection_context(&self) {
        self.text_selection.scope(format!(
            "{:?}:{:?}:{:?}:{:?}:{:?}",
            self.view
                .as_ref()
                .and_then(|view| view.selected.as_ref().map(|server| server.id)),
            self.view.as_ref().and_then(|view| view.person.account_id),
            self.view
                .as_ref()
                .and_then(|view| view.lists()?.feed.as_ref().map(|feed| feed.place)),
            self.view.as_ref().and_then(|view| view
                .workspace()?
                .thread
                .as_ref()
                .map(|thread| thread.root_id)),
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

    /// The button beside a value that a person copies to use elsewhere (ADR 0731): an icon
    /// with its tooltip, which copies the value and shows that it was copied.
    pub(crate) fn value_copy_button(
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .kind(ButtonKind::Ghost)
            .icon(crate::assets::Icon::Copy)
            .tooltip(label)
            .on_click(cx.listener(move |this, _, _, cx| this.copy_text(value.clone(), cx)))
    }

    pub(crate) fn render_copied_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = theme(cx);
        self.copied_shown.then(|| {
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
            .into_any_element()
        })
    }
}
