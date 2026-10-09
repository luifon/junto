use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::{PartialSchema, ToSchema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected} bytes in unpadded base64url")]
pub struct InvalidByteString {
    expected: usize,
}

macro_rules! byte_string {
    ($(#[$meta:meta])* $name:ident, $length:literal, $encoded_length:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name([u8; $length]);

        impl $name {
            pub const LENGTH: usize = $length;

            pub const fn from_bytes(bytes: [u8; $length]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; $length] {
                &self.0
            }
        }

        impl TryFrom<&[u8]> for $name {
            type Error = InvalidByteString;

            fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
                bytes
                    .try_into()
                    .map(Self)
                    .map_err(|_| InvalidByteString { expected: $length })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&URL_SAFE_NO_PAD.encode(self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, concat!(stringify!($name), "({})"), self)
            }
        }

        impl FromStr for $name {
            type Err = InvalidByteString;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                let invalid = InvalidByteString { expected: $length };
                let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| invalid)?;
                Self::try_from(bytes.as_slice())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = Cow::<str>::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }

        impl PartialSchema for $name {
            fn schema() -> RefOr<Schema> {
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .description(Some(concat!(
                        stringify!($length),
                        " bytes in base64url without padding"
                    )))
                    .min_length(Some($encoded_length))
                    .max_length(Some($encoded_length))
                    .pattern(Some("^[A-Za-z0-9_-]+$"))
                    .into()
            }
        }

        impl ToSchema for $name {
            fn name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }
        }
    };
}

byte_string!(
    /// The Ed25519 public key of one device on one server.
    PublicKey,
    32,
    43
);
byte_string!(
    /// An Ed25519 signature.
    Signature,
    64,
    86
);
byte_string!(
    /// The random part of a challenge.
    Nonce,
    32,
    43
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_string_is_unpadded_base64url_on_the_wire() {
        let key = PublicKey::from_bytes([0xfb; 32]);
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json.len(), 43 + 2);
        assert!(!json.contains('='));
        assert!(!json.contains('+'));
        assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), key);
    }

    #[test]
    fn byte_string_refuses_other_lengths_and_alphabets() {
        let short = URL_SAFE_NO_PAD.encode([1u8; 31]);
        let long = URL_SAFE_NO_PAD.encode([1u8; 33]);
        let padded = format!("{}=", URL_SAFE_NO_PAD.encode([1u8; 32]));
        for text in [
            short.as_str(),
            long.as_str(),
            padded.as_str(),
            "",
            "not base64!",
        ] {
            assert!(text.parse::<PublicKey>().is_err(), "{text}");
        }
        assert!(
            URL_SAFE_NO_PAD
                .encode([1u8; 64])
                .parse::<Signature>()
                .is_ok()
        );
    }
}
