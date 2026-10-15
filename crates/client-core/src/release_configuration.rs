//! Validation shared by the build script and release commands.
use base64::{Engine, prelude::BASE64_STANDARD};

pub fn keys(bytes: &[u8]) -> Result<(), &'static str> {
    let keys: Vec<String> = serde_json::from_slice(bytes).map_err(|_| "invalid update keys")?;
    if keys.len() > 32 {
        return Err("too many update keys");
    }
    for key in keys {
        let bytes: [u8; 32] = BASE64_STANDARD
            .decode(key)
            .map_err(|_| "invalid base64 key")?
            .try_into()
            .map_err(|_| "invalid key size")?;
        ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| "invalid public key")?;
    }
    Ok(())
}

/// Refuses a public key that is in the release set and in the freshness set: its holder
/// would have both roles (ADR 0724).
pub fn distinct_roles(release: &[u8], freshness: &[u8]) -> Result<(), &'static str> {
    let release: Vec<String> =
        serde_json::from_slice(release).map_err(|_| "invalid update keys")?;
    let freshness: Vec<String> =
        serde_json::from_slice(freshness).map_err(|_| "invalid update keys")?;
    let decoded = |text: &String| BASE64_STANDARD.decode(text).unwrap_or_default();
    if release
        .iter()
        .any(|key| freshness.iter().any(|other| decoded(key) == decoded(other)))
    {
        return Err("a key is in the release set and in the freshness set");
    }
    Ok(())
}

pub fn certificate(bytes: &[u8]) -> Result<(), &'static str> {
    let hash = std::str::from_utf8(bytes)
        .map_err(|_| "invalid certificate fingerprint")?
        .trim();
    if !hash.is_empty() && (hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit())) {
        return Err("invalid certificate fingerprint");
    }
    Ok(())
}

pub fn source(source: &str) -> Result<url::Url, &'static str> {
    let url = url::Url::parse(source).map_err(|_| "invalid update source")?;
    let local = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if source.len() > 2048
        || source.chars().any(|c| c.is_control() || c.is_whitespace())
        || !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid update source");
    }
    Ok(url)
}

pub fn identity(required: &str, expected: &str) -> Result<(), &'static str> {
    certificate(expected.as_bytes())?;
    if expected.trim().is_empty() {
        return Err("the project signing certificate is not configured");
    }
    let marker = format!(
        "identifier \"org.junto.app\" and certificate leaf = h\"{}\"",
        expected.trim().to_ascii_lowercase()
    );
    if required.to_ascii_lowercase() != marker {
        return Err("different project signing certificate");
    }
    Ok(())
}

/// `--test-configuration` states that the person knows the build is a test configuration.
/// A product build refuses it, so that the flag cannot change what a product build trusts.
pub fn publication(test_configuration: bool, allow_test: bool) -> Result<(), &'static str> {
    if test_configuration && !allow_test {
        return Err("test configuration requires --test-configuration");
    }
    if allow_test && !test_configuration {
        return Err("--test-configuration is refused by a product build");
    }
    Ok(())
}

pub fn label(version: &str, test_configuration: bool) -> String {
    format!(
        "{version}{}",
        if test_configuration {
            " (test configuration)"
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_build_inputs_are_refused_by_the_build_validator() {
        for bytes in [b"bad".as_slice(), b"{}", b"[\"bad\"]"] {
            assert!(keys(bytes).is_err());
        }
        assert!(keys(b"[]").is_ok());
        for address in [
            "http://example.invalid",
            "https://user@example.invalid",
            "https://example.invalid/#fragment",
            "https://example.invalid/?query",
            "https://example.invalid/ path",
        ] {
            assert!(source(address).is_err());
        }
        assert!(source("https://example.invalid/releases/").is_ok());
        assert!(source("http://127.0.0.1:8000/").is_ok());
        assert!(certificate(b"bad").is_err());
    }
    #[test]
    fn publication_requires_the_project_certificate_and_explicit_test_build_flag() {
        let pin = "a".repeat(40);
        let required = format!("identifier \"org.junto.app\" and certificate leaf = H\"{pin}\"");
        assert!(identity(&required, &pin).is_ok());
        assert_eq!(
            identity(&required, ""),
            Err("the project signing certificate is not configured")
        );
        assert_eq!(
            identity(&required, &"b".repeat(40)),
            Err("different project signing certificate")
        );
        assert!(identity("cdhash unsigned", &pin).is_err());
        assert!(publication(true, false).is_err());
        assert!(publication(true, true).is_ok());
        assert!(publication(false, false).is_ok());
        assert_eq!(
            publication(false, true),
            Err("--test-configuration is refused by a product build")
        );
        assert_eq!(label("1.0.0", true), "1.0.0 (test configuration)");
        assert_eq!(label("1.0.0", false), "1.0.0");
    }

    #[test]
    fn one_key_in_both_trust_sets_is_refused() {
        let first = format!("\"{}\"", BASE64_STANDARD.encode([1u8; 32]));
        let second = format!("\"{}\"", BASE64_STANDARD.encode([2u8; 32]));
        let set = |keys: &[&str]| format!("[{}]", keys.join(","));
        assert_eq!(
            distinct_roles(
                set(&[&first, &second]).as_bytes(),
                set(&[&second]).as_bytes()
            ),
            Err("a key is in the release set and in the freshness set")
        );
        assert!(distinct_roles(set(&[&first]).as_bytes(), set(&[&second]).as_bytes()).is_ok());
        assert!(distinct_roles(b"[]", b"[]").is_ok());
    }

    #[test]
    fn naming_the_pinned_certificate_inside_an_alternative_does_not_pin_the_signer() {
        let pin = "a".repeat(40);
        let required =
            format!("identifier \"org.junto.app\" and (certificate leaf = H\"{pin}\" or true)");
        assert!(identity(&required, &pin).is_err());
    }
}
