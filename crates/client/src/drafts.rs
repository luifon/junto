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
}
