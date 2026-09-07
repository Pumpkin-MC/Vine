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

pub struct ConnectionRateLimiter {
    enabled: bool,
    rate_per_sec: f64,
    burst: f64,
    buckets: RwLock<HashMap<IpAddr, TokenBucket>>,
}

impl ConnectionRateLimiter {
    pub fn new(enabled: bool, rate_per_sec: u32, burst: u32) -> Self {
        Self {
            enabled,
            rate_per_sec: rate_per_sec as f64,
            burst: burst as f64,
            buckets: RwLock::new(HashMap::new()),
        }
    }

    /// Returns true if connection is allowed, false if rate limited
    pub fn check_connection(&self, ip: &IpAddr) -> bool {
        if !self.enabled {
            return true;
        }

        let mut buckets = match self.buckets.write() {
            Ok(b) => b,
            Err(_) => return true,
        };

        if buckets.len() > 10_000 {
            let now = Instant::now();
            buckets.retain(|_, b| now.duration_since(b.last_update) < Duration::from_secs(60));
        }

        let bucket = buckets
            .entry(*ip)
            .or_insert_with(|| TokenBucket::new(self.burst));

        bucket.try_consume(self.rate_per_sec, self.burst)
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
}
