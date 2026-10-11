use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use junto_protocol::origin::{Origin, Scheme};
use time::Duration;

use crate::database::{StreamPool, Timeouts};

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

/// The origins at which clients reach this server. A signed challenge is accepted only for one
/// of them (ADR 0108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigins {
    origins: Vec<Origin>,
    local_only: bool,
}

impl PublicOrigins {
    /// The origins that an operator states, separated by commas. Each is an `https` origin;
    /// an `http` origin is accepted only for a loopback host.
    pub fn stated(text: &str) -> Result<Self, String> {
        let mut origins = Vec::new();
        for part in text.split(',') {
            let part = part.trim();
            let origin = Origin::parse(part).map_err(|error| format!("{part:?}: {error}"))?;
            if origin.scheme() == Scheme::Http && !origin.is_loopback() {
                return Err(format!(
                    "{part:?}: an http origin is accepted only for a loopback host"
                ));
            }
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        Ok(Self {
            origins,
            local_only: false,
        })
    }

    /// The origins of a server without stated origins: the loopback origins of its own port.
    /// `None` when the server listens on another address, where clients of other machines
    /// reach it.
    pub fn local(listen_address: SocketAddr) -> Option<Self> {
        if !listen_address.ip().is_loopback() {
            return None;
        }
        let port = listen_address.port();
        let origins = ["localhost", "127.0.0.1", "[::1]"]
            .into_iter()
            .filter_map(|host| Origin::parse(&format!("http://{host}:{port}")).ok())
            .collect();
        Some(Self {
            origins,
            local_only: true,
        })
    }

    pub fn of(origins: Vec<Origin>) -> Self {
        Self {
            origins,
            local_only: false,
        }
    }

    pub fn contains(&self, origin: &Origin) -> bool {
        self.origins.contains(origin)
    }

    pub fn list(&self) -> &[Origin] {
        &self.origins
    }

    /// True when no origin was stated and the server accepts loopback origins only.
    pub const fn is_local_only(&self) -> bool {
        self.local_only
    }
}

/// The secret of the API key of the media server. It is never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaSecret(String);

impl MediaSecret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for MediaSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MediaSecret(redacted)")
    }
}

/// How the server reaches its media server, and the address that it gives to clients
/// (ADR 0508).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSettings {
    /// The address of the API of the media server on the private network.
    pub api_url: String,
    /// The address that clients connect to.
    pub client_url: String,
    pub api_key: String,
    pub api_secret: MediaSecret,
    pub request_timeout: Duration,
}

/// Where the recorder writes and which layout page it loads (ADR 0604).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderSettings {
    /// The directory of the recordings as the server sees it. The storage guard reads its
    /// free space.
    pub directory: PathBuf,
    /// The same directory as the recorder sees it.
    pub recorder_directory: String,
    /// The address at which the recorder loads the layout page (ADR 0602).
    pub layout_url: String,
}

/// The numbers of recordings and of the storage guard (ADR 0604, ADR 0605).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingLimits {
    /// The interval at which the controller compares each recording with its room.
    pub check_interval: Duration,
    /// The time limit of a request that starts a job. Without a recorder the media server
    /// answers after 22 seconds (ADR 0601).
    pub start_timeout: Duration,
    /// The time limit of every other request to the recorder.
    pub request_timeout: Duration,
    /// The time in which one pass of the controller collects the reports of jobs. A report
    /// that is not there by then counts as not given.
    pub report_time_limit: Duration,
    /// A job that is older than this and has no participant in the room is lost.
    pub lost_after: Duration,
    /// The time that a job has to report its end after its stop was delivered.
    pub ending_grace: Duration,
    /// The requests to start the job of one track, or the composite, that fail in a row
    /// before the server stops asking.
    pub start_attempts: u32,
    pub most_segments: u32,
    /// The recordings that are starting or run at one time.
    pub most_concurrent: u32,
    /// The free disk space that the server keeps.
    pub reserve_bytes: u64,
    /// The disk space that one recording reserves when it starts.
    pub reservation_bytes: u64,
}

impl Default for RecordingLimits {
    fn default() -> Self {
        const GIB: u64 = 1024 * 1024 * 1024;
        Self {
            check_interval: Duration::seconds(1),
            start_timeout: Duration::seconds(25),
            request_timeout: Duration::seconds(5),
            report_time_limit: Duration::seconds(5),
            lost_after: Duration::seconds(10),
            ending_grace: Duration::seconds(20),
            start_attempts: 3,
            most_segments: 2000,
            most_concurrent: 2,
            reserve_bytes: 5 * GIB,
            reservation_bytes: 8 * GIB,
        }
    }
}

