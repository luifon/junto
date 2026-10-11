//! Access tokens of administrators (ADR 0203, ADR 0210).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{AccessTokenId, AccountId};
use crate::secret::AccessToken;

/// The header value prefix of an access token.
pub const ACCESS_TOKEN_PREFIX: &str = "jat_";

/// One group of routes that an access token can reach. A token states its actions when it is
/// created, and a request with it is refused on a route of another action.
///
/// A server can add actions within one API version. An action that this build does not know
/// is read as [`TokenAction::Unknown`], which is never written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TokenAction {
    /// Read the workspaces, categories, channels, roles, overrides and members.
    ReadStructure,
    /// Create and change workspaces, categories, channels, roles and overrides.
    ManageStructure,
    /// Assign roles, add members, create invites, decide join requests, remove and ban.
    ManageMembers,
    /// Read the audit log of the server and of workspaces.
    ReadAuditLog,
    /// An action that this build does not know. It is read and never written.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl TokenAction {
    /// Every action that this build knows, without [`TokenAction::Unknown`].
    pub const ALL: &'static [Self] = &[
        Self::ReadStructure,
        Self::ManageStructure,
        Self::ManageMembers,
        Self::ReadAuditLog,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::ReadStructure => "read_structure",
            Self::ManageStructure => "manage_structure",
            Self::ManageMembers => "manage_members",
            Self::ReadAuditLog => "read_audit_log",
            Self::Unknown => "unknown",
        }
    }

    /// The fixed bit of the action in a stored set. The number of a bit is not used again
    /// for another action. `None` for [`TokenAction::Unknown`].
    pub const fn bit(self) -> Option<u32> {
        match self {
            Self::ReadStructure => Some(0),
            Self::ManageStructure => Some(1),
            Self::ManageMembers => Some(2),
            Self::ReadAuditLog => Some(3),
            Self::Unknown => None,
        }
    }
}

/// An access token as a list shows it. It never contains the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccessTokenInfo {
    pub id: AccessTokenId,
    pub name: String,
    /// The account that created the token. A request with the token is decided with the
    /// permissions that this account has at the time of the request.
    pub created_by: AccountId,
    pub actions: Vec<TokenAction>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateAccessToken {
    pub name: String,
    /// At least one action.
    pub actions: Vec<TokenAction>,
    /// At most 366 days after the request.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// The answer to the creation of a token. The token is not shown again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccessTokenCreated {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub token: AccessToken,
    pub access_token: AccessTokenInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccessTokenPage {
    /// Newest first.
    pub access_tokens: Vec<AccessTokenInfo>,
    /// Pass as `before` to read the next page. Absent on the last page.
    pub next_before: Option<AccessTokenId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct AccessTokenQuery {
    /// Return tokens older than this one.
    #[serde(default)]
    pub before: Option<AccessTokenId>,
    /// The largest number of tokens to return, from 1 to 200. Absent: 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_fixed_bit_and_its_wire_name() {
        let fixed = [
            (TokenAction::ReadStructure, 0, "read_structure"),
            (TokenAction::ManageStructure, 1, "manage_structure"),
            (TokenAction::ManageMembers, 2, "manage_members"),
            (TokenAction::ReadAuditLog, 3, "read_audit_log"),
        ];
        assert_eq!(fixed.len(), TokenAction::ALL.len());
        for (action, bit, name) in fixed {
            assert!(TokenAction::ALL.contains(&action));
            assert_eq!(action.bit(), Some(bit));
            assert_eq!(action.name(), name);
            assert_eq!(serde_json::to_value(action).unwrap(), name);
        }
    }

    #[test]
    fn an_action_of_a_newer_server_is_read_as_unknown_and_never_written() {
        let action: TokenAction = serde_json::from_str("\"manage_recordings\"").unwrap();
        assert_eq!(action, TokenAction::Unknown);
        assert_eq!(action.bit(), None);
        assert!(serde_json::to_string(&action).is_err());
        assert!(!TokenAction::ALL.contains(&TokenAction::Unknown));
    }

    #[test]
    fn the_created_token_is_not_in_debug_output() {
        let created = AccessTokenCreated {
            token: AccessToken::new("jat_secret-value"),
            access_token: AccessTokenInfo {
                id: AccessTokenId::generate(),
                name: "setup".to_owned(),
                created_by: AccountId::generate(),
                actions: vec![TokenAction::ReadStructure],
                created_at: OffsetDateTime::UNIX_EPOCH,
                expires_at: OffsetDateTime::UNIX_EPOCH,
            },
        };
        assert!(!format!("{created:?}").contains("secret-value"));
    }
}
