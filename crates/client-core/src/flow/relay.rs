//! The messages of the relay of a device link (ADR 0104). The first server keeps them, and
//! servers do not trust each other (ADR 0714), so each message is signed by the sending
//! device and encrypted to the receiving device, with the keys of the two devices on the
//! first server.
//!
//! A sealed message is the public X25519 key of a fresh exchange key (32 bytes) followed by
//! the ChaCha20-Poly1305 encryption of the Ed25519 signature (64 bytes) and the JSON
//! content. The receiving device's Ed25519 public key gives its X25519 public key; the
//! shared secret of the exchange goes through HKDF-SHA256 with both public keys as the salt
//! and the kind of the message as the information. Each key encrypts one message, so the
//! nonce is zero. The signature covers the kind, the first server and the public key of the
//! new device, so a message of one relay is not taken for a message of another.

use curve25519_dalek::montgomery::MontgomeryPoint;
use ed25519_dalek::{Signature as DalekSignature, VerifyingKey};
use junto_protocol::device_relay::Sealed;
use junto_protocol::ids::ServerId;
use junto_protocol::keys::{PublicKey, Signature};
use junto_protocol::secret::LinkCode;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, Salt};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::address::ServerAddress;
use crate::keys::DeviceKey;

const VERSION: &[u8] = b"junto-device-link-relay-v1";
const EXCHANGE_KEY: usize = 32;
const SIGNATURE: usize = 64;

/// The most servers that an offer or an answer names.
pub const MOST_SERVERS: usize = 64;

/// The longest reason of a failed server in an answer, in characters.
pub const LONGEST_REASON: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// From the approving device to the new device.
    Offer,
    /// From the new device to the approving device.
    Answer,
}

impl Kind {
    fn label(self) -> &'static [u8] {
        match self {
            Self::Offer => b"offer",
            Self::Answer => b"answer",
        }
    }
}

/// What a message is bound to: the first server and the key of the new device there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) first: ServerId,
    pub(crate) new_device: PublicKey,
}

/// Why a message could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RelayFault {
    #[error("the message of the other device could not be read")]
    Unreadable,
    #[error("the message is not signed by the other device")]
    NotSigned,
    #[error("the message is longer than a relay takes")]
    TooLong,
    #[error("the random source of the system is not available")]
    Randomness,
}

/// A server that the new device adds and asks for a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferedServer {
    pub server_id: ServerId,
    pub address: ServerAddress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Offer {
    pub(crate) servers: Vec<OfferedServer>,
}

/// Where the link of the new device on one server of the offer stands.
// The fields of a link are flattened into it, which serde does not combine with a refusal
// of unknown fields; the other device signed the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Answered {
    Waiting {
        code: LinkCode,
        public_key: PublicKey,
    },
    /// The new device has an account on the server already.
    Linked,
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AnsweredLink {
    pub(crate) server_id: ServerId,
    #[serde(flatten)]
    pub(crate) answered: Answered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Answer {
    pub(crate) links: Vec<AnsweredLink>,
}

fn signed_text(kind: Kind, binding: Binding, content: &[u8]) -> Vec<u8> {
    let mut text = Vec::with_capacity(VERSION.len() + 96 + content.len());
    text.extend_from_slice(VERSION);
    text.push(b'\n');
    text.extend_from_slice(kind.label());
    text.push(b'\n');
    text.extend_from_slice(binding.first.to_string().as_bytes());
    text.push(b'\n');
    text.extend_from_slice(binding.new_device.as_bytes());
    text.push(b'\n');
    text.extend_from_slice(content);
    text
}

fn exchange_public(key: PublicKey) -> Result<MontgomeryPoint, RelayFault> {
    VerifyingKey::from_bytes(key.as_bytes())
        .map(|key| key.to_montgomery())
        .map_err(|_| RelayFault::Unreadable)
}

/// The key that encrypts one message, from the shared secret of the exchange.
fn message_key(
    kind: Kind,
    shared: &MontgomeryPoint,
    exchange: &[u8; EXCHANGE_KEY],
    receiver: PublicKey,
) -> Result<LessSafeKey, RelayFault> {
    // A key of small order gives the shared secret zero, which an attacker knows.
    if shared.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(RelayFault::Unreadable);
    }
    let mut salt = Vec::with_capacity(2 * EXCHANGE_KEY);
    salt.extend_from_slice(exchange);
    salt.extend_from_slice(receiver.as_bytes());
    let information = [VERSION, b" ", kind.label()].concat();
    let information = [information.as_slice()];
    let extracted = Salt::new(HKDF_SHA256, &salt).extract(shared.as_bytes());
    let material = extracted
        .expand(&information, &CHACHA20_POLY1305)
        .map_err(|_| RelayFault::Unreadable)?;
    Ok(LessSafeKey::new(UnboundKey::from(material)))
}

