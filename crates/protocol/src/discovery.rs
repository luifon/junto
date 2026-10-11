//! Search, notification policy and retention (ADR 0409).
use crate::ids::{AccountId, MessageId, RecordingId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    All,
    #[default]
    Mentions,
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct NotificationSettings {
    pub level: Option<NotificationLevel>,
    #[serde(default)]
    pub mute: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub muted_until: Option<OffsetDateTime>,
    pub role_mentions: Option<bool>,
    pub everyone_mentions: Option<bool>,
    #[serde(default)]
    pub do_not_disturb: bool,
    pub daily_quiet: Option<DailyQuiet>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct DailyQuiet {
    pub start_minute: u16,
    pub end_minute: u16,
    pub utc_offset_minutes: i16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScopedNotificationSettings {
    pub scope_id: Uuid,
    pub settings: NotificationSettings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Message,
    Mention,
    Direct,
    Thread,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Notification {
    pub id: Uuid,
    pub message_id: MessageId,
    pub kind: NotificationKind,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub read_at: Option<OffsetDateTime>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotificationPage {
    pub notifications: Vec<Notification>,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct NotificationDefault {
    pub level: NotificationLevel,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Retention {
    pub days: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchPage {
    pub results: Vec<SearchHit>,
    pub next: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchHit {
    pub message_id: Option<MessageId>,
    pub recording_id: Option<RecordingId>,
    pub line: Option<u32>,
    pub place_id: Uuid,
    pub author_id: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub text: String,
    pub rank: f32,
}
