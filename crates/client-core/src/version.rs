//! Which API version the client uses with one server (ADR 0201, ADR 0713), and the release
//! version that a server states (ADR 0780).

use std::fmt;
use std::sync::Arc;

use crate::updates::Version;

/// The API version that the client uses with one server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ApiVersion(u32);

impl ApiVersion {
    pub const fn number(self) -> u32 {
        self.0
    }

    /// The route prefix of this version.
    pub fn prefix(self) -> String {
        format!("/api/v{}", self.0)
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "v{}", self.0)
    }
}

/// Why a server cannot be used. Only that server is affected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Incompatibility {
    #[error("the server needs a newer client: its oldest API version is {server_oldest}")]
    ClientTooOld { server_oldest: u32 },
    #[error("the server is too old for this client: its newest API version is {server_newest}")]
    ServerTooOld { server_newest: u32 },
    #[error("the server and this client have no API version in common")]
    NoCommonVersion,
    #[error("the server reports no API version")]
    NoVersions,
}

/// The versions that a client speaks: the current one and the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedVersions {
    current: u32,
    previous: Option<u32>,
}

impl SupportedVersions {
    /// The versions of this build. Version 1 has no version before it.
    pub const THIS_BUILD: Self = Self {
        current: junto_protocol::API_VERSION,
        previous: version_before(junto_protocol::API_VERSION),
    };

    pub const fn new(current: u32, previous: Option<u32>) -> Self {
        Self { current, previous }
    }

    pub const fn current(&self) -> u32 {
        self.current
    }

    pub const fn previous(&self) -> Option<u32> {
        self.previous
    }

    /// The current version when the server supports it, the previous one otherwise.
    pub fn negotiate(&self, server: &[u32]) -> Result<ApiVersion, Incompatibility> {
        if server.contains(&self.current) {
            return Ok(ApiVersion(self.current));
        }
        if let Some(previous) = self.previous
            && server.contains(&previous)
        {
            return Ok(ApiVersion(previous));
        }
        let (Some(server_oldest), Some(server_newest)) =
            (server.iter().copied().min(), server.iter().copied().max())
        else {
            return Err(Incompatibility::NoVersions);
        };
        let client_oldest = self.previous.unwrap_or(self.current);
        if server_oldest > self.current {
            Err(Incompatibility::ClientTooOld { server_oldest })
        } else if server_newest < client_oldest {
            Err(Incompatibility::ServerTooOld { server_newest })
        } else {
            Err(Incompatibility::NoCommonVersion)
        }
    }
}

const fn version_before(version: u32) -> Option<u32> {
    if version > 1 { Some(version - 1) } else { None }
}

/// The longest release version of a server that the client keeps, in bytes.
const LONGEST_SERVER_VERSION: usize = 64;

/// The release version that a server states in the `hello` of its event stream: a semantic
/// version, such as `0.1.0` or `0.1.0-test.2`. A clone copies a pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerVersion(Arc<semver::Version>);

impl ServerVersion {
    /// `None` for a text of more than 64 bytes and for a text that is no semantic version.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > LONGEST_SERVER_VERSION {
            return None;
        }
        semver::Version::parse(text)
            .ok()
            .map(|version| Self(Arc::new(version)))
    }

    /// Whether a release of the version `release` comes after the version of the server
    /// in the order of semantic versions: a pre-release comes before the release of its
    /// three numbers, and build metadata has no place in the order.
    pub fn is_older_than(&self, release: Version) -> bool {
        let server = &*self.0;
        let release = semver::Version::new(
            u64::from(release.major()),
            u64::from(release.minor()),
            u64::from(release.patch()),
        );
        (server.major, server.minor, server.patch, &server.pre)
            < (release.major, release.minor, release.patch, &release.pre)
    }
}

impl fmt::Display for ServerVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_build_speaks_version_one_only() {
        assert_eq!(SupportedVersions::THIS_BUILD.current(), 1);
        assert_eq!(SupportedVersions::THIS_BUILD.previous(), None);
        assert_eq!(
            SupportedVersions::THIS_BUILD
                .negotiate(&[1])
                .unwrap()
                .prefix(),
            "/api/v1"
        );
    }

    #[test]
    fn the_current_version_is_preferred_and_the_previous_one_is_accepted() {
        let client = SupportedVersions::new(2, Some(1));
        assert_eq!(client.negotiate(&[1, 2]).unwrap().number(), 2);
        assert_eq!(client.negotiate(&[2, 3]).unwrap().number(), 2);
        assert_eq!(client.negotiate(&[1]).unwrap().number(), 1);
        assert_eq!(client.negotiate(&[1]).unwrap().prefix(), "/api/v1");
        assert_eq!(client.negotiate(&[2, 1, 2]).unwrap().to_string(), "v2");
    }

    #[test]
    fn a_server_states_a_semantic_version_and_a_release_after_it_is_newer() {
        let version = |text: &str| ServerVersion::parse(text).unwrap();
        assert_eq!(version("0.1.0").to_string(), "0.1.0");
        assert_eq!(version("0.1.0-test.2").to_string(), "0.1.0-test.2");
        assert_eq!(version("1.2.3+build.5").to_string(), "1.2.3+build.5");
        let longest = format!("0.1.0-{}", "a".repeat(LONGEST_SERVER_VERSION - 6));
        assert_eq!(version(&longest).to_string(), longest);
        let too_long = format!("{longest}a");
        for text in [
            "",
            "0.1",
            "0.1.0.0",
            "v0.1.0",
            "01.1.0",
            "0.1.0-",
            "0.1.0 ",
            "Junto",
            too_long.as_str(),
        ] {
            assert_eq!(ServerVersion::parse(text), None, "{text:?}");
        }

        let release = Version::new(0, 1, 0);
        for older in ["0.0.9", "0.1.0-test.2", "0.1.0-alpha", "0.1.0-0"] {
            assert!(version(older).is_older_than(release), "{older}");
        }
        for not_older in ["0.1.0", "0.1.0+build.7", "0.1.1-test.1", "0.1.1", "1.0.0"] {
            assert!(!version(not_older).is_older_than(release), "{not_older}");
        }
        assert!(version("0.1.0-test.10").is_older_than(Version::new(0, 1, 1)));
        assert!(version("0.9.9").is_older_than(Version::new(0, 10, 0)));
    }

    #[test]
    fn a_server_two_versions_away_cannot_be_used() {
        let client = SupportedVersions::new(3, Some(2));
        assert_eq!(
            client.negotiate(&[1]),
            Err(Incompatibility::ServerTooOld { server_newest: 1 })
        );
        assert_eq!(
            client.negotiate(&[4, 5]),
            Err(Incompatibility::ClientTooOld { server_oldest: 4 })
        );
        assert_eq!(client.negotiate(&[]), Err(Incompatibility::NoVersions));
        assert_eq!(
            SupportedVersions::new(4, Some(2)).negotiate(&[3]),
            Err(Incompatibility::NoCommonVersion)
        );
        assert_eq!(
            SupportedVersions::new(1, None).negotiate(&[2]),
            Err(Incompatibility::ClientTooOld { server_oldest: 2 })
        );
    }
}
