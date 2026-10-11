//! The one place where a device key signs (ADR 0108, ADR 0117).
//!
//! The client builds the signed bytes from the fields of a challenge, and only after it has
//! compared the server, the operation, the device key and the account with what it asked
//! for. A server cannot obtain a signature for another server, another operation or
//! another key.
//!
//! The signed bytes name the origin that the request goes to. The origin comes from the
//! address in the server book and from no response, so a signature that a device made at
//! one address is of no use at another address.

use junto_protocol::auth::{Challenge, ChallengeOperation, Proof};
use junto_protocol::ids::{AccountId, ServerId};
use junto_protocol::keys::PublicKey;
use junto_protocol::origin::Origin;

use crate::keys::DeviceKey;

/// The account that a challenge may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedAccount {
    /// The key belongs to no account: every operation before a login.
    None,
    /// The account that the client stored for this device.
    Is(AccountId),
    /// A login of a key whose account the client learns from the server: after a device
    /// link was approved on another device.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expected {
    /// The origin of the address that the challenge was asked from.
    pub(crate) origin: Origin,
    pub(crate) server_id: ServerId,
    pub(crate) operation: ChallengeOperation,
    pub(crate) public_key: PublicKey,
    pub(crate) account: ExpectedAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChallengeMismatch {
    #[error("it names the server {got}, and the saved server is {expected}")]
    Server { expected: ServerId, got: ServerId },
    #[error("it is for another operation")]
    Operation,
    #[error("it names another device key")]
    PublicKey,
    #[error("it names another account")]
    Account,
}

/// A challenge whose fields equal what the client asked for, with the origin that it was
/// asked from.
#[derive(Debug)]
pub(crate) struct VerifiedChallenge {
    challenge: Challenge,
    origin: Origin,
}

pub(crate) fn verify(
    challenge: Challenge,
    expected: Expected,
) -> Result<VerifiedChallenge, ChallengeMismatch> {
    if challenge.server_id != expected.server_id {
        return Err(ChallengeMismatch::Server {
            expected: expected.server_id,
            got: challenge.server_id,
        });
    }
    if challenge.operation != expected.operation {
        return Err(ChallengeMismatch::Operation);
    }
    if challenge.public_key != expected.public_key {
        return Err(ChallengeMismatch::PublicKey);
    }
    let account_fits = match (expected.account, challenge.account_id) {
        (ExpectedAccount::None, None) => true,
        (ExpectedAccount::Is(expected), Some(got)) => expected == got,
        (ExpectedAccount::Any, Some(_)) => true,
        _ => false,
    };
    if !account_fits {
        return Err(ChallengeMismatch::Account);
    }
    Ok(VerifiedChallenge {
        challenge,
        origin: expected.origin,
    })
}

impl VerifiedChallenge {
    pub(crate) fn account_id(&self) -> Option<AccountId> {
        self.challenge.account_id
    }

