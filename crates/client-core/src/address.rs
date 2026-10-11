//! The address of a server as a person types it and as the client stores it (ADR 0004).

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use junto_protocol::origin::Origin;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::{Host, Url};

const LONGEST_ADDRESS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("the address is empty")]
    Empty,
    #[error("the address is longer than 255 characters")]
    TooLong,
    #[error("the address is not a host name")]
    Malformed,
    #[error("a server address starts with https://")]
    Scheme,
    #[error("a server address has no user name, path, query or fragment")]
    ExtraParts,
    #[error("an IP address is accepted only for a server on this machine")]
    IpNotLocal,
    #[error("a connection without TLS is accepted only for a server on this machine")]
    InsecureNotLocal,
}

/// The origin of one server: scheme, host and port. Every request of the client goes to
/// this origin, and a device signs it in every proof (ADR 0117).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ServerAddress {
    origin: Origin,
    /// The same origin as a URL with the path `/`. Copies of an address share it.
    url: Arc<Url>,
}

impl ServerAddress {
    /// Reads an address. A text without a scheme gets `https://`. `http://` and an IP
    /// address are accepted only for the loopback interface, for local development.
    pub fn parse(text: &str) -> Result<Self, AddressError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AddressError::Empty);
        }
        if text.len() > LONGEST_ADDRESS {
            return Err(AddressError::TooLong);
        }
        if text
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        {
            return Err(AddressError::Malformed);
        }
        let with_scheme = if text.contains("://") {
            text.to_owned()
        } else {
            format!("https://{text}")
        };
        let url = Url::parse(&with_scheme).map_err(|_| AddressError::Malformed)?;
        let secure = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(AddressError::Scheme),
        };
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(AddressError::ExtraParts);
        }
        let is_ip = match url.host() {
            Some(Host::Domain(_)) => false,
            Some(Host::Ipv4(_) | Host::Ipv6(_)) => true,
            None => return Err(AddressError::Malformed),
        };
        let mut url = url;
        url.set_path("/");
        // The address has the one spelling of an origin, so the text that is stored, the
        // text that a device signs and the place that a request goes to are the same.
        let text = url.as_str().trim_end_matches('/');
        let origin = Origin::parse(text).map_err(|_| AddressError::Malformed)?;
        if origin.as_str() != text {
            return Err(AddressError::Malformed);
        }
        if is_ip && !origin.is_loopback() {
            return Err(AddressError::IpNotLocal);
        }
        if !secure && !origin.is_loopback() {
            return Err(AddressError::InsecureNotLocal);
        }
        Ok(Self {
            origin,
            url: Arc::new(url),
        })
    }

    /// The stored and shown form, without a slash at the end.
    pub fn as_str(&self) -> &str {
        self.origin.as_str()
    }

    /// The origin that a device signs when it proves its key to this address (ADR 0117).
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn host(&self) -> &str {
        self.url.host_str().unwrap_or_default()
    }

    pub fn is_local(&self) -> bool {
        self.origin.is_loopback()
    }

    /// The URL of an API path on this server. `path` is built by the client from a fixed
    /// text and typed identifiers.
    pub(crate) fn endpoint(&self, path: &str) -> Url {
        let mut url = Url::clone(&self.url);
        url.set_path(path);
        url
    }

    /// The URL of the logo that the server names, when the path stays on this server.
    pub fn logo_url(&self, logo_path: &str) -> Option<Url> {
        let plain = logo_path.starts_with('/')
            && !logo_path.starts_with("//")
            && !logo_path
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\');
        if !plain {
            return None;
        }
        let url = self.url.join(logo_path).ok()?;
        (url.origin() == self.url.origin()).then_some(url)
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for ServerAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ServerAddress({})", self.as_str())
    }
}

