//! Observability without telemetry: everything stays on this computer.
//!
//! * A rotating log file, `<data dir>/logs/akev.log` ([`LOG_MAX_BYTES`] per file, the
//!   current one plus `akev.log.1` and `akev.log.2`), written by the CLI daemon and the
//!   desktop app ([`open_log`] + [`file_layer`]).
//! * A tracing layer ([`RecentEvents`]) keeping the last log lines (for crash reports) and
//!   the last warnings and errors (for `GET /api/diagnostics`) in memory.
//! * A panic hook ([`install_panic_hook`]) writing a crash report to
//!   `<data dir>/crashes/crash-<UTC time>.txt`: version, OS, thread, panic message and
//!   location, backtrace and the last log lines. Never captured traffic, never the API token:
//!   every line goes through [`scrub`].
//!
//! The binaries compose these with their own stderr output; the library never installs a
//! global subscriber or hook by itself.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// Log directory inside the data directory.
pub const LOG_DIR: &str = "logs";
/// Current log file name.
pub const LOG_FILE: &str = "akev.log";
/// Crash report directory inside the data directory.
pub const CRASH_DIR: &str = "crashes";
/// A log file is rotated once it would grow past this size.
pub const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Log files kept: `akev.log`, `akev.log.1`, `akev.log.2`.
pub const LOG_FILES_KEPT: usize = 3;
/// Crash reports kept (the oldest are deleted).
pub const MAX_CRASH_REPORTS: usize = 30;
/// Log lines remembered for crash reports.
const RECENT_LINES: usize = 200;
/// Log lines included in a crash report.
const CRASH_LOG_LINES: usize = 50;
/// Warnings and errors remembered for diagnostics.
const RECENT_WARNINGS: usize = 100;
/// Longest remembered line or message.
const MAX_LINE: usize = 2000;
/// Secrets remembered for scrubbing (one per started instance).
const MAX_SECRETS: usize = 16;

/// Default filter of the log file: Akev's own info messages, warnings from everything else.
pub const DEFAULT_FILE_FILTER: &str = "warn,akev=info,akev_api=info,akev_proxy=info,akev_store=info,akev_rules=info,akev_intercept=info,akev_wireguard=info,akev_ca=info,akev_content=info,akev_core=info,akev_filter=info,akev_codegen=info,akev_desktop=info";

// ---------------------------------------------------------------------------------------
// Rotating log file

struct Rotating {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    keep: usize,
}

impl Rotating {
    fn open_file(path: &Path) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Logs can name the hosts clients talk to: keep them private.
            options.mode(0o600);
        }
        options.open(path)
    }

    fn numbered(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// `akev.log.1` → `akev.log.2` ..., `akev.log` → `akev.log.1`, then a fresh `akev.log`.
    fn rotate(&mut self) {
        self.file = None;
        if self.keep > 1 {
            for n in (1..self.keep - 1).rev() {
                let from = self.numbered(n);
                if from.exists() {
                    let to = self.numbered(n + 1);
                    let _ = std::fs::remove_file(&to);
                    let _ = std::fs::rename(&from, &to);
                }
            }
            let first = self.numbered(1);
            let _ = std::fs::remove_file(&first);
            let _ = std::fs::rename(&self.path, &first);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
        self.size = 0;
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.rotate();
        }
        if self.file.is_none() {
            let file = Self::open_file(&self.path)?;
            // Another process may have rotated or appended meanwhile.
            self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
            self.file = Some(file);
        }
        let Some(file) = self.file.as_mut() else {
            return Err(io::Error::other("log file unavailable"));
        };
        match file.write_all(buf) {
            Ok(()) => {
                self.size += buf.len() as u64;
                Ok(())
            }
            Err(e) => {
                // Reopen on the next write (the file may have been deleted or moved).
                self.file = None;
                Err(e)
            }
        }
    }
}

/// A size-rotated log file, usable as a `tracing_subscriber` writer. Cheap to clone.
#[derive(Clone)]
pub struct LogWriter {
    inner: Arc<Mutex<Rotating>>,
}

