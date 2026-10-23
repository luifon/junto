use junto_authz::{ServerPermission, ServerPermissions, WorkspacePermission, WorkspacePermissions};
use junto_protocol::account::{AccountStatus, DeviceOrigin};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::invite::InviteRevocation;
use junto_protocol::keys::PublicKey;
use junto_protocol::structure::ChannelKind;

use crate::error::{ApiError, ApiResult};

/// A value that is stored as one of a fixed set of texts.
pub trait StoredText: Sized + Copy {
    fn stored(self) -> &'static str;
    fn from_stored(text: &str) -> Option<Self>;

    fn read(text: &str) -> ApiResult<Self> {
        Self::from_stored(text)
            .ok_or_else(|| ApiError::internal(&format_args!("unknown stored value {text:?}")))
    }
}

macro_rules! stored_text {
    ($type:ty { $($variant:ident => $text:literal,)+ }) => {
        impl StoredText for $type {
            fn stored(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            fn from_stored(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

impl StoredText for AccountStatus {
    /// `unknown` is refused by the check of the column: the server never writes a state that
    /// it read from a newer server.
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Banned => "banned",
            Self::Removed => "removed",
            Self::Inactive => "inactive",
            Self::Deleted => "deleted",
            Self::Unknown => "unknown",
        }
    }

    fn from_stored(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "banned" => Some(Self::Banned),
            "removed" => Some(Self::Removed),
            "inactive" => Some(Self::Inactive),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

stored_text!(DeviceOrigin {
    OwnerCode => "owner_code",
    Invite => "invite",
    JoinRequest => "join_request",
    Link => "link",
    Recovery => "recovery",
});

stored_text!(ChannelKind {
    Text => "text",
    Voice => "voice",
});

stored_text!(InviteRevocation {
    Revoked => "revoked",
    CreatorBanned => "creator_banned",
});

stored_text!(ChallengeOperation {
    Login => "login",
    ClaimOwner => "claim_owner",
    Join => "join",
    RequestToJoin => "request_to_join",
    JoinRequestStatus => "join_request_status",
    LinkDevice => "link_device",
    ModerationNotices => "moderation_notices",
});

pub fn server_permissions(bits: i64) -> ApiResult<ServerPermissions> {
    u64::try_from(bits)
        .ok()
        .and_then(ServerPermissions::from_bits)
        .ok_or_else(|| ApiError::internal(&format_args!("unknown server permission bits {bits}")))
}

pub fn workspace_permissions(bits: i64) -> ApiResult<WorkspacePermissions> {
    u64::try_from(bits)
        .ok()
        .and_then(WorkspacePermissions::from_bits)
        .ok_or_else(|| {
            ApiError::internal(&format_args!("unknown workspace permission bits {bits}"))
        })
}

pub fn server_bits(permissions: ServerPermissions) -> i64 {
    i64::try_from(permissions.bits()).unwrap_or(i64::MAX)
}

pub fn workspace_bits(permissions: WorkspacePermissions) -> i64 {
    i64::try_from(permissions.bits()).unwrap_or(i64::MAX)
}

pub fn server_list(permissions: ServerPermissions) -> Vec<ServerPermission> {
    permissions.iter().collect()
}

pub fn workspace_list(permissions: WorkspacePermissions) -> Vec<WorkspacePermission> {
    permissions.iter().collect()
}

pub fn public_key(bytes: &[u8]) -> ApiResult<PublicKey> {
    PublicKey::try_from(bytes).map_err(|error| ApiError::internal(&error))
}

pub fn count(value: i32) -> ApiResult<u32> {
    u32::try_from(value).map_err(|error| ApiError::internal(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_texts_match_the_wire_names() {
        fn check<T>(values: &[T])
        where
            T: StoredText + serde::Serialize + PartialEq + std::fmt::Debug,
        {
            for value in values {
                let wire = serde_json::to_value(value).unwrap();
                assert_eq!(wire, serde_json::Value::from(value.stored()));
                assert_eq!(T::from_stored(value.stored()).as_ref(), Some(value));
            }
        }
        check(&[
            AccountStatus::Active,
            AccountStatus::Banned,
            AccountStatus::Removed,
            AccountStatus::Inactive,
            AccountStatus::Deleted,
        ]);
        assert_eq!(AccountStatus::from_stored("unknown"), None);
        check(&[
            DeviceOrigin::OwnerCode,
            DeviceOrigin::Invite,
            DeviceOrigin::JoinRequest,
            DeviceOrigin::Link,
            DeviceOrigin::Recovery,
        ]);
        check(&[ChannelKind::Text, ChannelKind::Voice]);
        check(&[InviteRevocation::Revoked, InviteRevocation::CreatorBanned]);
        check(ChallengeOperation::ALL);
        assert_eq!(ChannelKind::from_stored("video"), None);
    }

    #[test]
    fn permission_bits_round_trip_and_unknown_bits_are_refused() {
        let all = WorkspacePermissions::ALL;
        assert_eq!(workspace_permissions(workspace_bits(all)).unwrap(), all);
        assert!(workspace_permissions(1 << 16).is_err());
        assert!(workspace_permissions(-1).is_err());
        assert_eq!(
            server_permissions(server_bits(ServerPermissions::ALL)).unwrap(),
            ServerPermissions::ALL
        );
        assert!(server_permissions(64).is_err());
    }
}
