use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidOrigin {
    #[error("an origin is written as scheme://host or scheme://host:port")]
    Form,
    #[error("the scheme of an origin is http or https")]
    Scheme,
    #[error("the host of an origin is a domain name or an IP address")]
    Host,
    #[error("the port of an origin is a number from 1 to 65535")]
    Port,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    const fn name(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

/// The scheme, the host and the port of the address at which a client reaches a server, for
/// example `https://chat.example.test` or `http://localhost:8080`.
///
/// It has one spelling: lower case, no path and no trailing slash, and no port when the port
/// is the default of the scheme. A device signs the origin that it connected to (ADR 0108).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    text: String,
    scheme: Scheme,
    loopback: bool,
}

const LONGEST_HOST: usize = 253;
const LONGEST_LABEL: usize = 63;

fn is_domain_name(host: &str) -> bool {
    host.len() <= LONGEST_HOST
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= LONGEST_LABEL
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        && !host
            .rsplit('.')
            .next()
            .is_some_and(|last| last.bytes().all(|byte| byte.is_ascii_digit()))
}

impl Origin {
    pub fn parse(text: &str) -> Result<Self, InvalidOrigin> {
        let lower = text.to_ascii_lowercase();
        let (scheme, rest) = lower.split_once("://").ok_or(InvalidOrigin::Form)?;
        let scheme = match scheme {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            _ => return Err(InvalidOrigin::Scheme),
        };
        let (host, port) = match rest.strip_prefix('[') {
            Some(bracketed) => {
                let (address, after) = bracketed.split_once(']').ok_or(InvalidOrigin::Host)?;
                let port = match after {
                    "" => None,
                    after => Some(after.strip_prefix(':').ok_or(InvalidOrigin::Form)?),
                };
                (format!("[{address}]"), port)
            }
            None => match rest.split_once(':') {
                Some((host, port)) => (host.to_owned(), Some(port)),
                None => (rest.to_owned(), None),
            },
        };
        let port = match port {
            None => None,
            Some(digits) => {
                let plain = !digits.is_empty()
                    && digits.len() <= 5
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                    && !digits.starts_with('0');
                let port: u16 = digits
                    .parse()
                    .ok()
                    .filter(|_| plain)
                    .ok_or(InvalidOrigin::Port)?;
                (port != scheme.default_port()).then_some(port)
            }
        };
        let (host, loopback) = if let Some(address) = host
            .strip_prefix('[')
            .and_then(|address| address.strip_suffix(']'))
        {
            let address: Ipv6Addr = address.parse().map_err(|_| InvalidOrigin::Host)?;
            (format!("[{address}]"), address.is_loopback())
        } else if let Ok(address) = host.parse::<Ipv4Addr>() {
            (address.to_string(), address.is_loopback())
        } else if is_domain_name(&host) {
            let loopback = host == "localhost";
            (host, loopback)
        } else {
            return Err(InvalidOrigin::Host);
        };
        let text = match port {
            Some(port) => format!("{}://{host}:{port}", scheme.name()),
            None => format!("{}://{host}", scheme.name()),
        };
        Ok(Self {
            text,
            scheme,
            loopback,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub const fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// True for `localhost`, an address in 127.0.0.0/8 and `[::1]`: an address that only a
    /// program on the same machine reaches.
    pub const fn is_loopback(&self) -> bool {
        self.loopback
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl fmt::Debug for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Origin({})", self.text)
    }
}

impl FromStr for Origin {
    type Err = InvalidOrigin;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for Origin {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for Origin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for Origin {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .description(Some(
                "The scheme, the host and the port of a server address, without a path: \
                 `https://chat.example.test`",
            ))
            .into()
    }
}

#[cfg(feature = "openapi")]
impl utoipa::ToSchema for Origin {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Origin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_has_one_spelling() {
        for (written, normal) in [
            ("https://chat.example.test", "https://chat.example.test"),
            ("HTTPS://Chat.Example.TEST", "https://chat.example.test"),
            ("https://chat.example.test:443", "https://chat.example.test"),
            (
                "https://chat.example.test:8443",
                "https://chat.example.test:8443",
            ),
            ("http://localhost:80", "http://localhost"),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://[::1]:8080", "http://[::1]:8080"),
            ("https://[2001:DB8:0:0:0:0:0:1]", "https://[2001:db8::1]"),
        ] {
            let origin = Origin::parse(written).unwrap();
            assert_eq!(origin.as_str(), normal, "{written}");
            assert_eq!(Origin::parse(normal).unwrap(), origin);
            assert_eq!(
                serde_json::to_value(&origin).unwrap(),
                serde_json::Value::from(normal)
            );
        }
        let read: Origin = serde_json::from_str("\"https://Chat.example.test:443\"").unwrap();
        assert_eq!(read.to_string(), "https://chat.example.test");
    }

    #[test]
    fn an_origin_has_no_path_no_user_and_a_known_scheme() {
        for (written, error) in [
            ("chat.example.test", InvalidOrigin::Form),
            ("https://chat.example.test/", InvalidOrigin::Host),
            ("https://chat.example.test/api", InvalidOrigin::Host),
            ("https://user@chat.example.test", InvalidOrigin::Host),
            ("https://chat.example.test?x=1", InvalidOrigin::Host),
            ("https://", InvalidOrigin::Host),
            ("https://chat..example.test", InvalidOrigin::Host),
            ("https://-chat.example.test", InvalidOrigin::Host),
            ("https://chat.example.999", InvalidOrigin::Host),
            ("https://256.1.1.1", InvalidOrigin::Host),
            ("https://[::1", InvalidOrigin::Host),
            ("https://[::1]8080", InvalidOrigin::Form),
            ("https://chat.example.test:", InvalidOrigin::Port),
            ("https://chat.example.test:0", InvalidOrigin::Port),
            ("https://chat.example.test:08080", InvalidOrigin::Port),
            ("https://chat.example.test:65536", InvalidOrigin::Port),
            ("https://chat.example.test:80:80", InvalidOrigin::Port),
            ("wss://chat.example.test", InvalidOrigin::Scheme),
            ("ftp://chat.example.test", InvalidOrigin::Scheme),
        ] {
            assert_eq!(Origin::parse(written), Err(error), "{written}");
        }
        assert!(serde_json::from_str::<Origin>("\"chat.example.test\"").is_err());
    }

    #[test]
    fn loopback_origins_are_told_apart() {
        for loopback in [
            "http://localhost:8080",
            "http://127.0.0.1",
            "https://127.8.9.10:1",
            "http://[::1]:9",
        ] {
            assert!(Origin::parse(loopback).unwrap().is_loopback(), "{loopback}");
        }
        for remote in [
            "https://chat.example.test",
            "http://localhost.example.test",
            "http://192.0.2.1",
            "http://[2001:db8::1]",
            "http://0.0.0.0",
        ] {
            assert!(!Origin::parse(remote).unwrap().is_loopback(), "{remote}");
        }
        assert_eq!(
            Origin::parse("http://localhost").unwrap().scheme(),
            Scheme::Http
        );
    }
}
