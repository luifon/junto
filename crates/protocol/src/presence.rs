use serde::{Deserialize, Serialize};

use crate::ids::AccountId;

pub const PRESENCE_PAGE: usize = 100;

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
        assert_eq!(
            serde_json::from_str::<Presence>(r#""unknown""#).unwrap(),
            Presence::Offline
        );
    }
}
