use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::account::RecoveryNotice;
use crate::ids::{AccountId, ChallengeId, DeviceId, ServerId};
use crate::keys::{Nonce, PublicKey, Signature};
use crate::secret::{OwnerCode, SessionToken};

/// What a signed challenge is for. A challenge is accepted only by the route of its operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeOperation {
    Login,
    ClaimOwner,
    Join,
    RequestToJoin,
    JoinRequestStatus,
    LinkDevice,
}

impl ChallengeOperation {
    pub const ALL: &'static [Self] = &[
        Self::Login,
        Self::ClaimOwner,
        Self::Join,
        Self::RequestToJoin,
        Self::JoinRequestStatus,
        Self::LinkDevice,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::ClaimOwner => "claim_owner",
            Self::Join => "join",
            Self::RequestToJoin => "request_to_join",
            Self::JoinRequestStatus => "join_request_status",
            Self::LinkDevice => "link_device",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateChallenge {
    pub public_key: PublicKey,
    pub operation: ChallengeOperation,
}

/// A single-use challenge. The device signs [`Challenge::signing_input`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Challenge {
    pub id: ChallengeId,
    pub server_id: ServerId,
    pub operation: ChallengeOperation,
    /// The account of the device key, when the key belongs to one.
    pub account_id: Option<AccountId>,
    pub public_key: PublicKey,
    pub nonce: Nonce,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl Challenge {
    pub const SIGNING_DOMAIN: &'static str = "junto-challenge-v1";

    /// The bytes that the device signs. A client builds them from the fields after it has
    /// compared `server_id`, `operation` and `public_key` with what it asked for.
    pub fn signing_input(&self) -> Vec<u8> {
        let account = match self.account_id {
            Some(account_id) => account_id.to_string(),
            None => "none".to_owned(),
        };
        format!(
            "{domain}\nserver:{server}\noperation:{operation}\naccount:{account}\ndevice:{device}\nchallenge:{challenge}\nnonce:{nonce}\nexpires:{expires}",
            domain = Self::SIGNING_DOMAIN,
            server = self.server_id,
            operation = self.operation.name(),
            device = self.public_key,
            challenge = self.id,
            nonce = self.nonce,
            expires = self.expires_at.unix_timestamp(),
        )
        .into_bytes()
    }
}

/// The answer to a challenge. The challenge names the device key that must have signed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Proof {
    pub challenge_id: ChallengeId,
    pub signature: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Login {
    pub proof: Proof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SessionCreated {
    pub token: SessionToken,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    /// The recoveries of the account since the previous login of this device.
    pub recoveries: Vec<RecoveryNotice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClaimOwner {
    pub proof: Proof,
    pub owner_code: OwnerCode,
    pub name: String,
    #[serde(default)]
    pub device_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_names_match_the_wire_names() {
        for operation in ChallengeOperation::ALL {
            assert_eq!(
                serde_json::to_value(operation).unwrap(),
                serde_json::Value::from(operation.name())
            );
        }
    }

    #[test]
    fn signing_input_names_server_operation_account_and_device() {
        let challenge = Challenge {
            id: "0198f1c2-7d3a-7b1e-9c55-000000000001".parse().unwrap(),
            server_id: "0198f1c2-7d3a-7b1e-9c55-000000000002".parse().unwrap(),
            operation: ChallengeOperation::Login,
            account_id: Some("0198f1c2-7d3a-7b1e-9c55-000000000003".parse().unwrap()),
            public_key: PublicKey::from_bytes([0; 32]),
            nonce: Nonce::from_bytes([255; 32]),
            expires_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
        };
        let expected = "junto-challenge-v1\n\
            server:0198f1c2-7d3a-7b1e-9c55-000000000002\n\
            operation:login\n\
            account:0198f1c2-7d3a-7b1e-9c55-000000000003\n\
            device:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n\
            challenge:0198f1c2-7d3a-7b1e-9c55-000000000001\n\
            nonce:__________________________________________8\n\
            expires:1800000000";
        assert_eq!(
            String::from_utf8(challenge.signing_input()).unwrap(),
            expected
        );

        let without_account = Challenge {
            account_id: None,
            operation: ChallengeOperation::Join,
            ..challenge.clone()
        };
        let text = String::from_utf8(without_account.signing_input()).unwrap();
        assert!(text.contains("\naccount:none\n"));
        assert!(text.contains("\noperation:join\n"));
        assert_ne!(without_account.signing_input(), challenge.signing_input());
    }
}
