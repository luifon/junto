//! The relay of a device link on the first server (ADR 0104, ADR 0716): the two devices of a
//! link exchange the list of the other servers and the links that the new device asked
//! them for. The server keeps each message as opaque bytes; the devices seal it.

use std::borrow::Cow;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;

use crate::ids::DeviceId;
use crate::keys::PublicKey;

/// The most bytes of one sealed message.
pub const SEALED_MAX_BYTES: usize = 64 * 1024;

/// A message that one device of a link sealed for the other one: signed by the sender and
/// encrypted to the receiver. Unpadded base64url on the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed(Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a sealed message is empty, longer than 64 KiB or not unpadded base64url")]
pub struct InvalidSealed;

impl Sealed {
    pub fn new(bytes: Vec<u8>) -> Result<Self, InvalidSealed> {
        if bytes.is_empty() || bytes.len() > SEALED_MAX_BYTES {
            return Err(InvalidSealed);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Sealed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Sealed({} bytes)", self.0.len())
    }
}

impl Serialize for Sealed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Sealed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        if text.len() > SEALED_MAX_BYTES.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom(InvalidSealed));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(text.as_bytes())
            .map_err(|_| serde::de::Error::custom(InvalidSealed))?;
        Self::new(bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for Sealed {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .description(Some(
                "1 to 65536 bytes in base64url without padding, sealed by the sending device",
            ))
            .min_length(Some(2))
            .max_length(Some(SEALED_MAX_BYTES.div_ceil(3) * 4))
            .pattern(Some("^[A-Za-z0-9_-]+$"))
            .into()
    }
}

#[cfg(feature = "openapi")]
impl utoipa::ToSchema for Sealed {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("Sealed")
    }
}

/// What the new device reads: the offer of the device that approved it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RelayOffer {
    /// The device that approved this one; its key on this server sealed the offer.
    pub old_device_id: DeviceId,
    pub old_public_key: PublicKey,
    pub offer: Sealed,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// What the new device writes: its answer to the offer, which replaces an earlier one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AnswerRelay {
    pub answer: Sealed,
}

/// What the approving device reads: the answer of the new device, when it wrote one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RelayAnswer {
    pub new_device_id: DeviceId,
    pub new_public_key: PublicKey,
    pub answer: Option<Sealed>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_message_is_bounded_unpadded_base64url() {
        let sealed = Sealed::new(vec![1, 2, 3, 250]).unwrap();
        let text = serde_json::to_string(&sealed).unwrap();
        assert_eq!(text, "\"AQID-g\"");
        assert_eq!(serde_json::from_str::<Sealed>(&text).unwrap(), sealed);
        assert!(Sealed::new(Vec::new()).is_err());
        assert!(Sealed::new(vec![0; SEALED_MAX_BYTES + 1]).is_err());
        assert!(Sealed::new(vec![0; SEALED_MAX_BYTES]).is_ok());
        assert!(serde_json::from_str::<Sealed>("\"AQID+g==\"").is_err());
        let longest = URL_SAFE_NO_PAD.encode(vec![0; SEALED_MAX_BYTES + 1]);
        assert!(serde_json::from_value::<Sealed>(serde_json::Value::String(longest)).is_err());
        assert_eq!(format!("{sealed:?}"), "Sealed(4 bytes)");
    }
}