    /// Signs with the key that the challenge names. Another key is refused.
    pub(crate) fn sign(&self, key: &DeviceKey) -> Result<Proof, ChallengeMismatch> {
        if key.public_key() != self.challenge.public_key {
            return Err(ChallengeMismatch::PublicKey);
        }
        Ok(Proof {
            challenge_id: self.challenge.id,
            origin: self.origin.clone(),
            signature: key.sign(&self.challenge.signing_input(&self.origin)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::VerifyingKey;
    use junto_protocol::ids::ChallengeId;
    use junto_protocol::keys::Nonce;
    use time::OffsetDateTime;

    fn challenge(key: &DeviceKey, server_id: ServerId, operation: ChallengeOperation) -> Challenge {
        Challenge {
            id: ChallengeId::generate(),
            server_id,
            operation,
            account_id: None,
            public_key: key.public_key(),
            nonce: Nonce::from_bytes([7; 32]),
            expires_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
        }
    }

    fn origin() -> Origin {
        Origin::parse("https://chat.example.test").unwrap()
    }

    fn expected(key: &DeviceKey, server_id: ServerId, operation: ChallengeOperation) -> Expected {
        Expected {
            origin: origin(),
            server_id,
            operation,
            public_key: key.public_key(),
            account: ExpectedAccount::None,
        }
    }

    #[test]
    fn a_matching_challenge_is_signed_over_the_bytes_of_the_record() {
        let key = DeviceKey::generate().unwrap();
        let server_id = ServerId::generate();
        let challenge = challenge(&key, server_id, ChallengeOperation::Join);
        let verified = verify(
            challenge.clone(),
            expected(&key, server_id, ChallengeOperation::Join),
        )
        .unwrap();
        let proof = verified.sign(&key).unwrap();
        assert_eq!(proof.challenge_id, challenge.id);
        assert_eq!(proof.origin, origin());
        let text = String::from_utf8(challenge.signing_input(&origin())).unwrap();
        assert!(text.starts_with(&format!(
            "junto-challenge-v2\nserver:{server_id}\norigin:https://chat.example.test\n\
             operation:join\naccount:none\n"
        )));
        let verifying = VerifyingKey::from_bytes(key.public_key().as_bytes()).unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(proof.signature.as_bytes());
        verifying
            .verify_strict(text.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn the_signature_is_valid_for_the_origin_of_the_request_only() {
        let key = DeviceKey::generate().unwrap();
        let server_id = ServerId::generate();
        let challenge = challenge(&key, server_id, ChallengeOperation::Login);
        let asked_at = Origin::parse("https://relay.example.test").unwrap();
        let verified = verify(
            Challenge {
                account_id: Some(AccountId::generate()),
                ..challenge.clone()
            },
            Expected {
                origin: asked_at.clone(),
                account: ExpectedAccount::Any,
                ..expected(&key, server_id, ChallengeOperation::Login)
            },
        )
        .unwrap();
        let proof = verified.sign(&key).unwrap();
        assert_eq!(proof.origin, asked_at);
        let verifying = VerifyingKey::from_bytes(key.public_key().as_bytes()).unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(proof.signature.as_bytes());
        let signed_for = |origin: &Origin| {
            let bytes = Challenge {
                account_id: verified.account_id(),
                ..challenge.clone()
            }
            .signing_input(origin);
            verifying.verify_strict(&bytes, &signature).is_ok()
        };
        assert!(signed_for(&asked_at));
        // The server that the challenge came from has another origin. The same signature
        // does not fit the bytes with that origin.
        assert!(!signed_for(&origin()));
    }

    #[test]
    fn a_challenge_of_another_server_is_not_signed() {
        let key = DeviceKey::generate().unwrap();
        let (saved, other) = (ServerId::generate(), ServerId::generate());
        let relayed = challenge(&key, other, ChallengeOperation::Login);
        assert_eq!(
            verify(relayed, expected(&key, saved, ChallengeOperation::Login)).unwrap_err(),
            ChallengeMismatch::Server {
                expected: saved,
                got: other
            }
        );
    }

    #[test]
    fn a_challenge_for_another_operation_or_key_is_not_signed() {
        let key = DeviceKey::generate().unwrap();
        let other_key = DeviceKey::generate().unwrap();
        let server_id = ServerId::generate();
        assert_eq!(
            verify(
                challenge(&key, server_id, ChallengeOperation::Login),
                expected(&key, server_id, ChallengeOperation::Join)
            )
            .unwrap_err(),
            ChallengeMismatch::Operation
        );
        assert_eq!(
            verify(
                challenge(&other_key, server_id, ChallengeOperation::Join),
                expected(&key, server_id, ChallengeOperation::Join)
            )
            .unwrap_err(),
            ChallengeMismatch::PublicKey
        );
        let verified = verify(
            challenge(&other_key, server_id, ChallengeOperation::Join),
            expected(&other_key, server_id, ChallengeOperation::Join),
        )
        .unwrap();
        assert_eq!(
            verified.sign(&key).unwrap_err(),
            ChallengeMismatch::PublicKey
        );
    }

    #[test]
    fn the_account_of_a_challenge_must_be_the_expected_one() {
        let key = DeviceKey::generate().unwrap();
        let server_id = ServerId::generate();
        let (stored, other) = (AccountId::generate(), AccountId::generate());
        let with = |account_id: Option<AccountId>| Challenge {
            account_id,
            ..challenge(&key, server_id, ChallengeOperation::Login)
        };
        let expecting = |account: ExpectedAccount| Expected {
            account,
            ..expected(&key, server_id, ChallengeOperation::Login)
        };
        let accepted = [
            (None, ExpectedAccount::None),
            (Some(stored), ExpectedAccount::Is(stored)),
            (Some(other), ExpectedAccount::Any),
        ];
        for (named, expectation) in accepted {
            let verified = verify(with(named), expecting(expectation)).unwrap();
            assert_eq!(verified.account_id(), named);
        }
        let refused = [
            (Some(stored), ExpectedAccount::None),
            (Some(other), ExpectedAccount::Is(stored)),
            (None, ExpectedAccount::Is(stored)),
            (None, ExpectedAccount::Any),
        ];
        for (named, expectation) in refused {
            assert_eq!(
                verify(with(named), expecting(expectation)).unwrap_err(),
                ChallengeMismatch::Account,
                "{named:?} {expectation:?}"
            );
        }
    }
}
