// Adapted from NexuSSH, Copyright (c) 2026 hipocrisle, MIT; see ../LICENSE.
//! Tiny in-memory token-bucket rate limiter to slow brute-force on the
//! login / prelogin endpoints. Keyed by an arbitrary string (we use
//! "username|client-ip"). Not distributed — fine for a single-binary Phase 0.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// `capacity` = burst size, `refill_per_sec` = sustained rate.
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        RateLimiter {
            capacity,
            refill_per_sec,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if the action is allowed (and consumes a token).
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap();

        // Opportunistic cleanup so the map can't grow unbounded.
        if map.len() > 10_000 {
            map.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(3600));
        }

        if map.len() >= 20000 && !map.contains_key(key) {
            return false;
        }
        let b = map.entry(key.to_string()).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
