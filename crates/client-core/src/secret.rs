//! Secrets in the memory of the client: session tokens, and the buffers of requests and
//! responses, which can hold a token or a code. Each is overwritten when it is dropped,
//! and no growth of a buffer leaves an older copy behind.
//!
//! The limits: the HTTP library and the secret types of the protocol crate keep copies
//! that this crate cannot overwrite (ADR 0714).

use std::fmt;
use std::io;
use std::ops::Deref;
use std::sync::Arc;

use serde::{Deserialize, Deserializer};
use zeroize::{Zeroize, Zeroizing};

/// A session token. Clones share one buffer, which is overwritten when the last one is
/// dropped. Formatting shows nothing of it.
#[derive(Clone)]
pub struct SecretToken(Arc<Zeroizing<String>>);

impl SecretToken {
    /// Takes the text. The caller keeps no copy.
    pub fn new(text: String) -> Self {
        Self(Arc::new(Zeroizing::new(text)))
    }

    /// Returns the token. The caller must not log it and must not keep a copy.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl PartialEq for SecretToken {
    fn eq(&self, other: &Self) -> bool {
        self.expose() == other.expose()
    }
}

impl Eq for SecretToken {}

impl fmt::Debug for SecretToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretToken(redacted)")
    }
}

/// The longest token that the client accepts from a server.
const LONGEST_TOKEN: usize = 512;

impl<'de> Deserialize<'de> for SecretToken {
    /// A token is read from the text of a response into its own buffer, and from nowhere
    /// else. A token is letters, digits, `_` and `-` (ADR 0109), so its JSON text has no
    /// escape; a string with an escape would pass through a buffer of the JSON reader that
    /// this crate cannot overwrite, and is refused.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Token;

        impl<'de> serde::de::Visitor<'de> for Token {
            type Value = SecretToken;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a token of letters, digits, `_` and `-` without escapes")
            }

            fn visit_borrowed_str<E: serde::de::Error>(
                self,
                text: &'de str,
            ) -> Result<SecretToken, E> {
                let plain = !text.is_empty()
                    && text.len() <= LONGEST_TOKEN
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
                if !plain {
                    return Err(E::invalid_value(
                        serde::de::Unexpected::Other("text"),
                        &self,
                    ));
                }
                let mut owned = String::with_capacity(text.len());
                owned.push_str(text);
                Ok(SecretToken::new(owned))
            }
        }

        deserializer.deserialize_str(Token)
    }
}

/// Bytes of a request or a response. The buffer is overwritten when it is dropped, and a
/// buffer that must grow is copied to a new one and overwritten at once.
#[derive(Default)]
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Zeroizing::new(Vec::with_capacity(capacity)))
    }

    /// Takes a buffer. The caller keeps no copy.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn extend(&mut self, more: &[u8]) {
        let needed = self.0.len() + more.len();
        if needed > self.0.capacity() {
            let mut grown = Vec::with_capacity(needed.max(self.0.capacity() * 2).max(256));
            grown.extend_from_slice(&self.0);
            let mut old = std::mem::replace(&mut *self.0, grown);
            old.zeroize();
        }
        self.0.extend_from_slice(more);
    }
}

impl From<Vec<u8>> for SecretBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_vec(bytes)
    }
}

impl Deref for SecretBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl io::Write for SecretBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SecretBytes({} bytes)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::ZeroizeOnDrop;

    fn overwritten_on_drop<T: ZeroizeOnDrop>() {}

    #[test]
    fn the_buffers_of_tokens_and_messages_are_overwritten_when_they_are_dropped() {
        overwritten_on_drop::<Zeroizing<String>>();
        overwritten_on_drop::<Zeroizing<Vec<u8>>>();
        let token = SecretToken::new("jst_secret".to_owned());
        let Zeroizing { .. }: &Zeroizing<String> = &token.0;
        let bytes = SecretBytes::from_vec(b"secret".to_vec());
        let Zeroizing { .. }: &Zeroizing<Vec<u8>> = &bytes.0;
    }

    #[test]
    fn clones_of_a_token_share_one_buffer() {
        let token = SecretToken::new("jst_secret".to_owned());
        let copy = token.clone();
        assert!(std::ptr::eq(token.expose(), copy.expose()));
        assert_eq!(token, copy);
        assert_ne!(token, SecretToken::new("jst_other".to_owned()));
        assert_eq!(format!("{token:?}"), "SecretToken(redacted)");
    }

    #[derive(Deserialize)]
    struct Wire {
        token: SecretToken,
    }

    #[test]
    fn a_token_is_read_from_a_response_into_its_own_buffer() {
        let response = br#"{"token":"jst_secret-value_09"}"#.to_vec();
        let wire: Wire = serde_json::from_slice(&response).unwrap();
        assert_eq!(wire.token.expose(), "jst_secret-value_09");
        assert!(
            !response
                .as_ptr_range()
                .contains(&wire.token.expose().as_ptr())
        );
    }

    #[test]
    fn a_token_with_an_escape_or_another_character_is_refused() {
        let escaped = "{\"token\":\"jst_\\u0041bc\"}";
        assert!(escaped.contains("\\u0041"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(escaped).unwrap()["token"],
            "jst_Abc"
        );
        assert!(serde_json::from_slice::<Wire>(escaped.as_bytes()).is_err());

        let long = format!("{{\"token\":\"{}\"}}", "a".repeat(513));
        for refused in [
            "{\"token\":\"\"}",
            "{\"token\":\"jst_a b\"}",
            "{\"token\":\"jst_a\\nb\"}",
            "{\"token\":\"jst_\u{e9}\"}",
            "{\"token\":7}",
            long.as_str(),
        ] {
            assert!(
                serde_json::from_slice::<Wire>(refused.as_bytes()).is_err(),
                "{refused}"
            );
        }
        let longest = format!("{{\"token\":\"{}\"}}", "a".repeat(512));
        assert!(serde_json::from_slice::<Wire>(longest.as_bytes()).is_ok());
    }

    #[test]
    fn a_buffer_that_grows_keeps_its_content_and_moves_to_a_new_buffer() {
        let mut bytes = SecretBytes::with_capacity(4);
        let first = bytes.as_slice().as_ptr();
        bytes.extend(b"abcd");
        assert_eq!(bytes.as_slice().as_ptr(), first);
        let mut expected = b"abcd".to_vec();
        for round in 0..200u8 {
            bytes.extend(&[round; 37]);
            expected.extend_from_slice(&[round; 37]);
        }
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert_ne!(bytes.as_slice().as_ptr(), first);
        assert_eq!(
            format!("{bytes:?}"),
            format!("SecretBytes({} bytes)", expected.len())
        );

        use std::io::Write;
        let mut written = SecretBytes::default();
        serde_json::to_writer(&mut written, &serde_json::json!({"code": "ABCD"})).unwrap();
        written.flush().unwrap();
        assert_eq!(&*written, br#"{"code":"ABCD"}"#);
    }
}
