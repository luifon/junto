//! Volatile activity of an open composer (ADR 0401).
use crate::ids::{AccountId, ChannelId, ConversationId, MessageId};
use serde::{Deserialize, Serialize};

pub const TYPING_LIFETIME_MS: u32 = 8000;
pub const MOST_TYPING_PLACES: usize = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypingPlace {
    Channel {
        channel_id: ChannelId,
    },
    Direct {
        conversation_id: ConversationId,
    },
    Thread {
        channel_id: ChannelId,
        root_id: MessageId,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Typing {
    pub place: TypingPlace,
    pub account_id: AccountId,
    pub name: String,
    pub active: bool,
    pub expires_in_ms: u32,
}
