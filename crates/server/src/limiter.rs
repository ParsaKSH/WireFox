//! Throttling of failed logins (API and SOCKS).

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Upper bound on tracked keys, so a flood of source addresses can't grow the
/// table without limit.
const MAX_TRACKED: usize = 65_536;
const PRUNE_INTERVAL: Duration = Duration::from_secs(1);

/// Counts failures per key and blocks a key after `max_failures` within `window`.
pub struct FailureLimiter<K> {
    max_failures: u32,
    window: Duration,
    inner: Mutex<Inner<K>>,
}

struct Inner<K> {
    entries: HashMap<K, Entry>,
    last_prune: Instant,
}

struct Entry {
    failures: u32,
    since: Instant,
}

impl<K: Hash + Eq + Clone> FailureLimiter<K> {
    pub fn new(max_failures: u32, window: Duration) -> Self {
        FailureLimiter {
            max_failures,
            window,
            inner: Mutex::new(Inner { entries: HashMap::new(), last_prune: Instant::now() }),
        }
    }

    pub fn is_blocked(&self, key: &K) -> bool {
        self.inner
            .lock()
            .entries
            .get(key)
            .is_some_and(|e| e.since.elapsed() < self.window && e.failures >= self.max_failures)
    }

    pub fn record_failure(&self, key: K) {
        let mut inner = self.inner.lock();
        if inner.entries.len() >= MAX_TRACKED && !inner.entries.contains_key(&key) {
            if inner.last_prune.elapsed() >= PRUNE_INTERVAL {
                inner.entries.retain(|_, e| e.since.elapsed() < self.window);
                inner.last_prune = Instant::now();
            }
            if inner.entries.len() >= MAX_TRACKED {
                // Still full of live entries: make room rather than stop tracking.
                if let Some(victim) = inner.entries.keys().next().cloned() {
                    inner.entries.remove(&victim);
                }
            }
        }
        let entry = inner.entries.entry(key).or_insert(Entry { failures: 0, since: Instant::now() });
        if entry.since.elapsed() >= self.window {
            *entry = Entry { failures: 0, since: Instant::now() };
        }
        entry.failures += 1;
    }

    pub fn record_success(&self, key: &K) {
        self.inner.lock().entries.remove(key);
    }
}

/// SOCKS login throttling. Failures are counted per client and username, so one
/// misconfigured app doesn't lock out everyone behind the same NAT, plus a much
/// higher per-client cap against spraying many usernames.
pub struct SocksAuthLimiter {
    per_user: FailureLimiter<(IpAddr, String)>,
    per_client: FailureLimiter<IpAddr>,
}

impl SocksAuthLimiter {
    pub fn new(window: Duration) -> Self {
        SocksAuthLimiter { per_user: FailureLimiter::new(10, window), per_client: FailureLimiter::new(100, window) }
    }

    pub fn is_blocked(&self, ip: IpAddr, username: &str) -> bool {
        let client = client_key(ip);
        self.per_client.is_blocked(&client) || self.per_user.is_blocked(&(client, username.to_string()))
    }

    pub fn record_failure(&self, ip: IpAddr, username: &str) {
        let client = client_key(ip);
        self.per_client.record_failure(client);
        self.per_user.record_failure((client, username.to_string()));
    }

    pub fn record_success(&self, ip: IpAddr, username: &str) {
        self.per_user.record_success(&(client_key(ip), username.to_string()));
    }
}

/// The address failures are counted under. IPv6 clients usually own a whole
/// /64, so they are grouped by it.
pub fn client_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_after_max_failures() {
        let limiter = FailureLimiter::new(3, Duration::from_secs(60));
        for _ in 0..3 {
            assert!(!limiter.is_blocked(&1));
            limiter.record_failure(1);
        }
        assert!(limiter.is_blocked(&1));
        assert!(!limiter.is_blocked(&2));
        limiter.record_success(&1);
        assert!(!limiter.is_blocked(&1));
    }

    #[test]
    fn window_expires() {
        let limiter = FailureLimiter::new(1, Duration::from_millis(10));
        limiter.record_failure(1);
        assert!(limiter.is_blocked(&1));
        std::thread::sleep(Duration::from_millis(20));
        assert!(!limiter.is_blocked(&1));
    }

    #[test]
    fn client_keys() {
        assert_eq!(client_key(ip("2001:db8:1:2::1")), client_key(ip("2001:db8:1:2::ffff")));
        assert_ne!(client_key(ip("2001:db8:1:2::1")), client_key(ip("2001:db8:1:3::1")));
        assert_eq!(client_key(ip("::ffff:192.0.2.9")), ip("192.0.2.9"));
    }

    #[test]
    fn socks_failures_are_per_user() {
        let limiter = SocksAuthLimiter::new(Duration::from_secs(60));
        let client = ip("192.0.2.1");
        for _ in 0..10 {
            limiter.record_failure(client, "old-phone");
        }
        assert!(limiter.is_blocked(client, "old-phone"));
        assert!(!limiter.is_blocked(client, "laptop"), "other users on the same IP keep working");
        for i in 0..100 {
            limiter.record_failure(client, &format!("guess-{i}"));
        }
        assert!(limiter.is_blocked(client, "laptop"), "spraying usernames blocks the whole client");
    }
}