impl LogWriter {
    /// Open (append to) `path`, rotating past `max_bytes` and keeping `keep` files in all.
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<LogWriter> {
        let file = Rotating::open_file(path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(LogWriter {
            inner: Arc::new(Mutex::new(Rotating {
                path: path.to_path_buf(),
                file: Some(file),
                size,
                max_bytes: max_bytes.max(1),
                keep: keep.max(1),
            })),
        })
    }

    /// Path of the current log file.
    pub fn path(&self) -> PathBuf {
        self.inner.lock().path.clone()
    }
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.inner.lock().file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Open `<data_dir>/logs/akev.log` (creating the directories) and remember it as this
/// process's log file (reported by `GET /api/diagnostics`).
pub fn open_log(data_dir: &Path) -> io::Result<LogWriter> {
    crate::persist::ensure_dir(data_dir)?;
    let dir = data_dir.join(LOG_DIR);
    crate::persist::ensure_dir(&dir)?;
    let writer = LogWriter::open(&dir.join(LOG_FILE), LOG_MAX_BYTES, LOG_FILES_KEPT)?;
    *LOG_PATH.lock() = Some(writer.path());
    Ok(writer)
}

/// The log file this process writes, if any.
pub fn log_file_path() -> Option<PathBuf> {
    LOG_PATH.lock().clone()
}

/// A plain-text (no colours) formatting layer writing to `writer`.
pub fn file_layer<S>(writer: LogWriter) -> impl Layer<S> + Send + Sync + 'static
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(writer)
        .log_internal_errors(false)
}

/// The log file's filter: `RUST_LOG` when set, else `verbose` directives (from `-v`), else
/// [`DEFAULT_FILE_FILTER`].
pub fn file_filter(verbose: Option<&str>) -> tracing_subscriber::EnvFilter {
    use tracing_subscriber::EnvFilter;
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(verbose.unwrap_or(DEFAULT_FILE_FILTER)))
}

// ---------------------------------------------------------------------------------------
// Recent events

/// A warning or error remembered for diagnostics.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWarning {
    /// When it was logged (RFC 3339, UTC).
    pub at: String,
    /// `WARN` or `ERROR`.
    pub level: String,
    /// The module that logged it.
    pub target: String,
    /// The message and its fields (scrubbed).
    pub message: String,
}

struct Recent {
    lines: VecDeque<String>,
    warnings: VecDeque<RecentWarning>,
}

static RECENT: Mutex<Recent> = Mutex::new(Recent {
    lines: VecDeque::new(),
    warnings: VecDeque::new(),
});

/// The last warnings and errors (oldest first).
pub fn recent_warnings() -> Vec<RecentWarning> {
    RECENT.lock().warnings.iter().cloned().collect()
}

/// The last `n` log lines (oldest first).
pub fn recent_lines(n: usize) -> Vec<String> {
    let recent = RECENT.lock();
    let skip = recent.lines.len().saturating_sub(n);
    recent.lines.iter().skip(skip).cloned().collect()
}

/// Remembers recent events in memory: every line it sees (for crash reports) and the
/// warnings and errors (for diagnostics). Give it the log file's filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecentEvents;

/// Collects an event's message and fields as `message k=v ...`.
struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.insert_str(0, value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let mut message = String::new();
            let _ = write!(message, "{value:?}");
            self.0.insert_str(0, &message);
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl<S: Subscriber> Layer<S> for RecentEvents {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let message = truncate(scrub(fields.0.trim_start()));
        let at = now_rfc3339();
        let level = meta.level().to_string();
        let line = format!("{at} {level:>5} {}: {message}", meta.target());
        // Never block (or deadlock, inside the panic hook) on the ring.
        let Some(mut recent) = RECENT.try_lock_for(std::time::Duration::from_millis(50)) else {
            return;
        };
        if recent.lines.len() >= RECENT_LINES {
            recent.lines.pop_front();
        }
        recent.lines.push_back(line);
        if *meta.level() <= Level::WARN {
            if recent.warnings.len() >= RECENT_WARNINGS {
                recent.warnings.pop_front();
            }
            recent.warnings.push_back(RecentWarning {
                at,
                level,
                target: meta.target().to_string(),
                message,
            });
        }
    }
}

// ---------------------------------------------------------------------------------------
// Scrubbing

static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Remember a secret (the API token) so it never appears in crash reports or diagnostics.
pub fn register_secret(secret: &str) {
    if secret.len() < 4 {
        return;
    }
    let mut secrets = SECRETS.lock();
    if secrets.iter().any(|s| s == secret) {
        return;
    }
    if secrets.len() >= MAX_SECRETS {
        secrets.remove(0);
    }
    secrets.push(secret.to_string());
}

const REDACTED: &str = "[redacted]";

