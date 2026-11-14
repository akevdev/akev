//! Minimal terminal styling (ANSI) honouring `NO_COLOR`, `TERM=dumb` and non-TTY outputs,
//! plus human-friendly number formatting.

use std::io::IsTerminal;

/// Whether colours are allowed at all (environment).
fn color_allowed() -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    !matches!(std::env::var("TERM").as_deref(), Ok("dumb"))
}

/// Styler for one output stream.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    on: bool,
}

impl Paint {
    pub fn stdout() -> Self {
        Paint {
            on: color_allowed() && std::io::stdout().is_terminal(),
        }
    }

    pub fn stderr() -> Self {
        Paint {
            on: color_allowed() && std::io::stderr().is_terminal(),
        }
    }

    #[cfg(test)]
    pub fn plain() -> Self {
        Paint { on: false }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn magenta(&self, s: &str) -> String {
        self.wrap("35", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn accent(&self, s: &str) -> String {
        self.wrap("1;36", s)
    }

    /// Colour an HTTP status code by class.
    pub fn status(&self, status: Option<u16>, text: &str) -> String {
        match status {
            Some(200..=299) => self.green(text),
            Some(300..=399) => self.cyan(text),
            Some(400..=499) => self.yellow(text),
            Some(_) => self.red(text),
            None => self.red(text),
        }
    }
}

/// `1.2 KB`, `3.4 MB`...
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `85ms`, `1.24s`, `2m05s`.
pub fn duration_ms(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{}ms", ms.round() as u64)
    } else if ms < 60_000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else {
        let secs = (ms / 1000.0).round() as u64;
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(duration_ms(12.4), "12ms");
        assert_eq!(duration_ms(1234.0), "1.23s");
        assert_eq!(duration_ms(125_000.0), "2m05s");
    }

    #[test]
    fn plain_paint_has_no_escapes() {
        let p = Paint::plain();
        assert_eq!(p.bold("x"), "x");
        assert_eq!(p.status(Some(500), "500"), "500");
    }
}
