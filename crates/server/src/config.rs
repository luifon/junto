use std::net::{IpAddr, SocketAddr};

use time::Duration;

use crate::database::Timeouts;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("the environment variable {0} is not set")]
    Missing(&'static str),
    #[error("the environment variable {name} has an invalid value: {reason}")]
    Invalid { name: &'static str, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Text,
}

/// The settings that an operator gives through environment variables.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub listen_address: SocketAddr,
    pub server_name: String,
    pub trusted_proxies: Vec<IpAddr>,
    pub log_format: LogFormat,
    pub log_level: tracing::Level,
    pub limits: Limits,
}

/// The numbers that ADR 0108, 0109, 0111, 0112 and 0114 fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub challenge_lifetime: Duration,
    pub session_lifetime: Duration,
    pub invite_default_lifetime: Duration,
    pub invite_longest_lifetime: Duration,
    pub invite_most_uses: u32,
    pub device_link_lifetime: Duration,
    pub join_request_lifetime: Duration,
    pub most_pending_join_requests: u32,
    pub join_request_history: Duration,
    pub join_attempts: RateLimit,
    pub challenge_requests: RateLimit,
    pub wrong_signatures: RateLimit,
    pub redemptions: RateLimit,
    pub limiter_entries: usize,
    pub most_outstanding_challenges: u32,
    pub most_pending_device_links: u32,
    pub owner_code_lifetime: Duration,
    pub request_timeout: Duration,
    pub database: Timeouts,
    pub maintenance_batch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub attempts: u32,
    pub window: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            challenge_lifetime: Duration::seconds(60),
            session_lifetime: Duration::days(30),
            invite_default_lifetime: Duration::days(7),
            invite_longest_lifetime: Duration::days(365),
            invite_most_uses: 10_000,
            device_link_lifetime: Duration::minutes(10),
            join_request_lifetime: Duration::days(7),
            most_pending_join_requests: 200,
            join_request_history: Duration::days(30),
            join_attempts: RateLimit {
                attempts: 20,
                window: Duration::minutes(10),
            },
            challenge_requests: RateLimit {
                attempts: 120,
                window: Duration::minutes(1),
            },
            wrong_signatures: RateLimit {
                attempts: 10,
                window: Duration::minutes(10),
            },
            redemptions: RateLimit {
                attempts: 10,
                window: Duration::minutes(10),
            },
            limiter_entries: 100_000,
            most_outstanding_challenges: 10_000,
            most_pending_device_links: 500,
            owner_code_lifetime: Duration::hours(1),
            request_timeout: Duration::seconds(30),
            database: Timeouts {
                statement: Duration::seconds(10),
                lock: Duration::seconds(5),
                idle_in_transaction: Duration::seconds(15),
            },
            maintenance_batch: 500,
        }
    }
}

