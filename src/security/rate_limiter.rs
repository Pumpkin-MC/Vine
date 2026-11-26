use std::{
    collections::HashMap,
    net::IpAddr,
    sync::RwLock,
    time::{Duration, Instant},
};

struct TokenBucket {
    tokens: f64,
    last_update: Instant,
}

impl TokenBucket {
    fn new(burst: f64) -> Self {
        Self {
            tokens: burst,
            last_update: Instant::now(),
        }
    }

    fn try_consume(&mut self, rate_per_sec: f64, burst: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update).as_secs_f64();
        self.last_update = now;

        self.tokens = (self.tokens + elapsed * rate_per_sec).min(burst);

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Number of tracked IPs above which stale buckets are pruned.
const PRUNE_THRESHOLD: usize = 10_000;
/// Minimum time between two pruning scans. Without this, a flood from many distinct IPs (all
/// buckets fresh, so nothing is removed) would trigger a full scan of the map on every connection.
const PRUNE_INTERVAL: Duration = Duration::from_secs(1);
/// A bucket idle for longer than this is dropped when pruning.
const IDLE_BUCKET_TTL: Duration = Duration::from_secs(60);
/// Hard cap on tracked IPs. At the cap, connections from IPs that are not tracked yet are
/// refused instead of letting the map (and memory) grow without bound under a flood.
const MAX_TRACKED_IPS: usize = 262_144;

struct RateLimiterSettings {
    enabled: bool,
    rate_per_sec: f64,
    burst: f64,
}

struct Buckets {
    map: HashMap<IpAddr, TokenBucket>,
    last_prune: Instant,
}

pub struct ConnectionRateLimiter {
    settings: RwLock<RateLimiterSettings>,
    buckets: RwLock<Buckets>,
    prune_threshold: usize,
    prune_interval: Duration,
    max_tracked: usize,
}

impl ConnectionRateLimiter {
    pub fn new(enabled: bool, rate_per_sec: u32, burst: u32) -> Self {
        Self::with_limits(
            enabled,
            rate_per_sec,
            burst,
            PRUNE_THRESHOLD,
            PRUNE_INTERVAL,
            MAX_TRACKED_IPS,
        )
    }

    fn with_limits(
        enabled: bool,
        rate_per_sec: u32,
        burst: u32,
        prune_threshold: usize,
        prune_interval: Duration,
        max_tracked: usize,
    ) -> Self {
        Self {
            settings: RwLock::new(RateLimiterSettings {
                enabled,
                rate_per_sec: rate_per_sec as f64,
                burst: burst as f64,
            }),
            buckets: RwLock::new(Buckets {
                map: HashMap::new(),
                last_prune: Instant::now(),
            }),
            prune_threshold,
            prune_interval,
            max_tracked,
        }
    }

    /// Updates connection rate limiter settings at runtime (used by `/vine reload`).
    pub fn reload(&self, enabled: bool, rate_per_sec: u32, burst: u32) {
        if let Ok(mut guard) = self.settings.write() {
            guard.enabled = enabled;
            guard.rate_per_sec = rate_per_sec as f64;
            guard.burst = burst as f64;
        }
    }

    /// Returns true if connection is allowed, false if rate limited
    pub fn check_connection(&self, ip: &IpAddr) -> bool {
        let (enabled, rate_per_sec, burst) = match self.settings.read() {
            Ok(s) => (s.enabled, s.rate_per_sec, s.burst),
            Err(_) => return true,
        };
        if !enabled {
            return true;
        }

        let mut buckets = match self.buckets.write() {
            Ok(b) => b,
            Err(_) => return true,
        };

        let now = Instant::now();
        if buckets.map.len() > self.prune_threshold
            && now.duration_since(buckets.last_prune) >= self.prune_interval
        {
            buckets.last_prune = now;
            buckets
                .map
                .retain(|_, b| now.duration_since(b.last_update) < IDLE_BUCKET_TTL);
        }

        if buckets.map.len() >= self.max_tracked && !buckets.map.contains_key(ip) {
            return false;
        }

        let bucket = buckets
            .map
            .entry(*ip)
            .or_insert_with(|| TokenBucket::new(burst));

        bucket.try_consume(rate_per_sec, burst)
    }
}

/// Per-connection packet rate limiter to detect and prevent packet flood attacks
pub struct PacketRateLimiter {
    max_packets_per_sec: u32,
    packet_count: u32,
    window_start: Instant,
}

impl PacketRateLimiter {
    pub fn new(max_packets_per_sec: u32) -> Self {
        Self {
            max_packets_per_sec,
            packet_count: 0,
            window_start: Instant::now(),
        }
    }

