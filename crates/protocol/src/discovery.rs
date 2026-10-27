//! Search, notification policy and retention (ADR 0409).
use crate::ids::{AccountId, MessageId, RecordingId, WorkspaceId};
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
    /// Account scope: the system notifications of messages name who wrote and where,
    /// without the text (ADR 0742).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hide_text: bool,
    /// Account scope: system notifications and the sounds of messages wait while the person
    /// shares a screen or presents in meeting mode. Absent: they wait (ADR 0742).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_while_presenting: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct DailyQuiet {
    /// The minute of the day at which the quiet time starts, from 0 to 1439, on the clock
    /// of `time_zone`.
    pub start_minute: u16,
    /// The minute of the day at which it ends. An end before the start is on the next day.
    pub end_minute: u16,
    /// The name of a time zone of the IANA database, for example `Europe/Lisbon`. The
    /// quiet time holds while the clock of that zone is inside the window, also after a
    /// change for daylight saving time (ADR 0412).
    pub time_zone: String,
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
    /// A direct-message call that the account did not answer (ADR 0518). The message of
    /// the notice is the system line `direct_call_not_answered`.
    MissedCall,
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
    /// Absent: the workspace has no default.
    pub level: Option<NotificationLevel>,
}

/// Marks the notices of the caller read.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct MarkNotificationsRead {
    /// Every notice created at or before this time is marked: the `created_at` of the
    /// newest notice that the person saw. Absent, or a later time: the time of the
    /// request.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub until: Option<OffsetDateTime>,
}

/// What a request that marks the notices of the caller read did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotificationsRead {
    #[serde(with = "time::serde::rfc3339")]
    pub until: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub read_at: OffsetDateTime,
    /// The number of notices that the request marked.
    pub marked: u32,
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
    /// The workspace of the channel of the result. Absent for a direct conversation.
    #[serde(default)]
    pub workspace_id: Option<WorkspaceId>,
    pub author_id: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub text: String,
    pub rank: f32,
    /// The position of the message in its channel or conversation. Absent for a
    /// transcript line.
    #[serde(default)]
    pub position: Option<u64>,
    /// The root of the thread that the message is a reply of.
    #[serde(default)]
    pub thread_root: Option<MessageId>,
    /// For a transcript line: milliseconds from the start of the timeline of the
    /// recording.
    #[serde(default)]
    pub start_ms: Option<u64>,
}