fn associated(kind: Kind) -> Vec<u8> {
    [VERSION, b" ", kind.label()].concat()
}

/// Signs `content` with the sender's key and encrypts it to `receiver`.
pub(crate) fn seal<T: Serialize>(
    kind: Kind,
    binding: Binding,
    sender: &DeviceKey,
    receiver: PublicKey,
    content: &T,
) -> Result<Sealed, RelayFault> {
    let content = Zeroizing::new(serde_json::to_vec(content).map_err(|_| RelayFault::Unreadable)?);
    let signature = sender.sign_relay(&signed_text(kind, binding, &content));
    let mut plain = Zeroizing::new(Vec::with_capacity(SIGNATURE + content.len() + 16));
    plain.extend_from_slice(signature.as_bytes());
    plain.extend_from_slice(&content);

    let mut secret = Zeroizing::new([0u8; EXCHANGE_KEY]);
    getrandom::fill(secret.as_mut_slice()).map_err(|_| RelayFault::Randomness)?;
    let exchange = MontgomeryPoint::mul_base_clamped(*secret).to_bytes();
    let shared = exchange_public(receiver)?.mul_clamped(*secret);
    let key = message_key(kind, &shared, &exchange, receiver)?;
    let mut encrypted = plain.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key([0; 12]),
        Aad::from(associated(kind)),
        &mut encrypted,
    )
    .map_err(|_| RelayFault::Unreadable)?;
    let mut sealed = Vec::with_capacity(EXCHANGE_KEY + encrypted.len());
    sealed.extend_from_slice(&exchange);
    sealed.extend_from_slice(&encrypted);
    Sealed::new(sealed).map_err(|_| RelayFault::TooLong)
}

