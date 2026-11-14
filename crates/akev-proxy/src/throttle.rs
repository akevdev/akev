//! Token-bucket bandwidth limiting for the `throttle` rule action.

use std::time::{Duration, Instant};

/// A token bucket refilled at `rate` bytes/second with a small burst allowance.
#[derive(Debug)]
pub(crate) struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    /// `None` when `bytes_per_sec` is 0 (unlimited).
    pub(crate) fn new(bytes_per_sec: u64) -> Option<Self> {
        if bytes_per_sec == 0 {
            return None;
        }
        let rate = bytes_per_sec as f64;
        // ~50ms worth of burst, at least one MTU-ish packet.
        let burst = (rate / 20.0).max(1400.0);
        Some(TokenBucket {
            rate,
            burst,
            tokens: burst,
            last: Instant::now(),
        })
    }

    /// Largest piece a chunk should be split into so the stream stays smooth.
    pub(crate) fn slice_size(&self) -> usize {
        (self.burst as usize).max(1024)
    }

    /// Consume `n` bytes worth of tokens; returns how long to wait before sending them.
    pub(crate) fn take(&mut self, n: usize) -> Duration {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.tokens -= n as f64;
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.rate)
        }
    }

    /// Async helper: wait as required to send `n` bytes.
    pub(crate) async fn wait(&mut self, n: usize) {
        let d = self.take(n);
        if !d.is_zero() {
            tokio::time::sleep(d).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_is_none() {
        assert!(TokenBucket::new(0).is_none());
    }

    #[test]
    fn delays_proportionally_to_debt() {
        let mut b = TokenBucket::new(10_000).unwrap();
        // The burst (1400 bytes) is free.
        assert_eq!(b.take(1400), Duration::ZERO);
        // 10_000 more bytes at 10KB/s ≈ 1s.
        let d = b.take(10_000);
        assert!(
            d >= Duration::from_millis(950) && d <= Duration::from_millis(1050),
            "{d:?}"
        );
        assert!(b.slice_size() >= 1024);
    }

    #[tokio::test]
    async fn wait_sleeps() {
        let mut b = TokenBucket::new(100_000).unwrap();
        let start = Instant::now();
        b.wait(5_000 + 10_000).await; // burst 5000 → ~100ms
        assert!(start.elapsed() >= Duration::from_millis(80));
    }
}
