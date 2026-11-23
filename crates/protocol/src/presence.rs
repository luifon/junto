use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::AccountId;

pub const PRESENCE_PAGE: usize = 100;

/// The most characters of a status text.
pub const STATUS_TEXT_MAX_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
    #[default]
    #[serde(other)]
    Offline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PresenceChoice {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
}

impl PresenceChoice {
    pub const ALL: [Self; 4] = [
        Self::Online,
        Self::Idle,
        Self::DoNotDisturb,
        Self::Invisible,
    ];

    pub const fn stored(self) -> &'static str {
        match self {
            Self::Online | Self::DoNotDisturb => "online",
            Self::Idle => "idle",
            Self::Invisible => "invisible",
        }
    }

    pub const fn shown(self, connected: bool, idle: bool, own: bool) -> Presence {
        if !connected {
            return Presence::Offline;
        }
        match self {
            Self::Invisible if own => Presence::Invisible,
            Self::Invisible => Presence::Offline,
            Self::DoNotDisturb => Presence::DoNotDisturb,
            Self::Idle => Presence::Idle,
            Self::Online if idle => Presence::Idle,
            Self::Online => Presence::Online,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ReadPresence {
    /// At most 100 known identities. Unknown, hidden and invisible identities answer offline.
    pub accounts: Vec<AccountId>,
    /// Inactivity in the session of the operating system, for this authenticated device.
    pub idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccountPresence {
    pub account_id: AccountId,
    pub presence: Presence,
    /// The status text of the account, for a caller who may see the account. Absent when
    /// the account has none or its end time has passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<StatusText>,
}

/// A short text that an account shows with its status, with an optional end time after
/// which the server removes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct StatusText {
    /// One line of 1 to 100 characters without control characters.
    pub text: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub until: Option<OffsetDateTime>,
}

/// The new status text of the caller's account, or `null` to remove it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SetStatusText {
    pub status_text: Option<StatusText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PresencePage {
    pub own: PresenceChoice,
    pub accounts: Vec<AccountPresence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SetPresence {
    pub choice: PresenceChoice,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invisible_is_offline_to_others_and_any_active_device_wins() {
        assert_eq!(
            PresenceChoice::Invisible.shown(true, false, false),
            Presence::Offline
        );
        assert_eq!(
            PresenceChoice::Invisible.shown(true, false, true),
            Presence::Invisible
        );
        assert_eq!(
            PresenceChoice::DoNotDisturb.shown(false, false, false),
            Presence::Offline
        );
        assert_eq!(
            PresenceChoice::Online.shown(true, true, false),
            Presence::Idle
        );
        assert_eq!(
            PresenceChoice::Online.shown(true, false, false),
            Presence::Online
        );
        assert!(serde_json::from_str::<SetPresence>(r#"{"choice":"unknown"}"#).is_err());
        let without_text: AccountPresence = serde_json::from_value(serde_json::json!({
            "account_id": AccountId::generate(), "presence": "online",
        }))
        .unwrap();
        assert_eq!(without_text.status_text, None);
        let cleared: SetStatusText = serde_json::from_str(r#"{"status_text":null}"#).unwrap();
        assert_eq!(cleared.status_text, None);
        let set: SetStatusText =
            serde_json::from_str(r#"{"status_text":{"text":"Out of office"}}"#).unwrap();
        assert_eq!(set.status_text.and_then(|text| text.until), None);
        assert_eq!(
            serde_json::from_str::<Presence>(r#""unknown""#).unwrap(),
            Presence::Offline
        );
    }
}