    /// Record a packet and return false if limit exceeded (flood detected)
    pub fn record_packet(&mut self) -> bool {
        if self.max_packets_per_sec == 0 {
            return true;
        }

        let now = Instant::now();
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.packet_count = 1;
            return true;
        }

        self.packet_count += 1;
        self.packet_count <= self.max_packets_per_sec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_rate_limiter() {
        let mut limiter = PacketRateLimiter::new(3);
        assert!(limiter.record_packet());
        assert!(limiter.record_packet());
        assert!(limiter.record_packet());
        assert!(!limiter.record_packet());
    }

    #[test]
    fn test_connection_rate_limiter() {
        let limiter = ConnectionRateLimiter::new(true, 2, 2);
        let ip: IpAddr = "192.168.1.100".parse().unwrap();

        assert!(limiter.check_connection(&ip));
        assert!(limiter.check_connection(&ip));
        assert!(!limiter.check_connection(&ip));
    }

    #[test]
    fn test_connection_limiter_hard_cap_refuses_new_ips() {
        let limiter = ConnectionRateLimiter::with_limits(
            true,
            1000,
            1000,
            PRUNE_THRESHOLD,
            PRUNE_INTERVAL,
            2,
        );
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let c: IpAddr = "10.0.0.3".parse().unwrap();

        assert!(limiter.check_connection(&a));
        assert!(limiter.check_connection(&b));
        assert!(!limiter.check_connection(&c));
        // Already tracked IPs keep working.
        assert!(limiter.check_connection(&a));
    }

    fn age_all_buckets(limiter: &ConnectionRateLimiter) {
        let mut buckets = limiter.buckets.write().unwrap();
        for bucket in buckets.map.values_mut() {
            bucket.last_update = Instant::now() - Duration::from_secs(120);
        }
    }

    fn tracked(limiter: &ConnectionRateLimiter) -> usize {
        limiter.buckets.read().unwrap().map.len()
    }

    #[test]
    fn test_connection_limiter_prune_is_throttled() {
        let ips: Vec<IpAddr> = (1..=3)
            .map(|i| format!("10.0.1.{i}").parse().unwrap())
            .collect();
        let fresh: IpAddr = "10.0.2.1".parse().unwrap();

        // Long interval: stale buckets are not scanned again on every connection.
        let throttled = ConnectionRateLimiter::with_limits(
            true,
            1000,
            1000,
            1,
            Duration::from_secs(3600),
            MAX_TRACKED_IPS,
        );
        for ip in &ips {
            assert!(throttled.check_connection(ip));
        }
        age_all_buckets(&throttled);
        assert!(throttled.check_connection(&fresh));
        assert_eq!(tracked(&throttled), 4);

        // Zero interval: the stale buckets are pruned on the next connection.
        let pruning = ConnectionRateLimiter::with_limits(
            true,
            1000,
            1000,
            1,
            Duration::ZERO,
            MAX_TRACKED_IPS,
        );
        for ip in &ips {
            assert!(pruning.check_connection(ip));
        }
        age_all_buckets(&pruning);
        assert!(pruning.check_connection(&fresh));
        assert_eq!(tracked(&pruning), 1);
    }
}
