use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;

use time::{Duration, OffsetDateTime};

use crate::config::RateLimit;

/// Counts attempts per key in windows that start at the first attempt of the key. The state
/// is in memory, belongs to one server process and holds at most `capacity` keys.
///
/// Every window has the same length, so the keys expire in the order in which their windows
/// started. Each attempt drops the expired keys from the front of that order and, when the
/// limiter is full, the key whose window started first.
pub struct RateLimiter<K = IpAddr> {
    limit: RateLimit,
    capacity: usize,
    state: Mutex<State<K>>,
}

struct State<K> {
    windows: HashMap<K, Window>,
    started: VecDeque<(OffsetDateTime, K)>,
}

struct Window {
    started_at: OffsetDateTime,
    attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exceeded {
    pub retry_after: Duration,
}

impl<K: Hash + Eq + Copy> RateLimiter<K> {
    pub fn with_capacity(limit: RateLimit, capacity: usize) -> Self {
        Self {
            limit,
            capacity: capacity.max(1),
            state: Mutex::new(State {
                windows: HashMap::new(),
                started: VecDeque::new(),
            }),
        }
    }

    pub fn len(&self) -> usize {
        self.state().windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State<K>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Counts one attempt of `key` and refuses it when the window of the key is full.
    pub fn attempt_of(&self, key: K, now: OffsetDateTime) -> Result<(), Exceeded> {
        let mut state = self.state();
        while let Some((started_at, _)) = state.started.front() {
            if now - *started_at < self.limit.window {
                break;
            }
            state.forget_oldest();
        }
        if let Some(window) = state.windows.get_mut(&key) {
            if window.attempts >= self.limit.attempts {
                return Err(Exceeded {
                    retry_after: self.limit.window - (now - window.started_at),
                });
            }
            window.attempts += 1;
            return Ok(());
        }
        if self.limit.attempts == 0 {
            return Err(Exceeded {
                retry_after: self.limit.window,
            });
        }
        if state.windows.len() >= self.capacity {
            state.forget_oldest();
        }
        state.windows.insert(
            key,
            Window {
                started_at: now,
                attempts: 1,
            },
        );
        state.started.push_back((now, key));
        Ok(())
    }
}

impl<K: Hash + Eq + Copy> State<K> {
    fn forget_oldest(&mut self) {
        if let Some((started_at, key)) = self.started.pop_front() {
            let same_window = self
                .windows
                .get(&key)
                .is_some_and(|window| window.started_at == started_at);
            if same_window {
                self.windows.remove(&key);
            }
        }
    }
}

impl RateLimiter<IpAddr> {
    /// Counts one attempt of the client at `address`.
    pub fn attempt(&self, address: IpAddr, now: OffsetDateTime) -> Result<(), Exceeded> {
        self.attempt_of(limiter_key(address), now)
    }
}

/// Counts the challenges that have been issued and have not expired, for the ceiling of the
/// whole server.
pub struct Outstanding {
    ceiling: usize,
    expiries: Mutex<VecDeque<OffsetDateTime>>,
}

impl Outstanding {
    pub fn new(ceiling: u32) -> Self {
        Self {
            ceiling: usize::try_from(ceiling).unwrap_or(usize::MAX),
            expiries: Mutex::new(VecDeque::new()),
        }
    }

    /// Reserves one place until `expires_at`, or states how long the caller has to wait.
    pub fn reserve(&self, now: OffsetDateTime, expires_at: OffsetDateTime) -> Result<(), Exceeded> {
        let mut expiries = self
            .expiries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while expiries.front().is_some_and(|first| *first <= now) {
            expiries.pop_front();
        }
        match expiries.front() {
            Some(first) if expiries.len() >= self.ceiling => Err(Exceeded {
                retry_after: *first - now,
            }),
            None if self.ceiling == 0 => Err(Exceeded {
                retry_after: expires_at - now,
            }),
            _ => {
                expiries.push_back(expires_at);
                Ok(())
            }
        }
    }

    /// Gives back a place that [`Outstanding::reserve`] gave for a challenge that was not
    /// stored.
    pub fn release(&self, expires_at: OffsetDateTime) {
        let mut expiries = self
            .expiries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = expiries.iter().rposition(|expiry| *expiry == expires_at) {
            expiries.remove(index);
        }
    }
}

/// An IPv6 client controls a whole /64, so the limit counts the /64.
fn limiter_key(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let segments = v6.segments();
                IpAddr::V6(Ipv6Addr::new(
                    segments[0],
                    segments[1],
                    segments[2],
                    segments[3],
                    0,
                    0,
                    0,
                    0,
                ))
            }
        },
    }
}