/// The settings that an operator gives through environment variables.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub listen_address: SocketAddr,
    pub public_origins: PublicOrigins,
    pub server_name: String,
    pub trusted_proxies: Vec<IpAddr>,
    pub log_format: LogFormat,
    pub log_level: tracing::Level,
    /// Absent: the server has no media server and no call can be joined.
    pub media: Option<MediaSettings>,
    /// Absent: the server has no recorder and no recording starts.
    pub recorder: Option<RecorderSettings>,
    pub limits: Limits,
}

/// The numbers of calls (ADR 0509 to ADR 0514).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallLimits {
    /// The stated lifetime of a join token.
    pub token_lifetime: Duration,
    /// The time for which the media server accepts a token after its stated end.
    pub token_tolerance: Duration,
    /// The interval of the admission check.
    pub check_interval: Duration,
    /// The time limit of one request to the media server.
    pub request_timeout: Duration,
    /// A person whom the media server no longer lists has left after this time.
    pub absence_grace: Duration,
    pub entry_request_lifetime: Duration,
    /// The time in which an accepted person can enter.
    pub entry_admission: Duration,
    pub invitation_lifetime: Duration,
    pub move_lifetime: Duration,
    /// Meeting mode ends when its presenter is not in the call and sharing for this long.
    pub presenter_grace: Duration,
    /// A reported screen share ends when the media server lists no screen track for this
    /// long.
    pub share_grace: Duration,
    pub largest_user_limit: u32,
    pub most_waiting_per_call: u32,
    /// A room whose check failed this many times in a row is reported as failing.
    pub failing_after: u32,
    /// The rooms with a call that one part of the full check looks into.
    pub rooms_per_check: usize,
    /// The rooms that belong to no call and that one part of the full check looks into.
    pub most_ended_rooms_per_check: usize,
    /// The rooms that one check looks into at the same time.
    pub concurrent_room_checks: usize,
    /// The time limit of the check of one room.
    pub room_check_time_limit: Duration,
    /// The verified notifications of the media server that start a check.
    pub notifications: RateLimit,
    /// The processed notifications that the server remembers until their validity ends.
    pub most_remembered_notifications: usize,
    /// The rings of a direct-message call from one account to another.
    pub rings: RateLimit,
    /// After a declined ring, the caller cannot ring that account for this time.
    pub ring_pause_after_decline: Duration,
    /// Joins, locks, requests, invitations, moves, meetings and blocks of one session.
    pub actions: RateLimit,
    pub state_reports: RateLimit,
    pub speaking_reports: RateLimit,
    /// The shortest time between two speaking events of one call.
    pub speaking_interval: Duration,
    /// A speaking mark ends when its report is not renewed within this time.
    pub speaking_mark_lifetime: Duration,
    pub most_speaking_marks: usize,
}

impl Default for CallLimits {
    fn default() -> Self {
        Self {
            token_lifetime: Duration::seconds(60),
            token_tolerance: Duration::seconds(60),
            check_interval: Duration::seconds(2),
            request_timeout: Duration::seconds(2),
            absence_grace: Duration::seconds(20),
            entry_request_lifetime: Duration::seconds(60),
            entry_admission: Duration::seconds(60),
            invitation_lifetime: Duration::seconds(60),
            move_lifetime: Duration::seconds(30),
            presenter_grace: Duration::seconds(60),
            share_grace: Duration::seconds(10),
            largest_user_limit: 99,
            most_waiting_per_call: 50,
            failing_after: 3,
            rooms_per_check: 50,
            most_ended_rooms_per_check: 20,
            concurrent_room_checks: 10,
            room_check_time_limit: Duration::seconds(5),
            notifications: RateLimit {
                attempts: 300,
                window: Duration::seconds(10),
            },
            most_remembered_notifications: 8192,
            rings: RateLimit {
                attempts: 3,
                window: Duration::minutes(10),
            },
            ring_pause_after_decline: Duration::minutes(5),
            actions: RateLimit {
                attempts: 120,
                window: Duration::minutes(1),
            },
            state_reports: RateLimit {
                attempts: 30,
                window: Duration::seconds(10),
            },
            speaking_reports: RateLimit {
                attempts: 20,
                window: Duration::seconds(10),
            },
            speaking_interval: Duration::seconds(1),
            speaking_mark_lifetime: Duration::seconds(5),
            most_speaking_marks: 16,
        }
    }
}

