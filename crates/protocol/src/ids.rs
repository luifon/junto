use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
        )]
        #[cfg_attr(feature = "sqlx", derive(sqlx::Type), sqlx(transparent))]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }

            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.as_hyphenated().fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(text).map(Self)
            }
        }
    };
}

identifier!(
    /// Identifies one server. A client stores it with the server address.
    ServerId
);
identifier!(AccountId);
identifier!(DeviceId);
identifier!(ChallengeId);
identifier!(InviteId);
identifier!(JoinRequestId);
identifier!(RecoveryId);
identifier!(ServerRoleId);
identifier!(WorkspaceId);
identifier!(WorkspaceRoleId);
identifier!(CategoryId);
identifier!(ChannelId);
identifier!(AuditEntryId);
identifier!(
    /// Identifies one history of event positions. It changes when positions start again.
    StreamEpoch
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_is_a_hyphenated_uuid_on_the_wire() {
        let id: AccountId = "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f".parse().unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f\"");
        assert_eq!(serde_json::from_str::<AccountId>(&json).unwrap(), id);
        assert_eq!(id.to_string(), "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f");
    }

    #[test]
    fn generated_identifiers_are_version_7_and_ordered() {
        let first = ChannelId::generate();
        let second = ChannelId::generate();
        assert_eq!(first.as_uuid().get_version_num(), 7);
        assert!(first < second);
    }
}
