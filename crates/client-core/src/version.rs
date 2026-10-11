//! Which API version the client uses with one server (ADR 0201, ADR 0713).

use std::fmt;

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
