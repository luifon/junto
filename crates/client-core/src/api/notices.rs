//! The routes of notices, notification settings and search (ADR 0409).

use junto_protocol::discovery::{
    Notification, NotificationDefault, NotificationLevel, NotificationPage, NotificationSettings,
    SearchPage,
};
use junto_protocol::ids::{AccountId, ChannelId, WorkspaceId};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::client::read_json;
use super::error::ApiError;
use super::session::ServerApi;
use super::transport::Method;

/// What the notification settings of an account belong to (ADR 0409).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SettingsScope {
    /// The defaults of the account on this server, with "do not disturb" and the daily
    /// quiet time.
    Account,
    Workspace(WorkspaceId),
    /// The level, the mute and the two mention switches of one channel.
    Channel(ChannelId),
}

impl SettingsScope {
    /// The identifier that the server uses for the scope. The nil identifier names the
    /// account.
    pub const fn id(self) -> Uuid {
        match self {
            Self::Account => Uuid::nil(),
            Self::Workspace(workspace_id) => workspace_id.as_uuid(),
            Self::Channel(channel_id) => channel_id.as_uuid(),
        }
    }
}

/// A search over the messages and the transcript lines that the caller can read. The text
/// follows the web-search syntax of the server: words, a phrase in quotation marks, `or`,
/// and a minus before a word.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    /// From 1 to 1,024 bytes.
    pub text: String,
    pub channel_id: Option<ChannelId>,
    /// The author of a message, or the speaker of a transcript line.
    pub author_id: Option<AccountId>,
    /// Content from this time on.
    pub from: Option<OffsetDateTime>,
    /// Content before this time.
    pub until: Option<OffsetDateTime>,
    /// From 1 to 200. The server uses 50 without it.
    pub limit: Option<u32>,
    /// The continuation of the page before.
    pub cursor: Option<String>,
}

impl SearchQuery {
    fn pairs(&self) -> Result<Vec<(&'static str, String)>, ApiError> {
        let time = |time: OffsetDateTime| {
            time.format(&Rfc3339)
                .map_err(|error| ApiError::Request(error.to_string()))
        };
        let mut pairs = vec![("q", self.text.clone())];
        if let Some(channel_id) = self.channel_id {
            pairs.push(("channel_id", channel_id.to_string()));
        }
        if let Some(author_id) = self.author_id {
            pairs.push(("author_id", author_id.to_string()));
        }
        if let Some(from) = self.from {
            pairs.push(("from", time(from)?));
        }
        if let Some(until) = self.until {
            pairs.push(("until", time(until)?));
        }
        if let Some(limit) = self.limit {
            pairs.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = &self.cursor {
            pairs.push(("cursor", cursor.clone()));
        }
        Ok(pairs)
    }
}

impl ServerApi {
    // Notices.

    /// One page of the notices of the caller, the newest first. `before` is the last
    /// notice of the page before.
    pub async fn notifications(
        &self,
        before: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<NotificationPage, ApiError> {
        let mut query = Vec::new();
        if let Some(before) = before {
            query.push(("before", before.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        let response = self
            .call(Method::Get, "/notifications".to_owned(), query, None)
            .await?;
        read_json(&response)
    }

    /// Marks one notice as read, on every device of the account.
    pub async fn mark_notification_read(&self, id: Uuid) -> Result<Notification, ApiError> {
        let path = format!("/notifications/{id}/read");
        read_json(&self.call(Method::Put, path, Vec::new(), None).await?)
    }

    // Notification settings.

    /// The settings that the caller stored for a scope. A scope without stored settings
    /// answers with the settings that inherit everything.
    pub async fn notification_settings(
        &self,
        scope: SettingsScope,
    ) -> Result<NotificationSettings, ApiError> {
        self.get(format!("/notification-settings/{}", scope.id()))
            .await
    }

    /// Replaces the settings of a scope. An absent level inherits. The mute and the two
    /// mention switches belong to a channel; "do not disturb" and the daily quiet time
    /// belong to the account. The server refuses them elsewhere.
    pub async fn set_notification_settings(
        &self,
        scope: SettingsScope,
        settings: &NotificationSettings,
    ) -> Result<NotificationSettings, ApiError> {
        let path = format!("/notification-settings/{}", scope.id());
        self.send(Method::Put, path, settings).await
    }

    /// The level that an administrator set for the members of a workspace.
    pub async fn notification_default(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<NotificationLevel, ApiError> {
        let path = format!("/workspaces/{workspace_id}/notification-default");
        let held: NotificationDefault = self.get(path).await?;
        Ok(held.level)
    }

    /// Sets the level for the members of a workspace. Only an administrator can do this.
    pub async fn set_notification_default(
        &self,
        workspace_id: WorkspaceId,
        level: NotificationLevel,
    ) -> Result<NotificationLevel, ApiError> {
        let path = format!("/workspaces/{workspace_id}/notification-default");
        let held: NotificationDefault = self
            .send(Method::Put, path, &NotificationDefault { level })
            .await?;
        Ok(held.level)
    }

    // Search.

    /// One page of results, the best match first. Each page is decided by what the caller
    /// can read at the time of its request (ADR 0409).
    pub async fn search(&self, query: &SearchQuery) -> Result<SearchPage, ApiError> {
        let response = self
            .call(Method::Get, "/search".to_owned(), query.pairs()?, None)
            .await?;
        read_json(&response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_names_the_filters_that_are_set() {
        let query = SearchQuery {
            text: "retention spec".to_owned(),
            ..SearchQuery::default()
        };
        assert_eq!(query.pairs().unwrap(), [("q", "retention spec".to_owned())]);
        let (channel_id, author_id) = (ChannelId::generate(), AccountId::generate());
        let from = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let query = SearchQuery {
            text: "a".to_owned(),
            channel_id: Some(channel_id),
            author_id: Some(author_id),
            from: Some(from),
            until: Some(from + time::Duration::days(1)),
            limit: Some(20),
            cursor: Some("0.1:id:-1".to_owned()),
        };
        assert_eq!(
            query.pairs().unwrap(),
            [
                ("q", "a".to_owned()),
                ("channel_id", channel_id.to_string()),
                ("author_id", author_id.to_string()),
                ("from", "2023-11-14T22:13:20Z".to_owned()),
                ("until", "2023-11-15T22:13:20Z".to_owned()),
                ("limit", "20".to_owned()),
                ("cursor", "0.1:id:-1".to_owned()),
            ]
        );
    }

    #[test]
    fn the_account_scope_is_the_nil_identifier() {
        assert!(SettingsScope::Account.id().is_nil());
        let channel_id = ChannelId::generate();
        assert_eq!(
            SettingsScope::Channel(channel_id).id(),
            channel_id.as_uuid()
        );
    }
}