/// Query parameters / `key=value` pairs whose values are secrets.
const SECRET_KEYS: &[&str] = &[
    "token=",
    "access_token=",
    "refresh_token=",
    "id_token=",
    "api_key=",
    "apikey=",
    "password=",
    "secret=",
];
/// Header names (`name: value`) whose values are secrets.
const SECRET_HEADERS: &[&str] = &[
    "proxy-authorization:",
    "authorization:",
    "set-cookie:",
    "cookie:",
    "x-api-key:",
];
/// Authentication schemes followed by a credential.
const SECRET_SCHEMES: &[&str] = &["bearer ", "basic "];

/// Replace the value starting at byte `start` (ending at a delimiter) with [`REDACTED`].
fn redact_values(text: &str, needles: &[&str], until_eol: bool) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while pos < text.len() {
        let next = needles
            .iter()
            .filter_map(|n| lower[pos..].find(n).map(|i| (pos + i, n.len())))
            .min_by_key(|(i, _)| *i);
        let Some((at, len)) = next else { break };
        let value_start = at + len;
        let rest = &text[value_start..];
        // Skip spaces between `name:` and its value.
        let skipped = rest.len() - rest.trim_start_matches(' ').len();
        let value = &rest[skipped..];
        if value.starts_with(REDACTED) {
            let done = value_start + skipped + REDACTED.len();
            out.push_str(&text[pos..done]);
            pos = done;
            continue;
        }
        let end = value
            .find(|c: char| {
                if until_eol {
                    matches!(c, '\n' | '\r' | '"' | '\'')
                } else {
                    matches!(
                        c,
                        '&' | ' ' | '\n' | '\r' | '"' | '\'' | ';' | ',' | ')' | ']' | '}' | '>'
                    )
                }
            })
            .unwrap_or(value.len());
        out.push_str(&text[pos..value_start + skipped]);
        if end > 0 {
            out.push_str(REDACTED);
        }
        pos = value_start + skipped + end;
    }
    out.push_str(&text[pos.min(text.len())..]);
    out
}

/// Remove secrets from a log line or panic message: registered secrets (the API token),
/// `token=`-style values, credentials after `Bearer` / `Basic`, and the values of
/// `Authorization`, `Cookie` and similar headers. The home directory becomes `~`.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    if let Some(secrets) = SECRETS.try_lock_for(std::time::Duration::from_millis(50)) {
        for secret in secrets.iter() {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
    }
    out = redact_values(&out, SECRET_KEYS, false);
    out = redact_values(&out, SECRET_SCHEMES, false);
    out = redact_values(&out, SECRET_HEADERS, true);
    tilde_home(&out)
}

/// Replace the user's home directory with `~` (paths in reports shouldn't name the user).
pub fn tilde_home(text: &str) -> String {
    match dirs::home_dir() {
        Some(home) => {
            let home = home.display().to_string();
            if home.len() > 1 && text.contains(&home) {
                text.replace(&home, "~")
            } else {
                text.to_string()
            }
        }
        None => text.to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// Crash reports

static CRASH_DIR_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static IN_HOOK: AtomicBool = AtomicBool::new(false);

/// Install a panic hook writing crash reports into `crash_dir` (created when needed), then
/// running the previous hook (which prints the panic). Installing again only changes the
/// directory.
pub fn install_panic_hook(crash_dir: PathBuf) {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    *CRASH_DIR_PATH.lock() = Some(crash_dir);
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic while writing a report must not recurse.
        if !IN_HOOK.swap(true, Ordering::AcqRel) {
            let dir = CRASH_DIR_PATH
                .try_lock_for(std::time::Duration::from_millis(100))
                .and_then(|d| d.clone());
            if let Some(dir) = dir {
                let report = crash_report(&PanicDetails::from_hook(info));
                match write_crash_report(&dir, &report) {
                    Ok(path) => {
                        let _ = writeln!(
                            io::stderr(),
                            "Akev crashed. A crash report was saved to {}",
                            path.display()
                        );
                    }
                    Err(e) => {
                        let _ = writeln!(io::stderr(), "Couldn't save a crash report: {e}");
                    }
                }
            }
            IN_HOOK.store(false, Ordering::Release);
        }
        previous(info);
    }));
}

/// What a crash report says about the panic.
#[derive(Clone, Debug)]
pub struct PanicDetails {
    pub thread: String,
    pub message: String,
    pub location: String,
    pub backtrace: String,
}

impl PanicDetails {
    fn from_hook(info: &std::panic::PanicHookInfo<'_>) -> Self {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        PanicDetails {
            thread,
            message,
            location,
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        }
    }
}

