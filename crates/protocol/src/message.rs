//! Text messages and attachment uploads (ADR 0408).
use crate::ids::{
    AccountId, AttachmentId, ChannelId, ConversationId, MessageId, RecordingId, WorkspaceId,
    WorkspaceRoleId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessagePlace {
    Channel {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    Direct {
        conversation_id: ConversationId,
        low_account_id: AccountId,
        high_account_id: AccountId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Paragraph {
        content: Vec<Inline>,
    },
    Quote {
        content: Vec<Inline>,
    },
    Code {
        text: String,
        language: Option<String>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<Inline>>,
    },
    Checklist {
        items: Vec<CheckItem>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CheckItem {
    pub checked: bool,
    pub content: Vec<Inline>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Inline {
    Text {
        text: String,
        #[serde(default)]
        marks: Vec<Mark>,
    },
    Code {
        text: String,
    },
    Link {
        text: String,
        url: String,
    },
    Account {
        account_id: AccountId,
    },
    Role {
        role_id: WorkspaceRoleId,
    },
    Everyone,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Bold,
    Italic,
    Strikethrough,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SystemMessage {
    pub action: String,
    #[serde(default)]
    pub subject_id: Option<AccountId>,
    pub actor_id: Option<AccountId>,
    pub recording_id: Option<RecordingId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Message {
    pub id: MessageId,
    pub place: MessagePlace,
    /// Public sequence position; private system lines share the latest public position.
    pub position: u64,
    pub author_id: Option<AccountId>,
    pub author_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub edited_at: Option<OffsetDateTime>,
    pub document: Option<Document>,
    pub system: Option<SystemMessage>,
    pub reply_to: Option<MessageId>,
    pub permanent: bool,
    #[serde(default)]
    pub deleted: bool,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub thread_root: Option<MessageId>,
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub last_reply: Option<MessageId>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub files_removed_by_retention: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    pub own: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SendMessage {
    pub document: Document,
    pub reply_to: Option<MessageId>,
    #[serde(default)]
    pub attachments: Vec<AttachmentId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct EditMessage {
    pub document: Document,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SetReaction {
    pub emoji: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub has_more: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MessageState {
    pub place: MessagePlace,
    pub last_read: u64,
    pub latest: u64,
    /// Exact through 99; 100 means 100 or more unread messages.
    pub unread: u64,
    /// Exact through 99; 100 means 100 or more unread mentions.
    pub mentions: u64,
    #[serde(default)]
    pub unread_sign: bool,
    #[serde(default)]
    pub notification_mentions: u64,
    #[serde(default)]
    pub muted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct MarkRead {
    pub position: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Conversation {
    pub id: ConversationId,
    pub low_account_id: AccountId,
    pub high_account_id: AccountId,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct OpenConversation {
    pub account_id: AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Attachment {
    pub id: AttachmentId,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub media_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct StartUpload {
    pub channel_id: Option<ChannelId>,
    pub conversation_id: Option<ConversationId>,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Upload {
    pub id: AttachmentId,
    pub received: u64,
    pub bytes: u64,
    pub complete: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AttachmentLimits {
    pub bytes_per_file: u64,
    pub files_per_message: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ForkThread {
    pub name: String,
    pub kind: crate::structure::ChannelKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ThreadPage {
    pub root: Message,
    pub replies: MessagePage,
    pub forks: Vec<ChannelId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ThreadState {
    pub root_id: MessageId,
    pub last_read: u64,
    pub latest: u64,
    pub unread: u64,
    pub mentions: u64,
}

/// One page of threads with unread replies or mentions, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UnreadThreads {
    pub threads: Vec<ThreadState>,
    pub more: bool,
    /// Opaque continuation for the next unread-thread page.
    pub next: Option<String>,
}