/// The numbers that ADR 0108, 0109, 0111, 0112, 0114, 0116 and 0207 fix.
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
    pub invite_most_workspaces: usize,
    pub most_roles_per_workspace: u32,
    pub most_roles_per_member: u32,
    pub most_server_roles_per_account: u32,
    pub request_body_bytes: usize,
    pub request_body_drained_bytes: usize,
    /// The bytes of request bodies that the server holds at one time, for all requests.
    pub request_bodies_held_bytes: usize,
    /// The bytes of request bodies that the server holds at one time for one client address.
    pub request_bodies_held_per_address: usize,
    pub avatar_bytes: usize,
    pub avatar_longest_side: u32,
    pub calls: CallLimits,
    pub recordings: RecordingLimits,
    pub stream: StreamLimits,
}

/// The limits of the event stream (ADR 0116, ADR 0207).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamLimits {
    pub connections_per_device: usize,
    pub connections_per_account: usize,
    pub opens: RateLimit,
    pub heartbeat: Duration,
    pub idle_timeout: Duration,
    pub subscribe_timeout: Duration,
    pub send_timeout: Duration,
    pub read_deadline: Duration,
    pub retry_delay: Duration,
    pub most_failed_attempts: u32,
    pub close_timeout: Duration,
    pub stop_timeout: Duration,
    pub client_message_bytes: usize,
    pub client_frames: RateLimit,
    pub batch: u32,
    pub batch_bytes: u32,
    pub authorization_validity: Duration,
    pub replay_most_events: u32,
    pub events_kept: u32,
    pub poll_interval: Duration,
    pub concurrent_deliveries: usize,
    pub concurrent_session_checks: usize,
}

impl StreamLimits {
    /// The connections of the event stream: one for each read that can run at the same time,
    /// and one for the watcher.
    pub fn pool(&self) -> StreamPool {
        let readers = self.concurrent_deliveries.max(1) + self.concurrent_session_checks.max(1);
        StreamPool {
            connections: u32::try_from(readers + 1).unwrap_or(u32::MAX),
            time_limit: self.read_deadline,
        }
    }
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            connections_per_device: 2,
            connections_per_account: 10,
            opens: RateLimit {
                attempts: 30,
                window: Duration::minutes(1),
            },
            heartbeat: Duration::seconds(25),
            idle_timeout: Duration::seconds(60),
            subscribe_timeout: Duration::seconds(30),
            send_timeout: Duration::seconds(10),
            read_deadline: Duration::seconds(5),
            retry_delay: Duration::seconds(1),
            most_failed_attempts: 5,
            close_timeout: Duration::seconds(2),
            stop_timeout: Duration::seconds(5),
            client_message_bytes: 4096,
            client_frames: RateLimit {
                attempts: 30,
                window: Duration::minutes(1),
            },
            batch: 64,
            batch_bytes: 256 * 1024,
            authorization_validity: Duration::seconds(1),
            replay_most_events: 1_000,
            events_kept: 5_000,
            poll_interval: Duration::seconds(1),
            concurrent_deliveries: 4,
            concurrent_session_checks: 2,
        }
    }
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
            invite_most_workspaces: 100,
            most_roles_per_workspace: 250,
            most_roles_per_member: 64,
            most_server_roles_per_account: 64,
            request_body_bytes: 2 * 1024 * 1024,
            request_body_drained_bytes: 16 * 1024 * 1024,
            request_bodies_held_bytes: 64 * 1024 * 1024,
            request_bodies_held_per_address: 4 * 1024 * 1024,
            avatar_bytes: 256 * 1024,
            avatar_longest_side: 1024,
            calls: CallLimits::default(),
            recordings: RecordingLimits::default(),
            stream: StreamLimits::default(),
        }
    }
}

