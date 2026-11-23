use super::{Pane, Shell};
use crate::drafts::{SavedDraft, Token};
use gpui::{App, Context, WeakEntity};

#[derive(Default)]
struct Windows(Vec<WeakEntity<Shell>>);
impl gpui::Global for Windows {}
pub(super) fn register(shell: WeakEntity<Shell>, cx: &mut App) {
    let windows = &mut cx.default_global::<Windows>().0;
    windows.retain(|shell| shell.upgrade().is_some());
    windows.push(shell);
}
pub(crate) fn flush(cx: &mut App) {
    let shells = cx.default_global::<Windows>().0.clone();
    for shell in shells {
        let _ = shell.update(cx, |shell, cx| shell.save_composers(cx));
    }
}
impl Shell {
    pub(super) fn save_composers(&mut self, cx: &mut App) {
        self.typing_activity(Pane::Main, false, cx);
        self.typing_activity(Pane::Thread, false, cx);
        for pane in [Pane::Main, Pane::Thread] {
            self.save_composer(pane, cx);
            self.save_composer(super::Pen::Edit(pane), cx);
        }
        self.state.update(cx, |state, cx| state.flush_drafts(cx));
    }
    /// Keeps the text of an editor in the book of drafts: the new message of a composer, or
    /// the edit of the editor of a row, each in its own store (ADR 0740).
    pub(super) fn save_composer(&self, key: impl Into<super::Pen>, cx: &mut App) {
        let key: super::Pen = key.into();
        let pen = self.pen(key);
        let Some(desk) = pen.place else {
            return;
        };
        let Some(entry_generation) = self.state.read(cx).draft_entry_generation(desk.0) else {
            return;
        };
        if pen
            .saved
            .borrow()
            .as_ref()
            .is_some_and(|saved| saved.entry_generation != entry_generation)
        {
            return;
        }
        let content = pen.input.read(cx).content();
        let draft = SavedDraft {
            desk,
            entry_generation,
            text: content.text().to_string(),
            tokens: content
                .tokens()
                .iter()
                .map(|span| Token {
                    range: span.range(),
                    id: span.token().id().to_string(),
                    text: span.token().text().to_string(),
                    label: span.token().label().to_string(),
                })
                .collect(),
            reply: pen.reply_to,
            editing: pen.editing.as_ref().map(|editing| {
                (
                    editing.message_id,
                    editing.original.0.clone(),
                    editing.original.1.clone(),
                )
            }),
        };
        if pen.saved.borrow().as_ref() == Some(&draft) {
            return;
        }
        pen.saved.replace(Some(draft.clone()));
        if key.edits() {
            self.state
                .update(cx, |state, cx| state.keep_edit(draft, cx));
        } else {
            self.state
                .update(cx, |state, cx| state.keep_draft(draft, cx));
        }
    }
    pub(super) fn draft_mark(
        &self,
        server: junto_client_core::protocol::ids::ServerId,
        place: junto_client_core::model::PlaceId,
        cx: &Context<Self>,
    ) -> Vec<crate::ui::ChannelState> {
        let state = self.state.read(cx);
        let Some(account) = state.draft_account(server) else {
            return Vec::new();
        };
        if !state.has_draft((server, account, place)) {
            return Vec::new();
        }
        vec![crate::ui::ChannelState::Marked {
            icon: crate::assets::Icon::Edit,
            text: crate::text::DRAFT.into(),
            strong: false,
        }]
    }
}
