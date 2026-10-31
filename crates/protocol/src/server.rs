use serde::{Deserialize, Serialize};

use crate::ids::ServerId;

/// What a server tells any caller before the caller has an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerInfo {
    pub server_id: ServerId,
    pub name: String,
    /// The path of the logo on this server, when the server has one.
    pub logo_path: Option<String>,
    pub api_versions: Vec<u32>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Capabilities {
    #[serde(default)]
    pub afk_channel: bool,
    #[serde(default)]
    pub channel_topics: bool,
    #[serde(default)]
    pub message_format: crate::message::DocumentFormat,
    #[serde(default)]
    pub search: crate::discovery::SearchCapabilities,
}

/// What the health route reports when the server can serve requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Health {
    /// True when the server reached its database for this response.
    pub database: bool,
    /// Whether the last admission check reached the media server. Absent on a server that
    /// has no media server configured. While it is false, no call can be joined.
    pub media_server: Option<bool>,
    /// The number of rooms of calls that the server could not check several times in a row,
    /// while the media server answers otherwise. In such a room the server cannot remove a
    /// person, and it lets nobody in.
    #[serde(default)]
    pub failing_call_rooms: u32,
    /// True while the free disk space of the recordings, of the attachments or of the
    /// exports is at or below the reserve: the server starts no recording, stops the ones
    /// that run, and refuses new uploads and exports (ADR 0006). Absent on a server without
    /// such a directory, and when no free space can be read.
    #[serde(default)]
    pub storage_low: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_topic_capabilities_are_unsupported_and_unknown_capabilities_are_ignored() {
        let mut old = serde_json::json!({ "server_id": ServerId::generate(), "name": "Example Studio", "logo_path": null, "api_versions": [1] });
        let read: ServerInfo = serde_json::from_value(old.clone()).unwrap();
        assert!(!read.capabilities.afk_channel);
        assert!(!read.capabilities.channel_topics);
        assert_eq!(read.capabilities.message_format.event_revision, 0);
        assert_eq!(
            read.capabilities.search,
            crate::discovery::SearchCapabilities::default()
        );
        old["capabilities"] = serde_json::json!({ "future_feature": true });
        let read: ServerInfo = serde_json::from_value(old.clone()).unwrap();
        assert!(!read.capabilities.afk_channel);
        assert!(!read.capabilities.channel_topics);
        assert_eq!(read.capabilities.message_format.event_revision, 0);
        old["capabilities"]["channel_topics"] = serde_json::json!(true);
        let read: ServerInfo = serde_json::from_value(old).unwrap();
        assert!(read.capabilities.channel_topics);
    }

    #[test]
    fn partial_search_capabilities_default_each_missing_feature_and_ignore_unknowns() {
        let read: Capabilities = serde_json::from_value(serde_json::json!({
            "search": { "filters": true, "future_feature": true }, "unknown": true
        }))
        .unwrap();
        assert!(!read.channel_topics);
        assert!(read.search.filters);
        assert!(!read.search.filter_only && !read.search.files);
        let topic: Capabilities =
            serde_json::from_value(serde_json::json!({"channel_topics":true})).unwrap();
        assert!(topic.channel_topics);
        assert_eq!(
            topic.search,
            crate::discovery::SearchCapabilities::default()
        );
    }
}

#[cfg(test)]
mod document_capability_tests {
    use super::*;

    #[test]
    fn document_capabilities_default_to_zero_and_ignore_unknown_metadata() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"message_format":{}}),
        ] {
            let read: Capabilities = serde_json::from_value(value).unwrap();
            assert_eq!(read.message_format.event_revision, 0);
        }
        let read: Capabilities = serde_json::from_value(serde_json::json!({
            "message_format":{"event_revision":crate::events::EVENT_REVISION,"future_feature":true}
        }))
        .unwrap();
        assert_eq!(
            read.message_format.event_revision,
            crate::events::EVENT_REVISION
        );
        assert!(!read.channel_topics);
        assert_eq!(read.search, crate::discovery::SearchCapabilities::default());
    }
}