impl Config {
    pub const DATABASE_URL: &'static str = "JUNTO_DATABASE_URL";
    pub const LISTEN_ADDRESS: &'static str = "JUNTO_LISTEN_ADDRESS";
    pub const PUBLIC_ORIGINS: &'static str = "JUNTO_PUBLIC_ORIGINS";
    pub const SERVER_NAME: &'static str = "JUNTO_SERVER_NAME";
    pub const TRUSTED_PROXIES: &'static str = "JUNTO_TRUSTED_PROXIES";
    pub const LOG_FORMAT: &'static str = "JUNTO_LOG_FORMAT";
    pub const LOG_LEVEL: &'static str = "JUNTO_LOG_LEVEL";
    pub const LIVEKIT_API_URL: &'static str = "JUNTO_LIVEKIT_API_URL";
    pub const LIVEKIT_URL: &'static str = "JUNTO_LIVEKIT_URL";
    pub const LIVEKIT_API_KEY: &'static str = "JUNTO_LIVEKIT_API_KEY";
    pub const LIVEKIT_API_SECRET: &'static str = "JUNTO_LIVEKIT_API_SECRET";
    pub const RECORDINGS_DIR: &'static str = "JUNTO_RECORDINGS_DIR";
    pub const RECORDER_DIR: &'static str = "JUNTO_RECORDER_DIR";
    pub const RECORDER_LAYOUT_URL: &'static str = "JUNTO_RECORDER_LAYOUT_URL";

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