impl FromStr for ServerAddress {
    type Err = AddressError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for ServerAddress {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ServerAddress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_name_gets_https_and_a_stable_form() {
        for text in [
            "chat.example.test",
            "  chat.example.test  ",
            "https://chat.example.test",
            "https://chat.example.test/",
            "HTTPS://Chat.Example.Test:443/",
        ] {
            let address = ServerAddress::parse(text).unwrap();
            assert_eq!(address.as_str(), "https://chat.example.test", "{text}");
            assert_eq!(address.host(), "chat.example.test");
            assert!(!address.is_local());
        }
        assert_eq!(
            ServerAddress::parse("chat.example.test:8443")
                .unwrap()
                .as_str(),
            "https://chat.example.test:8443"
        );
    }

    #[test]
    fn plain_http_and_ip_addresses_are_for_this_machine_only() {
        for text in [
            "http://127.0.0.1:8080",
            "http://localhost:8080",
            "http://[::1]:8080",
            "https://127.0.0.1:8443",
        ] {
            assert!(ServerAddress::parse(text).unwrap().is_local(), "{text}");
        }
        assert_eq!(
            ServerAddress::parse("http://chat.example.test"),
            Err(AddressError::InsecureNotLocal)
        );
        for text in [
            "https://192.0.2.10",
            "http://192.0.2.10:8080",
            "https://[2001:db8::1]",
            "198.51.100.7",
        ] {
            assert_eq!(
                ServerAddress::parse(text),
                Err(AddressError::IpNotLocal),
                "{text}"
            );
        }
    }

    #[test]
    fn other_forms_are_refused() {
        let cases = [
            ("", AddressError::Empty),
            ("   ", AddressError::Empty),
            ("ftp://chat.example.test", AddressError::Scheme),
            ("file:///etc/hosts", AddressError::Scheme),
            ("https://user@chat.example.test", AddressError::ExtraParts),
            (
                "https://user:pw@chat.example.test",
                AddressError::ExtraParts,
            ),
            ("https://chat.example.test/api", AddressError::ExtraParts),
            ("https://chat.example.test/?a=1", AddressError::ExtraParts),
            ("https://chat.example.test/#top", AddressError::ExtraParts),
            ("chat example test", AddressError::Malformed),
            ("https://", AddressError::Malformed),
            ("chat.example.test\\evil", AddressError::Malformed),
        ];
        for (text, expected) in cases {
            assert_eq!(ServerAddress::parse(text), Err(expected), "{text:?}");
        }
        assert_eq!(
            ServerAddress::parse(&format!("{}.example.test", "a".repeat(250))),
            Err(AddressError::TooLong)
        );
    }

    #[test]
    fn an_address_is_an_origin_in_its_one_spelling() {
        for (text, origin) in [
            ("Chat.Example.Test:443", "https://chat.example.test"),
            ("chat.example.test:8443", "https://chat.example.test:8443"),
            ("http://LOCALHOST:80/", "http://localhost"),
            ("http://127.1:8080", "http://127.0.0.1:8080"),
            ("http://[0:0:0:0:0:0:0:1]:8080", "http://[::1]:8080"),
        ] {
            let address = ServerAddress::parse(text).unwrap();
            assert_eq!(address.origin().as_str(), origin, "{text}");
            assert_eq!(address.as_str(), origin);
            assert_eq!(address.endpoint("/").as_str(), format!("{origin}/"));
            assert_eq!(address.is_local(), address.origin().is_loopback());
            assert_eq!(Origin::parse(origin).unwrap(), *address.origin());
        }
        for text in [
            "chat.example.test.",
            "chat_room.example.test",
            "-chat.example.test",
            "chat.example.999",
        ] {
            assert_eq!(
                ServerAddress::parse(text),
                Err(AddressError::Malformed),
                "{text}"
            );
        }
    }

    #[test]
    fn an_endpoint_stays_on_the_origin() {
        let address = ServerAddress::parse("https://chat.example.test:8443").unwrap();
        assert_eq!(
            address.endpoint("/api/v1/account").as_str(),
            "https://chat.example.test:8443/api/v1/account"
        );
        assert_eq!(
            address.endpoint("/api/server").origin(),
            address.url.origin()
        );
    }

    #[test]
    fn a_logo_path_cannot_leave_the_server() {
        let address = ServerAddress::parse("https://chat.example.test").unwrap();
        assert_eq!(
            address.logo_url("/api/v1/logo").unwrap().as_str(),
            "https://chat.example.test/api/v1/logo"
        );
        for path in [
            "//other.example.test/logo.png",
            "https://other.example.test/logo.png",
            "logo.png",
            "/\\other.example.test/logo.png",
            "/logo .png",
            "/logo\n.png",
            "",
        ] {
            assert!(address.logo_url(path).is_none(), "{path:?}");
        }
    }

    #[test]
    fn the_stored_form_is_read_back_and_checked() {
        let address = ServerAddress::parse("chat.example.test").unwrap();
        let json = serde_json::to_string(&address).unwrap();
        assert_eq!(json, "\"https://chat.example.test\"");
        assert_eq!(
            serde_json::from_str::<ServerAddress>(&json).unwrap(),
            address
        );
        assert!(serde_json::from_str::<ServerAddress>("\"http://chat.example.test\"").is_err());
    }
}
