use gpui::Context;

use super::AppState;

impl AppState {
    pub fn recent_emoji(&self) -> &[String] {
        &self.recent_emoji
    }

    pub fn remember_emoji(&mut self, emoji: &str, cx: &mut Context<Self>) {
        self.remember_emoji_choices(std::iter::once(emoji), cx);
    }

    pub(crate) fn remember_emoji_choices<'a>(
        &mut self,
        choices: impl Iterator<Item = &'a str>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for emoji in choices {
            changed |= junto_client_core::emoji_history::remember(&mut self.recent_emoji, emoji);
        }
        if !changed && self.recent_emoji_error.is_none() {
            return;
        }
        self.recent_emoji_writes = self.recent_emoji_writes.wrapping_add(1);
        let number = self.recent_emoji_writes;
        let store = self.client.emoji_history();
        let wanted = self.recent_emoji.clone();
        Self::queue_write(
            cx,
            move || store.save(&wanted),
            move |this, result, cx| {
                if number == this.recent_emoji_writes {
                    this.recent_emoji_error = result.err().map(|error| {
                        crate::text::flow_error(&junto_client_core::flow::FlowError::Storage(
                            error.to_string(),
                        ))
                    });
                    cx.notify();
                }
            },
        );
        self.changed(cx);
    }
}