/// Decrypts a message with the receiver's key and checks that `sender` signed it.
pub(crate) fn open<T: for<'de> Deserialize<'de>>(
    kind: Kind,
    binding: Binding,
    receiver: &DeviceKey,
    sender: PublicKey,
    sealed: &Sealed,
) -> Result<T, RelayFault> {
    let bytes = sealed.as_bytes();
    let (exchange, encrypted) = bytes
        .split_first_chunk::<EXCHANGE_KEY>()
        .ok_or(RelayFault::Unreadable)?;
    let shared = MontgomeryPoint(*exchange).mul_clamped(*receiver.exchange_secret());
    let key = message_key(kind, &shared, exchange, receiver.public_key())?;
    let mut plain = Zeroizing::new(encrypted.to_vec());
    let opened = key
        .open_in_place(
            Nonce::assume_unique_for_key([0; 12]),
            Aad::from(associated(kind)),
            &mut plain,
        )
        .map_err(|_| RelayFault::Unreadable)?;
    let (signature, content) = opened
        .split_first_chunk::<SIGNATURE>()
        .ok_or(RelayFault::Unreadable)?;
    let verifying =
        VerifyingKey::from_bytes(sender.as_bytes()).map_err(|_| RelayFault::NotSigned)?;
    verifying
        .verify_strict(
            &signed_text(kind, binding, content),
            &DalekSignature::from_bytes(Signature::from_bytes(*signature).as_bytes()),
        )
        .map_err(|_| RelayFault::NotSigned)?;
    serde_json::from_slice(content).map_err(|_| RelayFault::Unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(new_device: &DeviceKey) -> Binding {
        Binding {
            first: ServerId::generate(),
            new_device: new_device.public_key(),
        }
    }

    fn offer() -> Offer {
        Offer {
            servers: vec![OfferedServer {
                server_id: ServerId::generate(),
                address: ServerAddress::parse("harbor.example").unwrap(),
            }],
        }
    }

    #[test]
    fn a_sealed_message_opens_only_for_its_receiver_and_only_with_its_sender() {
        let (old, new, other) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let binding = binding(&new);
        let offered = offer();
        let sealed = seal(Kind::Offer, binding, &old, new.public_key(), &offered).unwrap();
        let opened: Offer = open(Kind::Offer, binding, &new, old.public_key(), &sealed).unwrap();
        assert_eq!(opened, offered);

        assert_eq!(
            open::<Offer>(Kind::Offer, binding, &other, old.public_key(), &sealed),
            Err(RelayFault::Unreadable)
        );
        assert_eq!(
            open::<Offer>(Kind::Offer, binding, &new, other.public_key(), &sealed),
            Err(RelayFault::NotSigned)
        );
    }

    #[test]
    fn a_message_is_bound_to_its_kind_its_first_server_and_the_new_device() {
        let (old, new) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let binding = binding(&new);
        let sealed = seal(Kind::Offer, binding, &old, new.public_key(), &offer()).unwrap();
        assert!(open::<Offer>(Kind::Answer, binding, &new, old.public_key(), &sealed).is_err());
        let other_server = Binding {
            first: ServerId::generate(),
            ..binding
        };
        assert_eq!(
            open::<Offer>(Kind::Offer, other_server, &new, old.public_key(), &sealed),
            Err(RelayFault::NotSigned)
        );
        let other_device = Binding {
            new_device: old.public_key(),
            ..binding
        };
        assert_eq!(
            open::<Offer>(Kind::Offer, other_device, &new, old.public_key(), &sealed),
            Err(RelayFault::NotSigned)
        );
    }

    #[test]
    fn a_changed_or_cut_message_does_not_open() {
        let (old, new) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let binding = binding(&new);
        let sealed = seal(Kind::Offer, binding, &old, new.public_key(), &offer()).unwrap();
        let mut changed = sealed.as_bytes().to_vec();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        let changed = Sealed::new(changed).unwrap();
        assert_eq!(
            open::<Offer>(Kind::Offer, binding, &new, old.public_key(), &changed),
            Err(RelayFault::Unreadable)
        );
        let cut = Sealed::new(sealed.as_bytes()[..40].to_vec()).unwrap();
        assert_eq!(
            open::<Offer>(Kind::Offer, binding, &new, old.public_key(), &cut),
            Err(RelayFault::Unreadable)
        );
        let small_order =
            Sealed::new([[0u8; 32].as_slice(), &sealed.as_bytes()[32..]].concat()).unwrap();
        assert_eq!(
            open::<Offer>(Kind::Offer, binding, &new, old.public_key(), &small_order),
            Err(RelayFault::Unreadable)
        );
    }

    #[test]
    fn two_seals_of_one_message_differ() {
        let (old, new) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let binding = binding(&new);
        let first = seal(Kind::Offer, binding, &old, new.public_key(), &offer()).unwrap();
        let second = seal(Kind::Offer, binding, &old, new.public_key(), &offer()).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn an_answer_names_each_server_with_its_state() {
        let answer = Answer {
            links: vec![
                AnsweredLink {
                    server_id: ServerId::generate(),
                    answered: Answered::Waiting {
                        code: LinkCode::new("ABCD-EFGH"),
                        public_key: PublicKey::from_bytes([3; 32]),
                    },
                },
                AnsweredLink {
                    server_id: ServerId::generate(),
                    answered: Answered::Failed {
                        reason: "the server cannot be reached".to_owned(),
                    },
                },
            ],
        };
        let text = serde_json::to_value(&answer).unwrap();
        assert_eq!(text["links"][0]["state"], "waiting");
        assert_eq!(text["links"][0]["code"], "ABCD-EFGH");
        assert_eq!(text["links"][1]["state"], "failed");
        assert_eq!(serde_json::from_value::<Answer>(text).unwrap(), answer);
    }
}