/// The text of a crash report (scrubbed).
pub fn crash_report(panic: &PanicDetails) -> String {
    let os = sysinfo::System::long_os_version().unwrap_or_else(|_| std::env::consts::OS.into());
    let program = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "akev".into());
    let mut out = String::new();
    let _ = writeln!(out, "Akev crash report");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Version: {} ({})",
        akev_core::VERSION,
        akev_core::RELEASE_DATE
    );
    let _ = writeln!(out, "OS: {os} ({})", std::env::consts::ARCH);
    let _ = writeln!(out, "Program: {program} (pid {})", std::process::id());
    let _ = writeln!(out, "Time: {}", now_rfc3339());
    let _ = writeln!(out, "Thread: {}", panic.thread);
    let _ = writeln!(out);
    let _ = writeln!(out, "Panic: {}", scrub(&panic.message));
    let _ = writeln!(out, "Location: {}", scrub(&panic.location));
    let _ = writeln!(out);
    let _ = writeln!(out, "Backtrace:");
    let _ = writeln!(out, "{}", scrub(&panic.backtrace));
    let _ = writeln!(out);
    let _ = writeln!(out, "Recent log lines:");
    for line in recent_lines_for_crash() {
        let _ = writeln!(out, "{line}");
    }
    out
}

/// Recent lines without blocking: the panic may have happened while the ring was locked.
fn recent_lines_for_crash() -> Vec<String> {
    let Some(recent) = RECENT.try_lock_for(std::time::Duration::from_millis(100)) else {
        return vec!["(log lines unavailable)".into()];
    };
    let skip = recent.lines.len().saturating_sub(CRASH_LOG_LINES);
    // Lines were scrubbed when recorded.
    recent.lines.iter().skip(skip).cloned().collect()
}

/// Write `report` as `crash-<UTC time>.txt` in `dir` and delete the oldest reports beyond
/// [`MAX_CRASH_REPORTS`]. Returns the new file's path.
pub fn write_crash_report(dir: &Path, report: &str) -> io::Result<PathBuf> {
    crate::persist::ensure_dir(dir)?;
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_else(|_| "unknown".into());
    let mut path = dir.join(format!("crash-{stamp}.txt"));
    let mut n = 1;
    let mut file = loop {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(f) => break f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 100 => {
                n += 1;
                path = dir.join(format!("crash-{stamp}-{n}.txt"));
            }
            Err(e) => return Err(e),
        }
    };
    file.write_all(report.as_bytes())?;
    let _ = file.sync_all();
    prune_crash_reports(dir, MAX_CRASH_REPORTS);
    Ok(path)
}

/// Names of the crash reports in `dir`, newest first.
pub fn crash_reports(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        .collect();
    // `crash-YYYYMMDD-HHMMSS[-n].txt`: the name order is the time order.
    names.sort_by_key(|n| std::cmp::Reverse(crash_sort_key(n)));
    names
}

fn crash_sort_key(name: &str) -> (String, u32) {
    let stem = name.trim_start_matches("crash-").trim_end_matches(".txt");
    // `YYYYMMDD-HHMMSS` is 15 characters; a `-n` suffix numbers reports of the same second.
    match (
        stem.get(..15),
        stem.get(15..).and_then(|s| s.strip_prefix('-')),
    ) {
        (Some(time), Some(n)) => (time.to_string(), n.parse().unwrap_or(0)),
        _ => (stem.to_string(), 1),
    }
}

