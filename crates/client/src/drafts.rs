use gpui_base::input::{InlineToken, InputContent};
use junto_client_core::{
    model::PlaceId,
    protocol::{
        ids::{AccountId, MessageId, ServerId},
        message::Document,
    },
};
use serde::{Deserialize, Serialize};
pub(crate) type Desk = (ServerId, AccountId, PlaceId);
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Token {
    pub range: std::ops::Range<usize>,
    pub id: String,
    pub text: String,
    pub label: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedDraft {
    pub desk: Desk,
    pub entry_generation: u64,
    pub text: String,
    pub tokens: Vec<Token>,
    pub reply: Option<MessageId>,
    pub editing: Option<(MessageId, Document, crate::compose::Written)>,
}
impl SavedDraft {
    pub fn content(&self) -> InputContent {
        let mut content = InputContent::new(self.text.clone());
        for token in &self.tokens {
            if let Ok(with) = content.clone().with_token(
                token.range.clone(),
                InlineToken::new(token.id.clone(), token.text.clone())
                    .with_label(token.label.clone()),
            ) {
                content = with;
            }
        }
        content
    }
    pub fn empty(&self) -> bool {
        self.text.trim().is_empty() && self.reply.is_none() && self.editing.is_none()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FileSource {
    File(std::path::PathBuf),
    Picture { name: String, hash: String },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedFiles {
    pub desk: Desk,
    pub entry_generation: u64,
    pub sources: Vec<FileSource>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub texts: Vec<SavedDraft>,
    pub files: Vec<SavedFiles>,
    /// The edits of messages in their rows, apart from the new messages (ADR 0740).
    #[serde(default)]
    pub edits: Vec<SavedDraft>,
}
impl Snapshot {
    /// A book of an earlier build kept the edit of a message in the draft of its place.
    /// That draft moves to the edits with its target and original document, and the place
    /// keeps no draft of a new message (ADR 0740). An edit already saved for the place wins.
    pub fn upgraded(mut self) -> Self {
        let (edits, texts): (Vec<_>, Vec<_>) = std::mem::take(&mut self.texts)
            .into_iter()
            .partition(|draft| draft.editing.is_some());
        self.texts = texts;
        for mut edit in edits {
            if self.edits.iter().all(|held| held.desk != edit.desk) {
                edit.reply = None;
                self.edits.push(edit);
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_token_ranges_and_account_keys_survive_the_codec() {
        let channel = junto_client_core::protocol::ids::ChannelId::generate();
        let server = ServerId::generate();
        let account = AccountId::generate();
        let first = SavedDraft {
            entry_generation: 42,
            desk: (server, account, PlaceId::Channel(channel)),
            text: "日本語 #general".to_owned(),
            tokens: vec![Token {
                range: "日本語 ".len().."日本語 #general".len(),
                id: format!("channel:{channel}"),
                text: "#general".to_owned(),
                label: "#general".to_owned(),
            }],
            reply: None,
            editing: None,
        };
        let mut other = first.clone();
        other.desk.1 = AccountId::generate();
        other.text = "other account".to_owned();
        other.tokens.clear();
        let decoded: Vec<SavedDraft> =
            serde_json::from_slice(&serde_json::to_vec(&[first.clone(), other.clone()]).unwrap())
                .unwrap();
        assert_eq!(decoded[0].content().tokens().len(), 1);
        assert_eq!(decoded[0].content().text().as_ref(), first.text);
        let desks: std::collections::HashMap<_, _> = decoded
            .into_iter()
            .map(|draft| (draft.desk, draft.text))
            .collect();
        assert_eq!(desks.len(), 2);
        assert_eq!(desks[&other.desk], "other account");
    }

    /// A book of the earlier format, without `edits`, keeps an edit in `texts`: it moves to
    /// the edits with its target and original document.
    #[test]
    fn an_edit_in_a_book_of_an_earlier_build_moves_to_the_edits() {
        let channel = junto_client_core::protocol::ids::ChannelId::generate();
        let desk = (
            ServerId::generate(),
            AccountId::generate(),
            PlaceId::Channel(channel),
        );
        let message = MessageId::generate();
        let original = Document { blocks: Vec::new() };
        let written = crate::compose::written(&original, &|_| String::new());
        let edit = SavedDraft {
            desk,
            entry_generation: 3,
            text: "changed text".to_owned(),
            tokens: Vec::new(),
            reply: None,
            editing: Some((message, original.clone(), written)),
        };
        let mut new_message = edit.clone();
        new_message.desk.2 =
            PlaceId::Channel(junto_client_core::protocol::ids::ChannelId::generate());
        new_message.text = "a new message".to_owned();
        new_message.editing = None;
        let earlier = serde_json::json!({
            "texts": [edit, new_message],
            "files": [],
        });
        let snapshot: Snapshot = serde_json::from_value(earlier).unwrap();
        let snapshot = snapshot.upgraded();
        assert_eq!(snapshot.texts.len(), 1);
        assert_eq!(snapshot.texts[0].text, "a new message");
        assert_eq!(snapshot.edits.len(), 1);
        assert_eq!(snapshot.edits[0].desk, desk);
        assert_eq!(snapshot.edits[0].text, "changed text");
        assert_eq!(
            snapshot.edits[0]
                .editing
                .as_ref()
                .map(|(id, document, _)| (*id, document.clone())),
            Some((message, original))
        );
    }
}