/// The address of the client of a request.
///
/// `forwarded_for` is used only when the connection comes from a trusted proxy. The header is
/// read from the right, and the first address that is not a trusted proxy is the client.
pub fn client_address(peer: IpAddr, forwarded_for: &[&str], trusted_proxies: &[IpAddr]) -> IpAddr {
    let trusted = |address: &IpAddr| {
        let address = canonical(*address);
        trusted_proxies
            .iter()
            .any(|proxy| canonical(*proxy) == address)
    };
    if !trusted(&peer) {
        return peer;
    }
    let hops = forwarded_for
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .rev();
    for hop in hops {
        match hop.parse::<IpAddr>() {
            Ok(address) if trusted(&address) => continue,
            Ok(address) => return address,
            Err(_) => return peer,
        }
    }
    peer
}

fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        IpAddr::V4(_) => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000 + seconds).unwrap()
    }

    fn limiter(attempts: u32, seconds: i64) -> RateLimiter {
        limiter_of(attempts, seconds, 1_000)
    }

    fn limiter_of(attempts: u32, seconds: i64, capacity: usize) -> RateLimiter {
        RateLimiter::with_capacity(
            RateLimit {
                attempts,
                window: Duration::seconds(seconds),
            },
            capacity,
        )
    }

    #[test]
    fn attempts_within_the_limit_pass_and_the_next_is_refused() {
        let limiter = limiter(3, 60);
        let client = ip("198.51.100.7");
        for second in 0..3 {
            assert_eq!(limiter.attempt(client, at(second)), Ok(()));
        }
        assert_eq!(
            limiter.attempt(client, at(10)),
            Err(Exceeded {
                retry_after: Duration::seconds(50)
            })
        );
    }

    #[test]
    fn a_new_window_starts_after_the_window_length() {
        let limiter = limiter(1, 60);
        let client = ip("198.51.100.7");
        assert!(limiter.attempt(client, at(0)).is_ok());
        assert!(limiter.attempt(client, at(59)).is_err());
        assert!(limiter.attempt(client, at(60)).is_ok());
        assert!(limiter.attempt(client, at(61)).is_err());
    }

    #[test]
    fn addresses_are_counted_separately() {
        let limiter = limiter(1, 60);
        assert!(limiter.attempt(ip("198.51.100.7"), at(0)).is_ok());
        assert!(limiter.attempt(ip("198.51.100.8"), at(0)).is_ok());
        assert!(limiter.attempt(ip("198.51.100.7"), at(1)).is_err());
    }

    #[test]
    fn ipv6_addresses_of_one_64_bit_prefix_share_a_count() {
        let limiter = limiter(1, 60);
        assert!(limiter.attempt(ip("2001:db8:1:2::1"), at(0)).is_ok());
        assert!(limiter.attempt(ip("2001:db8:1:2:ffff::9"), at(0)).is_err());
        assert!(limiter.attempt(ip("2001:db8:1:3::1"), at(0)).is_ok());
        assert!(limiter.attempt(ip("::ffff:198.51.100.7"), at(0)).is_ok());
        assert!(limiter.attempt(ip("198.51.100.7"), at(0)).is_err());
    }

    #[test]
    fn the_number_of_counted_addresses_never_exceeds_the_capacity() {
        let limiter = RateLimiter::with_capacity(
            RateLimit {
                attempts: 2,
                window: Duration::seconds(60),
            },
            100,
        );
        for prefix in 0..5_000u32 {
            let address = ip(&format!(
                "2001:db8:{:x}:{:x}::1",
                prefix >> 16,
                prefix & 0xffff
            ));
            assert!(limiter.attempt(address, at(0)).is_ok());
            assert!(limiter.len() <= 100);
        }
        assert_eq!(limiter.len(), 100);
    }

    #[test]
    fn the_newest_addresses_keep_their_counts_when_the_oldest_are_dropped() {
        let limiter = limiter_of(1, 60, 3);
        for (second, address) in ["198.51.100.1", "198.51.100.2", "198.51.100.3"]
            .into_iter()
            .enumerate()
        {
            assert!(limiter.attempt(ip(address), at(second as i64)).is_ok());
        }
        assert!(limiter.attempt(ip("198.51.100.4"), at(3)).is_ok());
        assert_eq!(limiter.len(), 3);
        assert!(limiter.attempt(ip("198.51.100.2"), at(4)).is_err());
        assert!(limiter.attempt(ip("198.51.100.3"), at(4)).is_err());
        assert!(limiter.attempt(ip("198.51.100.4"), at(4)).is_err());
        assert!(limiter.attempt(ip("198.51.100.1"), at(4)).is_ok());
    }

    #[test]
    fn expired_addresses_are_dropped_as_time_passes() {
        let limiter = limiter_of(1, 60, 1_000);
        for index in 0..10u8 {
            let address = ip(&format!("198.51.100.{index}"));
            assert!(limiter.attempt(address, at(i64::from(index))).is_ok());
        }
        assert_eq!(limiter.len(), 10);
        assert!(limiter.attempt(ip("203.0.113.1"), at(64)).is_ok());
        assert_eq!(limiter.len(), 6);
        assert!(limiter.attempt(ip("203.0.113.2"), at(200)).is_ok());
        assert_eq!(limiter.len(), 1);
        assert!(limiter.attempt(ip("198.51.100.9"), at(200)).is_ok());
    }

    #[test]
    fn a_limiter_counts_any_kind_of_key() {
        let limiter: RateLimiter<u64> = RateLimiter::with_capacity(
            RateLimit {
                attempts: 1,
                window: Duration::seconds(60),
            },
            10,
        );
        assert!(limiter.attempt_of(7, at(0)).is_ok());
        assert!(limiter.attempt_of(7, at(1)).is_err());
        assert!(limiter.attempt_of(8, at(1)).is_ok());
    }

    #[test]
    fn outstanding_places_are_bounded_and_return_at_their_expiry() {
        let outstanding = Outstanding::new(2);
        assert!(outstanding.reserve(at(0), at(60)).is_ok());
        assert!(outstanding.reserve(at(10), at(70)).is_ok());
        assert_eq!(
            outstanding.reserve(at(20), at(80)),
            Err(Exceeded {
                retry_after: Duration::seconds(40)
            })
        );
        assert!(outstanding.reserve(at(60), at(120)).is_ok());
        assert!(outstanding.reserve(at(61), at(121)).is_err());
        outstanding.release(at(120));
        assert!(outstanding.reserve(at(61), at(121)).is_ok());
        assert!(Outstanding::new(0).reserve(at(0), at(60)).is_err());
    }

    #[test]
    fn forwarded_address_is_ignored_without_a_trusted_proxy() {
        let peer = ip("203.0.113.5");
        assert_eq!(client_address(peer, &["198.51.100.7"], &[]), peer);
        assert_eq!(
            client_address(peer, &["198.51.100.7"], &[ip("192.0.2.1")]),
            peer
        );
    }

    #[test]
    fn forwarded_address_is_used_when_the_peer_is_a_trusted_proxy() {
        let proxy = ip("192.0.2.1");
        let trusted = [proxy];
        assert_eq!(
            client_address(proxy, &["198.51.100.7"], &trusted),
            ip("198.51.100.7")
        );
        assert_eq!(
            client_address(ip("::ffff:192.0.2.1"), &["198.51.100.7"], &trusted),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn the_rightmost_address_that_is_not_a_proxy_is_the_client() {
        let trusted = [ip("192.0.2.1"), ip("192.0.2.2")];
        assert_eq!(
            client_address(
                ip("192.0.2.1"),
                &["203.0.113.99, 198.51.100.7, 192.0.2.2"],
                &trusted
            ),
            ip("198.51.100.7")
        );
        assert_eq!(
            client_address(ip("192.0.2.1"), &["203.0.113.99", "198.51.100.7"], &trusted),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn the_peer_is_used_when_the_header_is_absent_or_unreadable() {
        let proxy = ip("192.0.2.1");
        let trusted = [proxy];
        assert_eq!(client_address(proxy, &[], &trusted), proxy);
        assert_eq!(client_address(proxy, &["unknown"], &trusted), proxy);
        assert_eq!(
            client_address(proxy, &["198.51.100.7, unknown"], &trusted),
            proxy
        );
        assert_eq!(client_address(proxy, &["192.0.2.1"], &trusted), proxy);
    }
}