fn prune_crash_reports(dir: &Path, keep: usize) {
    for old in crash_reports(dir).into_iter().skip(keep) {
        let _ = std::fs::remove_file(dir.join(old));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn log_files_rotate_and_keep_three() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("akev.log");
        let mut w = LogWriter::open(&path, 100, 3).unwrap();
        for i in 0..20 {
            writeln!(w, "line {i:02} {}", "x".repeat(30)).unwrap();
        }
        assert!(path.exists());
        assert!(dir.path().join("akev.log.1").exists());
        assert!(dir.path().join("akev.log.2").exists());
        assert!(!dir.path().join("akev.log.3").exists());
        for name in ["akev.log", "akev.log.1", "akev.log.2"] {
            let len = std::fs::metadata(dir.path().join(name)).unwrap().len();
            assert!(len <= 100, "{name} is {len} bytes");
        }
        // The newest line is in the current file, older ones in the rotated files.
        let current = std::fs::read_to_string(&path).unwrap();
        assert!(current.contains("line 19"), "{current}");
        let older = std::fs::read_to_string(dir.path().join("akev.log.1")).unwrap();
        assert!(!older.contains("line 19"));

        // Reopening appends to the current file.
        let before = std::fs::metadata(&path).unwrap().len();
        let mut again = LogWriter::open(&path, 1000, 3).unwrap();
        writeln!(again, "after").unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > before);
    }

    #[cfg(unix)]
    #[test]
    fn log_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("akev.log");
        let _w = LogWriter::open(&path, 100, 3).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn scrubbing_removes_credentials() {
        register_secret("s3cr3t-api-token-value");
        let s = scrub("GET /api/stream?token=abc123&x=1 with s3cr3t-api-token-value");
        assert!(!s.contains("abc123") && !s.contains("s3cr3t"), "{s}");
        assert!(s.contains("token=[redacted]&x=1"), "{s}");
        assert_eq!(
            scrub("?token=s3cr3t-api-token-value&y"),
            "?token=[redacted]&y"
        );
        let s = scrub("header Authorization: Bearer eyJhbGciOi.payload.sig\nnext");
        assert!(!s.contains("eyJ"), "{s}");
        assert!(s.ends_with("\nnext"), "{s}");
        let s = scrub("sent bearer abc.def and Cookie: session=1; theme=dark");
        assert!(!s.contains("abc.def") && !s.contains("session=1"), "{s}");
        let s = scrub("Proxy-Authorization: Basic dXNlcjpwYXNz");
        assert!(!s.contains("dXNlcjpwYXNz"), "{s}");
        assert_eq!(scrub("nothing secret here"), "nothing secret here");
        assert_eq!(scrub("token="), "token=");
        if let Some(home) = dirs::home_dir() {
            let s = scrub(&format!("opened {}/x/y.json", home.display()));
            assert_eq!(s, "opened ~/x/y.json");
        }
    }

    #[test]
    fn recent_events_keep_warnings_and_lines() {
        let subscriber = tracing_subscriber::registry().with(RecentEvents);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "akev_test", "an info line {}", 1);
            tracing::warn!(target: "akev_test", host = "example.com", "a warning token=zzz999");
            tracing::error!(target: "akev_test", "an error");
        });
        let lines = recent_lines(RECENT_LINES);
        assert!(lines.iter().any(|l| l.contains("an info line 1")));
        let warnings = recent_warnings();
        let w = warnings
            .iter()
            .find(|w| w.message.starts_with("a warning"))
            .unwrap();
        assert_eq!(w.level, "WARN");
        assert_eq!(w.target, "akev_test");
        assert!(
            w.message.contains("host=\"example.com\"") || w.message.contains("host=example.com")
        );
        assert!(!w.message.contains("zzz999"), "{}", w.message);
        assert!(
            warnings
                .iter()
                .any(|w| w.level == "ERROR" && w.message == "an error")
        );
        assert!(!warnings.iter().any(|w| w.message.contains("info line")));
    }

    #[test]
    fn crash_reports_are_written_scrubbed_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        register_secret("crash-secret-token-1234");
        let report = crash_report(&PanicDetails {
            thread: "akev-worker".into(),
            message: "index out of bounds (token=crash-secret-token-1234)".into(),
            location: "src/x.rs:1:2".into(),
            backtrace: "0: main".into(),
        });
        assert!(report.starts_with("Akev crash report"));
        assert!(report.contains(&format!("Version: {}", akev_core::VERSION)));
        assert!(report.contains("Thread: akev-worker"));
        assert!(report.contains("Location: src/x.rs:1:2"));
        assert!(report.contains("Backtrace:\n0: main"));
        assert!(report.contains("Recent log lines:"));
        assert!(!report.contains("crash-secret-token-1234"), "{report}");

        let first = write_crash_report(dir.path(), &report).unwrap();
        assert!(first.starts_with(dir.path()));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), report);
        // Same second: a numbered sibling, never an overwrite.
        let second = write_crash_report(dir.path(), "second").unwrap();
        assert_ne!(first, second);
        assert_eq!(crash_reports(dir.path()).len(), 2);
        assert_eq!(
            crash_reports(dir.path())[0],
            second.file_name().unwrap().to_string_lossy()
        );

        for i in 0..40 {
            std::fs::write(
                dir.path()
                    .join(format!("crash-20200101-0000{:02}.txt", i % 60)),
                "old",
            )
            .unwrap();
        }
        prune_crash_reports(dir.path(), MAX_CRASH_REPORTS);
        let left = crash_reports(dir.path());
        assert_eq!(left.len(), MAX_CRASH_REPORTS);
        // The newest survive.
        assert!(
            left.iter()
                .any(|n| *n == second.file_name().unwrap().to_string_lossy())
        );
        assert!(!left.contains(&"crash-20200101-000000.txt".to_string()));
    }
}