impl Config {
    pub const DATABASE_URL: &'static str = "JUNTO_DATABASE_URL";
    pub const LISTEN_ADDRESS: &'static str = "JUNTO_LISTEN_ADDRESS";
    pub const SERVER_NAME: &'static str = "JUNTO_SERVER_NAME";
    pub const TRUSTED_PROXIES: &'static str = "JUNTO_TRUSTED_PROXIES";
    pub const LOG_FORMAT: &'static str = "JUNTO_LOG_FORMAT";
    pub const LOG_LEVEL: &'static str = "JUNTO_LOG_LEVEL";

    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(
        lookup: impl Fn(&'static str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let value = |name| lookup(name).filter(|text| !text.trim().is_empty());
        let invalid = |name, reason: String| ConfigError::Invalid { name, reason };

        let database_url =
            value(Self::DATABASE_URL).ok_or(ConfigError::Missing(Self::DATABASE_URL))?;

        let listen_address = match value(Self::LISTEN_ADDRESS) {
            Some(text) => text
                .trim()
                .parse()
                .map_err(|error: std::net::AddrParseError| {
                    invalid(Self::LISTEN_ADDRESS, error.to_string())
                })?,
            None => SocketAddr::from(([127, 0, 0, 1], 8080)),
        };

        let server_name = value(Self::SERVER_NAME)
            .map(|text| text.trim().to_owned())
            .unwrap_or_else(|| "Junto".to_owned());
        if server_name.chars().count() > 64 {
            return Err(invalid(
                Self::SERVER_NAME,
                "longer than 64 characters".to_owned(),
            ));
        }

        let trusted_proxies = match value(Self::TRUSTED_PROXIES) {
            Some(text) => text
                .split(',')
                .map(|part| {
                    part.trim().parse::<IpAddr>().map_err(|error| {
                        invalid(Self::TRUSTED_PROXIES, format!("{part:?}: {error}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };

        let log_format = match value(Self::LOG_FORMAT).as_deref().map(str::trim) {
            None | Some("json") => LogFormat::Json,
            Some("text") => LogFormat::Text,
            Some(other) => {
                return Err(invalid(
                    Self::LOG_FORMAT,
                    format!("{other:?} is not \"json\" or \"text\""),
                ));
            }
        };

        let log_level = match value(Self::LOG_LEVEL) {
            Some(text) => text
                .trim()
                .parse()
                .map_err(|_| invalid(Self::LOG_LEVEL, format!("{text:?} is not a log level")))?,
            None => tracing::Level::INFO,
        };

        Ok(Self {
            database_url,
            listen_address,
            server_name,
            trusted_proxies,
            log_format,
            log_level,
            limits: Limits::default(),
        })
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("database_url", &"(redacted)")
            .field("listen_address", &self.listen_address)
            .field("server_name", &self.server_name)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("log_format", &self.log_format)
            .field("log_level", &self.log_level)
            .field("limits", &self.limits)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&'static str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn database_url_is_required() {
        let error = Config::from_lookup(lookup(&[])).unwrap_err();
        assert!(matches!(error, ConfigError::Missing("JUNTO_DATABASE_URL")));
    }

    #[test]
    fn defaults_apply_when_only_the_database_is_given() {
        let config = Config::from_lookup(lookup(&[(
            "JUNTO_DATABASE_URL",
            "postgres://db.example.test/junto",
        )]))
        .unwrap();
        assert_eq!(
            config.listen_address,
            SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        assert_eq!(config.server_name, "Junto");
        assert!(config.trusted_proxies.is_empty());
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.log_level, tracing::Level::INFO);
    }

    #[test]
    fn every_setting_is_read() {
        let config = Config::from_lookup(lookup(&[
            ("JUNTO_DATABASE_URL", "postgres://db.example.test/junto"),
            ("JUNTO_LISTEN_ADDRESS", "0.0.0.0:9000"),
            ("JUNTO_SERVER_NAME", " Acme "),
            ("JUNTO_TRUSTED_PROXIES", "192.0.2.1, 2001:db8::1"),
            ("JUNTO_LOG_FORMAT", "text"),
            ("JUNTO_LOG_LEVEL", "debug"),
        ]))
        .unwrap();
        assert_eq!(
            config.listen_address,
            SocketAddr::from(([0, 0, 0, 0], 9000))
        );
        assert_eq!(config.server_name, "Acme");
        assert_eq!(
            config.trusted_proxies,
            [
                "192.0.2.1".parse::<IpAddr>().unwrap(),
                "2001:db8::1".parse().unwrap()
            ]
        );
        assert_eq!(config.log_format, LogFormat::Text);
        assert_eq!(config.log_level, tracing::Level::DEBUG);
    }

    #[test]
    fn invalid_values_are_refused() {
        for (name, value) in [
            ("JUNTO_LISTEN_ADDRESS", "not-an-address"),
            ("JUNTO_TRUSTED_PROXIES", "192.0.2.1,proxy"),
            ("JUNTO_LOG_FORMAT", "xml"),
            ("JUNTO_LOG_LEVEL", "loud"),
        ] {
            let result = Config::from_lookup(move |key| {
                if key == "JUNTO_DATABASE_URL" {
                    Some("postgres://db.example.test/junto".to_owned())
                } else if key == name {
                    Some(value.to_owned())
                } else {
                    None
                }
            });
            assert!(matches!(result, Err(ConfigError::Invalid { .. })), "{name}");
        }
    }

    #[test]
    fn debug_output_does_not_contain_the_database_url() {
        let config = Config::from_lookup(lookup(&[(
            "JUNTO_DATABASE_URL",
            "postgres://junto:database-password@db.example.test/junto",
        )]))
        .unwrap();
        assert!(!format!("{config:?}").contains("database-password"));
    }
}
