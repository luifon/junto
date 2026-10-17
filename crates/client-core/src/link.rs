//! The address of a link in a message, in the form that this device opens (ADR 0714).
//!
//! The application of the system for a link receives the address in its command line: on
//! Windows the shell puts the address in the place of `%1` in the registered command, for
//! example `"program.exe" "%1"`. A quote or a space in the address would end that argument
//! and start others, which the server that sent the address would choose. The opened form
//! has neither: the URL parser normalises the address and its serializer percent-encodes
//! most such characters, and every character that RFC 3986 does not allow in a URI is then
//! percent-encoded too. The serializer keeps quotes and spaces in the part of a mailto address
//! before `?`, and backslashes in its part after `?`; this second step encodes them. Percent-encoding these
//! characters does not change the address: RFC 3986 and RFC 6068 require it.
//!
//! The opened form starts with its scheme, so it cannot start with `-` and be taken as an
//! option of the program.

use std::fmt;

use url::Url;

/// The longest address of a link that the server accepts, in bytes.
pub const LONGEST: usize = 2048;

/// The schemes of the links that a message can hold.
const SCHEMES: [&str; 3] = ["https", "http", "mailto"];

/// An address that this device may open: HTTP, HTTPS or mailto, without a user name and a
/// password, in the form of a URI that holds no quote, space, control character or other
/// character outside RFC 3986.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link(String);

impl Link {
    /// The link of an address that the server accepts: HTTP, HTTPS or mailto, without a
    /// user name and a password, without a control character, at most 2,048 bytes. Nothing
    /// for any other address.
    pub fn parse(address: &str) -> Option<Self> {
        Self::with_schemes(address, &SCHEMES)
    }

    /// A link of any scheme, for a test that registers an application of its own for a
    /// scheme that only it uses.
    #[cfg(feature = "test-support")]
    pub fn of_any_scheme(address: &str) -> Option<Self> {
        let scheme = address.split_once(':')?.0.to_ascii_lowercase();
        Self::with_schemes(address, &[scheme.as_str()])
    }

    fn with_schemes(address: &str, schemes: &[&str]) -> Option<Self> {
        if address.len() > LONGEST || address.chars().any(char::is_control) {
            return None;
        }
        let parsed = Url::parse(address).ok()?;
        if !schemes.contains(&parsed.scheme())
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return None;
        }
        let opened = uri(parsed.as_str());
        // The encoded form is an address of its own; it must stay the same address when it
        // is read again, as the application that opens it reads it.
        let again = Url::parse(&opened).ok()?;
        (again.as_str() == opened && again.scheme() == parsed.scheme()).then_some(Self(opened))
    }

    /// The address as the application of the system receives it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Link {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// True for a character that RFC 3986 allows in a URI as it is: the unreserved and the
/// reserved characters, and `%`, which starts an encoded byte.
fn allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&byte)
}

/// `address` with each byte of every other character percent-encoded.
fn uri(address: &str) -> String {
    let mut encoded = String::with_capacity(address.len());
    for byte in address.bytes() {
        if allowed(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(address: &str) -> Option<String> {
        Link::parse(address).map(|link| link.as_str().to_owned())
    }

    #[test]
    fn only_web_and_mail_addresses_without_credentials_are_links() {
        assert_eq!(
            opened("https://example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            opened("mailto:a@example.com").as_deref(),
            Some("mailto:a@example.com")
        );
        assert_eq!(
            opened("HTTP://Example.COM").as_deref(),
            Some("http://example.com/")
        );
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "smb://host/share",
            "ftp://example.com",
            "https://user:secret@example.com",
            "https://user@example.com",
            "https://example.com/\u{7}",
            "example.com",
            "",
        ] {
            assert_eq!(opened(refused), None, "{refused}");
        }
        assert_eq!(
            opened(&format!("https://example.com/{}", "a".repeat(LONGEST))),
            None
        );
    }

    #[test]
    fn an_address_with_quotes_spaces_and_arguments_opens_as_one_encoded_address() {
        let cases = [
            (
                r#"mailto:a@example.com?body=x" --attach "C:\Users\a b\secret.txt"#,
                "mailto:a@example.com?body=x%22%20--attach%20%22C:%5CUsers%5Ca%20b%5Csecret.txt",
            ),
            (
                r#"mailto:"a b"@example.com"#,
                "mailto:%22a%20b%22@example.com",
            ),
            (
                r#"https://example.com/a b"c?q="x y"#,
                "https://example.com/a%20b%22c?q=%22x%20y",
            ),
            (
                r#"https://example.com/<a>{b}|c^d`e#"f g"<h>"#,
                "https://example.com/%3Ca%3E%7Bb%7D%7Cc%5Ed%60e#%22f%20g%22%3Ch%3E",
            ),
            (
                "https://bücher.example/straße?ä=ö",
                "https://xn--bcher-kva.example/stra%C3%9Fe?%C3%A4=%C3%B6",
            ),
            // Spaces at the ends are not part of the address.
            (" https://example.com/a ", "https://example.com/a"),
        ];
        for (address, expected) in cases {
            let link = Link::parse(address).unwrap_or_else(|| panic!("{address}"));
            assert_eq!(link.as_str(), expected, "{address}");
            assert!(
                link.as_str().bytes().all(allowed),
                "{address} opens as {link}"
            );
        }
    }

    #[test]
    fn a_link_is_opened_in_the_same_form_again() {
        for address in [
            "https://example.com/a%20b?q=%22",
            "mailto:a@example.com?subject=a%20b",
            "https://example.com/%zz",
        ] {
            let link = Link::parse(address).unwrap_or_else(|| panic!("{address}"));
            assert_eq!(Link::parse(link.as_str()), Some(link.clone()), "{address}");
        }
    }
}
