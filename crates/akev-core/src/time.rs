use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current wall-clock time in unix milliseconds.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Escape text for HTML (`&`, `<`, `>`, `"`, `'`).
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Wall-clock timestamp in microseconds since the Unix epoch.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct UnixMicros(pub i64);

impl UnixMicros {
    pub fn now() -> Self {
        let d = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        UnixMicros(d.as_micros() as i64)
    }

    pub fn as_millis_f64(self) -> f64 {
        self.0 as f64 / 1000.0
    }

    /// RFC3339 representation (used by HAR export).
    pub fn to_rfc3339(self) -> String {
        let nanos = (self.0 as i128) * 1000;
        match time::OffsetDateTime::from_unix_timestamp_nanos(nanos) {
            Ok(dt) => dt
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            Err(_) => String::new(),
        }
    }

    pub fn from_rfc3339(s: &str) -> Option<Self> {
        let dt =
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
        Some(UnixMicros((dt.unix_timestamp_nanos() / 1000) as i64))
    }
}

/// A monotonic clock reading paired with the wall clock at capture start, so we can compute
/// precise durations (monotonic) while still reporting absolute times (wall clock).
#[derive(Clone, Copy, Debug)]
pub struct Stopwatch {
    wall: UnixMicros,
    mono: std::time::Instant,
}

impl Stopwatch {
    pub fn start() -> Self {
        Stopwatch {
            wall: UnixMicros::now(),
            mono: std::time::Instant::now(),
        }
    }

    pub fn started_at(&self) -> UnixMicros {
        self.wall
    }

    /// Milliseconds elapsed since start (fractional, monotonic).
    pub fn elapsed_ms(&self) -> f64 {
        self.mono.elapsed().as_secs_f64() * 1000.0
    }

    /// Current wall-clock time derived from the monotonic clock (never goes backwards
    /// relative to `started_at`).
    pub fn now(&self) -> UnixMicros {
        UnixMicros(self.wall.0 + self.mono.elapsed().as_micros() as i64)
    }
}