        let public_origins = match value(Self::PUBLIC_ORIGINS) {
            Some(text) => PublicOrigins::stated(&text)
                .map_err(|reason| invalid(Self::PUBLIC_ORIGINS, reason))?,
            None => PublicOrigins::local(listen_address).ok_or_else(|| {
                invalid(
                    Self::PUBLIC_ORIGINS,
                    "it is required when the server does not listen on a loopback address"
                        .to_owned(),
                )
            })?,
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

        let limits = Limits::default();
        let media_names = [
            Self::LIVEKIT_API_URL,
            Self::LIVEKIT_URL,
            Self::LIVEKIT_API_KEY,
            Self::LIVEKIT_API_SECRET,
        ];
        let media_values = media_names.map(|name| value(name).map(|text| text.trim().to_owned()));
        let media = match media_values {
            [None, None, None, None] => None,
            [
                Some(api_url),
                Some(client_url),
                Some(api_key),
                Some(api_secret),
            ] => {
                if !api_url.starts_with("http://") {
                    return Err(invalid(
                        Self::LIVEKIT_API_URL,
                        "the server reaches the API of the media server over the private \
                         network, with an http:// address"
                            .to_owned(),
                    ));
                }
                if !(client_url.starts_with("wss://") || client_url.starts_with("ws://")) {
                    return Err(invalid(
                        Self::LIVEKIT_URL,
                        "the address for clients starts with wss:// or ws://".to_owned(),
                    ));
                }
                Some(MediaSettings {
                    api_url,
                    client_url,
                    api_key,
                    api_secret: MediaSecret::new(api_secret),
                    request_timeout: limits.calls.request_timeout,
                })
            }
            values => {
                let missing = media_names
                    .iter()
                    .zip(&values)
                    .find(|(_, value)| value.is_none())
                    .map_or(Self::LIVEKIT_API_URL, |(name, _)| *name);
                return Err(invalid(
                    missing,
                    "the four settings of the media server are given together".to_owned(),
                ));
            }
        };

        let trimmed = |name| value(name).map(|text| text.trim().to_owned());
        let recorder = match (
            trimmed(Self::RECORDINGS_DIR),
            trimmed(Self::RECORDER_LAYOUT_URL),
        ) {
            (None, None) => {
                if value(Self::RECORDER_DIR).is_some() {
                    return Err(invalid(
                        Self::RECORDER_DIR,
                        format!("it is given together with {}", Self::RECORDINGS_DIR),
                    ));
                }
                None
            }
            (Some(directory), Some(layout_url)) => {
                if media.is_none() {
                    return Err(invalid(
                        Self::RECORDINGS_DIR,
                        "the recorder is reached through the media server, whose four settings \
                         are missing"
                            .to_owned(),
                    ));
                }
                let recorder_directory =
                    trimmed(Self::RECORDER_DIR).unwrap_or_else(|| directory.clone());
                for (name, path) in [
                    (Self::RECORDINGS_DIR, &directory),
                    (Self::RECORDER_DIR, &recorder_directory),
                ] {
                    if !path.starts_with('/') {
                        return Err(invalid(name, "the path is absolute".to_owned()));
                    }
                }
                if !(layout_url.starts_with("http://") || layout_url.starts_with("https://")) {
                    return Err(invalid(
                        Self::RECORDER_LAYOUT_URL,
                        "the address starts with http:// or https://".to_owned(),
                    ));
                }
                Some(RecorderSettings {
                    directory: PathBuf::from(directory),
                    recorder_directory: recorder_directory.trim_end_matches('/').to_owned(),
                    layout_url,
                })
            }
            (directory, _) => {
                let missing = if directory.is_none() {
                    Self::RECORDINGS_DIR
                } else {
                    Self::RECORDER_LAYOUT_URL
                };
                return Err(invalid(
                    missing,
                    "the directory of the recordings and the address of the layout page are \
                     given together"
                        .to_owned(),
                ));
            }
        };

        Ok(Self {
            database_url,
            listen_address,
            public_origins,
            server_name,
            trusted_proxies,
            log_format,
            log_level,
            media,
            recorder,
            limits,
        })
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("database_url", &"(redacted)")
            .field("listen_address", &self.listen_address)
            .field("public_origins", &self.public_origins)
            .field("server_name", &self.server_name)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("log_format", &self.log_format)
            .field("log_level", &self.log_level)
            .field("media", &self.media)
            .field("recorder", &self.recorder)
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
        assert!(config.public_origins.is_local_only());
        let origins: Vec<&str> = config
            .public_origins
            .list()
            .iter()
            .map(Origin::as_str)
            .collect();
        assert_eq!(
            origins,
            [
                "http://localhost:8080",
                "http://127.0.0.1:8080",
                "http://[::1]:8080"
            ]
        );
        assert!(config.trusted_proxies.is_empty());
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.log_level, tracing::Level::INFO);
    }

    #[test]
    fn every_setting_is_read() {
        let config = Config::from_lookup(lookup(&[
            ("JUNTO_DATABASE_URL", "postgres://db.example.test/junto"),
            ("JUNTO_LISTEN_ADDRESS", "0.0.0.0:9000"),
            (
                "JUNTO_PUBLIC_ORIGINS",
                "https://Chat.example.test, https://chat.example.test:443,http://localhost:9000",
            ),
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
        assert!(!config.public_origins.is_local_only());
        assert_eq!(
            config.public_origins.list(),
            [
                Origin::parse("https://chat.example.test").unwrap(),
                Origin::parse("http://localhost:9000").unwrap()
            ]
        );
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
            ("JUNTO_LISTEN_ADDRESS", "0.0.0.0:8080"),
            ("JUNTO_LISTEN_ADDRESS", "192.0.2.7:8080"),
            ("JUNTO_PUBLIC_ORIGINS", "chat.example.test"),
            ("JUNTO_PUBLIC_ORIGINS", "http://chat.example.test"),
            ("JUNTO_PUBLIC_ORIGINS", "https://chat.example.test/api"),
            ("JUNTO_PUBLIC_ORIGINS", "https://chat.example.test,"),
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
    fn a_server_that_other_machines_reach_needs_stated_origins() {
        let stated = Config::from_lookup(lookup(&[
            ("JUNTO_DATABASE_URL", "postgres://db.example.test/junto"),
            ("JUNTO_LISTEN_ADDRESS", "0.0.0.0:8080"),
            ("JUNTO_PUBLIC_ORIGINS", "https://chat.example.test"),
        ]))
        .unwrap();
        let loopback = Origin::parse("http://localhost:8080").unwrap();
        assert!(!stated.public_origins.contains(&loopback));
        assert!(
            stated
                .public_origins
                .contains(&Origin::parse("https://chat.example.test").unwrap())
        );
        assert!(PublicOrigins::local("192.0.2.7:8080".parse().unwrap()).is_none());
        assert!(PublicOrigins::local("[::1]:8080".parse().unwrap()).is_some());
    }

    #[test]
    fn the_media_server_is_given_with_all_four_settings_or_none() {
        const DATABASE: (&str, &str) = ("JUNTO_DATABASE_URL", "postgres://db.example.test/junto");
        let config = Config::from_lookup(lookup(&[DATABASE])).unwrap();
        assert_eq!(config.media, None);

        let config = Config::from_lookup(lookup(&[
            DATABASE,
            ("JUNTO_LIVEKIT_API_URL", "http://livekit:7880"),
            ("JUNTO_LIVEKIT_URL", "wss://media.example.test"),
            ("JUNTO_LIVEKIT_API_KEY", "key"),
            ("JUNTO_LIVEKIT_API_SECRET", "media-server-secret"),
        ]))
        .unwrap();
        let media = config.media.clone().unwrap();
        assert_eq!(media.api_url, "http://livekit:7880");
        assert_eq!(media.client_url, "wss://media.example.test");
        assert_eq!(media.api_secret.expose(), "media-server-secret");
        assert!(!format!("{config:?}").contains("media-server-secret"));

        let refused: [&[(&str, &str)]; 3] = [
            &[DATABASE, ("JUNTO_LIVEKIT_API_URL", "http://livekit:7880")],
            &[
                DATABASE,
                ("JUNTO_LIVEKIT_API_URL", "https://media.example.test"),
                ("JUNTO_LIVEKIT_URL", "wss://media.example.test"),
                ("JUNTO_LIVEKIT_API_KEY", "key"),
                ("JUNTO_LIVEKIT_API_SECRET", "media-server-secret"),
            ],
            &[
                DATABASE,
                ("JUNTO_LIVEKIT_API_URL", "http://livekit:7880"),
                ("JUNTO_LIVEKIT_URL", "https://media.example.test"),
                ("JUNTO_LIVEKIT_API_KEY", "key"),
                ("JUNTO_LIVEKIT_API_SECRET", "media-server-secret"),
            ],
        ];
        for pairs in refused {
            let result = Config::from_lookup(|name| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            });
            assert!(
                matches!(result, Err(ConfigError::Invalid { .. })),
                "{pairs:?}"
            );
        }
    }

    #[test]
    fn the_recorder_needs_its_directory_its_layout_page_and_the_media_server() {
        const BASE: [(&str, &str); 5] = [
            ("JUNTO_DATABASE_URL", "postgres://db.example.test/junto"),
            ("JUNTO_LIVEKIT_API_URL", "http://livekit:7880"),
            ("JUNTO_LIVEKIT_URL", "wss://media.example.test"),
            ("JUNTO_LIVEKIT_API_KEY", "key"),
            ("JUNTO_LIVEKIT_API_SECRET", "media-server-secret"),
        ];
        let with = |more: &'static [(&'static str, &'static str)]| {
            Config::from_lookup(move |name| {
                BASE.iter()
                    .chain(more)
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            })
        };
        assert_eq!(with(&[]).unwrap().recorder, None);

        let config = with(&[
            ("JUNTO_RECORDINGS_DIR", "/srv/junto/recordings"),
            ("JUNTO_RECORDER_LAYOUT_URL", "http://server:8080/layout/"),
        ])
        .unwrap();
        let recorder = config.recorder.unwrap();
        assert_eq!(recorder.directory, PathBuf::from("/srv/junto/recordings"));
        assert_eq!(recorder.recorder_directory, "/srv/junto/recordings");
        assert_eq!(recorder.layout_url, "http://server:8080/layout/");

        let config = with(&[
            ("JUNTO_RECORDINGS_DIR", "/srv/junto/recordings"),
            ("JUNTO_RECORDER_DIR", "/out/"),
            ("JUNTO_RECORDER_LAYOUT_URL", "http://server:8080/layout/"),
        ])
        .unwrap();
        assert_eq!(config.recorder.unwrap().recorder_directory, "/out");

        let refused: [&'static [(&str, &str)]; 5] = [
            &[("JUNTO_RECORDINGS_DIR", "/srv/junto/recordings")],
            &[("JUNTO_RECORDER_LAYOUT_URL", "http://server:8080/layout/")],
            &[("JUNTO_RECORDER_DIR", "/out")],
            &[
                ("JUNTO_RECORDINGS_DIR", "recordings"),
                ("JUNTO_RECORDER_LAYOUT_URL", "http://server:8080/layout/"),
            ],
            &[
                ("JUNTO_RECORDINGS_DIR", "/srv/junto/recordings"),
                ("JUNTO_RECORDER_LAYOUT_URL", "server:8080/layout/"),
            ],
        ];
        for more in refused {
            assert!(
                matches!(with(more), Err(ConfigError::Invalid { .. })),
                "{more:?}"
            );
        }
        let without_media = Config::from_lookup(lookup(&[
            ("JUNTO_DATABASE_URL", "postgres://db.example.test/junto"),
            ("JUNTO_RECORDINGS_DIR", "/srv/junto/recordings"),
            ("JUNTO_RECORDER_LAYOUT_URL", "http://server:8080/layout/"),
        ]));
        assert!(matches!(without_media, Err(ConfigError::Invalid { .. })));
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
