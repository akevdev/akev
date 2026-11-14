//! Session library: named `.akev` sessions kept in one directory (the daemon uses
//! `data_dir/sessions/`), plus the [`Store`] operations saving, opening and autosaving need.
//!
//! ```text
//! sessions/
//!   checkout-bug.akev        the session (native format, see `session.rs`)
//!   checkout-bug.json         sidecar: name, kind, exchange count, created/updated times
//!   autosave.akev            the autosave (rewritten in place)
//!   autosave-previous.akev   the previous run's autosave (see SessionLibrary::rotate)
//! ```
//!
//! * Session ids are file-safe slugs of the name at creation (`checkout-bug`, `checkout-bug-2`)
//!   and never change; renaming only rewrites the sidecar.
//! * Every write is crash-safe: the session is streamed into a temp file in the same
//!   directory, fsynced and renamed over the target (then the directory is fsynced), so a
//!   crash leaves either the old or the new session, never half a file. Sidecars are written
//!   the same way.
//! * Memory stays flat: sessions are streamed one exchange at a time from the store to disk
//!   (and back when opened, as [`Store::load_session`] does).
//! * A session without a usable sidecar (copied in by hand, or a crash between the two
//!   renames) is still listed: its header gives the name and its lines are counted.

use crate::record::Record;
use crate::session;
use crate::{NewRecord, SessionLimits, Store, StoreError};
use akev_core::{ExchangeId, UnixMicros};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Id of the autosave session.
pub const AUTOSAVE_ID: &str = "autosave";
/// Id of the previous run's autosave (see [`SessionLibrary::rotate`]).
pub const AUTOSAVE_PREVIOUS_ID: &str = "autosave-previous";
/// Display name of the autosave session.
pub const AUTOSAVE_NAME: &str = "Autosave";
/// Display name of the previous run's autosave.
pub const AUTOSAVE_PREVIOUS_NAME: &str = "Autosave (previous run)";

/// Session file extension.
const EXT: &str = "akev";
/// Sidecar extension.
const META_EXT: &str = "json";
/// Prefix of in-flight temp files (ignored by listings, cleaned up when stale).
const TEMP_PREFIX: &str = ".tmp-";
/// In-flight temp files older than this are leftovers of a crash.
const STALE_TEMP: Duration = Duration::from_secs(3600);
/// Longest session name.
pub const MAX_NAME_CHARS: usize = 200;
/// Longest id (slug) generated from a name.
const MAX_SLUG: usize = 48;
/// Exchanges inserted per batch when loading a session file.
const LOAD_BATCH: usize = 256;

/// Errors of session library operations.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// No session with that id.
    #[error("session not found")]
    NotFound,
    /// Invalid input (name, id).
    #[error("{0}")]
    Invalid(String),
    /// Reading or writing the session data failed (corrupt file, I/O error...).
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<std::io::Error> for LibraryError {
    fn from(e: std::io::Error) -> Self {
        LibraryError::Store(StoreError::Io(e))
    }
}

/// Result alias for library operations.
pub type LibraryResult<T> = std::result::Result<T, LibraryError>;

/// What a session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    /// Saved by the user.
    Saved,
    /// Written by autosave ([`AUTOSAVE_ID`] / [`AUTOSAVE_PREVIOUS_ID`]).
    Autosave,
}

/// A session in the library (`GET /api/sessions`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Stable id (file name without extension).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Saved by the user or by autosave.
    pub kind: SessionKind,
    /// Number of exchanges in the session.
    pub exchanges: u64,
    /// Size of the session file in bytes (compressed).
    pub size: u64,
    /// When the session was first saved (unix ms).
    pub created_at: f64,
    /// When its contents were last written (unix ms).
    pub updated_at: f64,
}

/// How [`SessionLibrary::open`] combines a session with the current traffic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenMode {
    /// Remove all current traffic (pinned exchanges included), then load the session.
    #[default]
    Replace,
    /// Add the session's exchanges to the current traffic.
    Append,
}

/// The sidecar (`<id>.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    name: String,
    kind: SessionKind,
    exchanges: u64,
    /// Size of the session file this sidecar describes (a different size means the file was
    /// replaced behind our back: it's rescanned).
    size: u64,
    created_at: f64,
    updated_at: f64,
}

/// A directory of named sessions. Cheap to create; operations are blocking (file I/O and
/// compression), so async callers run them on a blocking thread.
#[derive(Debug)]
pub struct SessionLibrary {
    dir: PathBuf,
    /// Serializes the commit phase of changes (id allocation, renames, sidecars).
    lock: Mutex<()>,
}

impl SessionLibrary {
    /// A library in `dir` (created on first write). Removes stale temp files of interrupted
    /// writes.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let library = SessionLibrary {
            dir: dir.into(),
            lock: Mutex::new(()),
        };
        library.remove_stale_temps();
        library
    }

    /// The library directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// All sessions, most recently updated first.
    pub fn list(&self) -> LibraryResult<Vec<SessionInfo>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXT) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_valid_id(id) {
                continue;
            }
            match self.info(id) {
                Ok(info) => out.push(info),
                // Deleted meanwhile, unreadable or corrupt: not listed.
                Err(e) => tracing::debug!(id, error = %e, "session not listed"),
            }
        }
        out.sort_by(|a, b| {
            b.updated_at
                .total_cmp(&a.updated_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// One session's info.
    pub fn info(&self, id: &str) -> LibraryResult<SessionInfo> {
        let path = self.session_path(id)?;
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(LibraryError::NotFound);
            }
            Err(e) => return Err(e.into()),
        };
        let file_meta = file.metadata()?;
        let size = file_meta.len();
        if let Some(meta) = self.read_meta(id).filter(|m| m.size == size) {
            return Ok(info_from(id, meta));
        }
        // No (current) sidecar: describe the file from its contents (the handle opened above,
        // even if the file is replaced meanwhile) and remember it.
        let scanned = session::scan(&mut BufReader::new(&file), SessionLimits::default())?;
        let modified = file_meta
            .modified()
            .map(system_ms)
            .unwrap_or_else(|_| now_ms());
        let meta = Meta {
            name: scanned
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| default_name(id)),
            kind: kind_of(id),
            exchanges: scanned.exchanges,
            size,
            created_at: scanned
                .created_at
                .and_then(|c| UnixMicros::from_rfc3339(&c))
                .map(|t| t.0 as f64 / 1000.0)
                .unwrap_or(modified),
            updated_at: modified,
        };
        {
            let _guard = self.lock.lock();
            // A save may have committed meanwhile (its sidecar wins), or replaced the file:
            // only remember what describes the file on disk now.
            let current = file_size(&path).ok();
            if let Some(fresh) = self.read_meta(id).filter(|m| Some(m.size) == current) {
                return Ok(info_from(id, fresh));
            }
            if current == Some(size)
                && let Err(e) = self.write_meta(id, &meta)
            {
                tracing::debug!(id, error = %e, "could not write session sidecar");
            }
        }
        Ok(info_from(id, meta))
    }

    /// Path of a session's file (it may not exist). Fails for invalid ids.
    pub fn session_path(&self, id: &str) -> LibraryResult<PathBuf> {
        if !is_valid_id(id) {
            return Err(LibraryError::NotFound);
        }
        Ok(self.dir.join(format!("{id}.{EXT}")))
    }

    /// Save the store's traffic (all, or `ids`) as a new session named `name`.
    pub fn save(
        &self,
        store: &Store,
        name: &str,
        ids: Option<&[ExchangeId]>,
    ) -> LibraryResult<SessionInfo> {
        let name = validate_name(name)?;
        let (temp, exchanges) = self.write_temp(store, &name, ids)?;
        let _guard = self.lock.lock();
        let id = self.unique_id(&slug(&name));
        let path = self.session_path(&id)?;
        self.commit(temp, &path)?;
        let now = now_ms();
        let meta = Meta {
            name,
            kind: SessionKind::Saved,
            exchanges,
            size: file_size(&path)?,
            created_at: now,
            updated_at: now,
        };
        self.write_meta(&id, &meta)?;
        Ok(info_from(&id, meta))
    }

    /// Write the autosave ([`AUTOSAVE_ID`]): the store's traffic (all, or `ids`) replaces the
    /// previous autosave atomically.
    pub fn autosave(
        &self,
        store: &Store,
        ids: Option<&[ExchangeId]>,
    ) -> LibraryResult<SessionInfo> {
        let (temp, exchanges) = self.write_temp(store, AUTOSAVE_NAME, ids)?;
        let _guard = self.lock.lock();
        let path = self.session_path(AUTOSAVE_ID)?;
        let created_at = self
            .read_meta(AUTOSAVE_ID)
            .filter(|_| path.exists())
            .map(|m| m.created_at);
        self.commit(temp, &path)?;
        let now = now_ms();
        let meta = Meta {
            name: AUTOSAVE_NAME.into(),
            kind: SessionKind::Autosave,
            exchanges,
            size: file_size(&path)?,
            created_at: created_at.unwrap_or(now),
            updated_at: now,
        };
        self.write_meta(AUTOSAVE_ID, &meta)?;
        Ok(info_from(AUTOSAVE_ID, meta))
    }

    /// Keep the current autosave as [`AUTOSAVE_PREVIOUS_ID`] (replacing an older one), so the
    /// next autosave doesn't overwrite the traffic of the previous run. Returns whether there
    /// was an autosave to move.
    pub fn rotate(&self) -> LibraryResult<bool> {
        let _guard = self.lock.lock();
        let from = self.session_path(AUTOSAVE_ID)?;
        if !from.exists() {
            return Ok(false);
        }
        let to = self.session_path(AUTOSAVE_PREVIOUS_ID)?;
        let meta = self.read_meta(AUTOSAVE_ID);
        std::fs::rename(&from, &to)?;
        let _ = std::fs::remove_file(self.meta_path(AUTOSAVE_ID));
        if let Some(mut meta) = meta {
            meta.name = AUTOSAVE_PREVIOUS_NAME.into();
            meta.kind = SessionKind::Autosave;
            self.write_meta(AUTOSAVE_PREVIOUS_ID, &meta)?;
        } else {
            let _ = std::fs::remove_file(self.meta_path(AUTOSAVE_PREVIOUS_ID));
        }
        sync_dir(&self.dir);
        Ok(true)
    }

    /// Rename a session. Renaming an autosave keeps it: it becomes a saved session with a new
    /// id (so the next autosave can't overwrite it) — the returned info carries that id.
    pub fn rename(&self, id: &str, name: &str) -> LibraryResult<SessionInfo> {
        let name = validate_name(name)?;
        let current = self.info(id)?;
        let _guard = self.lock.lock();
        let mut meta = Meta {
            name,
            kind: current.kind,
            exchanges: current.exchanges,
            size: current.size,
            created_at: current.created_at,
            updated_at: current.updated_at,
        };
        if current.kind == SessionKind::Saved {
            self.write_meta(id, &meta)?;
            return Ok(info_from(id, meta));
        }
        let new_id = self.unique_id(&slug(&meta.name));
        std::fs::rename(self.session_path(id)?, self.session_path(&new_id)?)?;
        meta.kind = SessionKind::Saved;
        self.write_meta(&new_id, &meta)?;
        let _ = std::fs::remove_file(self.meta_path(id));
        sync_dir(&self.dir);
        Ok(info_from(&new_id, meta))
    }

    /// Delete a session (file and sidecar).
    pub fn delete(&self, id: &str) -> LibraryResult<()> {
        let path = self.session_path(id)?;
        let _guard = self.lock.lock();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(LibraryError::NotFound);
            }
            Err(e) => return Err(e.into()),
        }
        let _ = std::fs::remove_file(self.meta_path(id));
        sync_dir(&self.dir);
        Ok(())
    }

    /// Load a session into the store (replacing or appending to the current traffic). The
    /// session is fully validated before the current traffic is touched, then inserted in
    /// batches (memory stays bounded: bodies beyond the budget spill as it goes). Returns the
    /// new ids.
    pub fn open(&self, id: &str, store: &Store, mode: OpenMode) -> LibraryResult<Vec<ExchangeId>> {
        let path = self.existing(id)?;
        Ok(store.load_session_file(&path, SessionLimits::default(), mode, None)?)
    }

    /// [`SessionLibrary::open`] (appending) with ids reserved beforehand
    /// ([`ExchangeId::reserve`]): the first `reserved` exchanges get `first`, `first + 1`...
    /// so they sort before traffic captured while the session was loading.
    pub fn restore(
        &self,
        id: &str,
        store: &Store,
        first: ExchangeId,
        reserved: u64,
    ) -> LibraryResult<Vec<ExchangeId>> {
        let path = self.existing(id)?;
        Ok(store.load_session_file(
            &path,
            SessionLimits::default(),
            OpenMode::Append,
            Some((first, reserved)),
        )?)
    }

    /// The path of an existing session.
    fn existing(&self, id: &str) -> LibraryResult<PathBuf> {
        let path = self.session_path(id)?;
        if path.is_file() {
            Ok(path)
        } else {
            Err(LibraryError::NotFound)
        }
    }

    /// Open a session's file for reading (e.g. to download it).
    pub fn open_file(&self, id: &str) -> LibraryResult<File> {
        match File::open(self.session_path(id)?) {
            Ok(f) => Ok(f),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(LibraryError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    // -----------------------------------------------------------------------------------

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.{META_EXT}"))
    }

    fn read_meta(&self, id: &str) -> Option<Meta> {
        let bytes = std::fs::read(self.meta_path(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write a sidecar atomically. Call with the lock held.
    fn write_meta(&self, id: &str, meta: &Meta) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(meta).map_err(std::io::Error::from)?;
        let mut temp = self.temp_file()?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        self.commit(temp, &self.meta_path(id))
    }

    fn temp_file(&self) -> std::io::Result<tempfile::NamedTempFile> {
        std::fs::create_dir_all(&self.dir)?;
        tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .suffix(".part")
            .tempfile_in(&self.dir)
    }

    /// Stream a session into a new temp file (no lock held: other operations continue).
    fn write_temp(
        &self,
        store: &Store,
        name: &str,
        ids: Option<&[ExchangeId]>,
    ) -> LibraryResult<(tempfile::NamedTempFile, u64)> {
        let mut temp = self.temp_file()?;
        let written = {
            let mut out = BufWriter::with_capacity(64 * 1024, temp.as_file_mut());
            let written = store.save_session_named(&mut out, ids, Some(name))?;
            out.flush()?;
            written
        };
        temp.as_file().sync_all()?;
        Ok((temp, written as u64))
    }

    /// Move a finished temp file into place (atomic replace) and make the rename durable.
    fn commit(&self, temp: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
        temp.persist(path).map_err(|e| e.error)?;
        sync_dir(&self.dir);
        Ok(())
    }

    /// `base`, `base-2`, `base-3`... — the first id no session uses (nor the autosave ids).
    fn unique_id(&self, base: &str) -> String {
        let taken = |id: &str| {
            id == AUTOSAVE_ID
                || id == AUTOSAVE_PREVIOUS_ID
                || self.dir.join(format!("{id}.{EXT}")).exists()
                || self.meta_path(id).exists()
        };
        if !taken(base) {
            return base.to_string();
        }
        (2u64..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !taken(id))
            .unwrap_or_else(|| base.to_string())
    }

    fn remove_stale_temps(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with(TEMP_PREFIX) {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMP);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn info_from(id: &str, meta: Meta) -> SessionInfo {
    SessionInfo {
        id: id.to_string(),
        name: meta.name,
        kind: meta.kind,
        exchanges: meta.exchanges,
        size: meta.size,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
    }
}

fn kind_of(id: &str) -> SessionKind {
    if id == AUTOSAVE_ID || id == AUTOSAVE_PREVIOUS_ID {
        SessionKind::Autosave
    } else {
        SessionKind::Saved
    }
}

fn default_name(id: &str) -> String {
    match id {
        AUTOSAVE_ID => AUTOSAVE_NAME.into(),
        AUTOSAVE_PREVIOUS_ID => AUTOSAVE_PREVIOUS_NAME.into(),
        other => other.to_string(),
    }
}

fn file_size(path: &Path) -> std::io::Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

use akev_core::time::now_ms;

fn system_ms(t: SystemTime) -> f64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Make renames in `dir` durable (best effort; a no-op where directories can't be synced).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Whether `id` is a valid session id: 1–64 of `a-z`, `0-9`, `-`, not starting with `-`.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A trimmed session name, or why it's not acceptable.
pub fn validate_name(name: &str) -> LibraryResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LibraryError::Invalid(
            "the session name must not be empty".into(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(LibraryError::Invalid(format!(
            "the session name must be at most {MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(LibraryError::Invalid(
            "the session name must not contain control characters".into(),
        ));
    }
    Ok(name.to_string())
}

/// File-safe id for a name: lowercase ASCII letters and digits, other runs become `-`
/// (`Checkout bug #2` → `checkout-bug-2`); `session` when nothing is left.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= MAX_SLUG {
            break;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------------------
// Store operations used by the library and autosave
// ---------------------------------------------------------------------------------------

impl Store {
    /// A counter that changes whenever the stored traffic visibly changes (exchanges added,
    /// updated, deleted, cleared, patched). Autosave compares it to skip unchanged traffic.
    pub fn generation(&self) -> u64 {
        self.inner.read().generation
    }

    /// [`Store::save_session`] with a session name in the header; returns the number of
    /// exchanges written.
    pub fn save_session_named(
        &self,
        writer: &mut dyn std::io::Write,
        ids: Option<&[ExchangeId]>,
        name: Option<&str>,
    ) -> crate::Result<usize> {
        self.save_session_with(writer, ids, name, crate::ExportOptions::default())
    }

    /// [`Store::save_session_named`] with export options (e.g. secrets redacted).
    pub fn save_session_with(
        &self,
        writer: &mut dyn std::io::Write,
        ids: Option<&[ExchangeId]>,
        name: Option<&str>,
        options: crate::ExportOptions,
    ) -> crate::Result<usize> {
        let ids = self.resolve_ids(ids);
        session::save(
            writer,
            ids.into_iter()
                .filter_map(|id| self.snapshot(id))
                .map(|s| options.apply(s)),
            name,
        )
    }

    /// Load a session file: a first pass validates every line (nothing is inserted from an
    /// invalid file, and with [`OpenMode::Replace`] the current traffic — pinned exchanges
    /// included — is only cleared once it passed), a second inserts the exchanges in batches,
    /// so memory stays around one batch however large the session is (bodies past the budget
    /// spill as it goes). With `reserved = Some((first, n))` the first `n` exchanges get the
    /// ids `first`, `first + 1`... ([`ExchangeId::reserve`]), any beyond fresh ones.
    pub fn load_session_file(
        &self,
        path: &Path,
        limits: SessionLimits,
        mode: OpenMode,
        reserved: Option<(ExchangeId, u64)>,
    ) -> crate::Result<Vec<ExchangeId>> {
        // Both passes read the same file (handle), even if it's replaced meanwhile.
        let file = File::open(path)?;
        let open = || -> crate::Result<BufReader<&File>> {
            use std::io::Seek;
            (&file).seek(std::io::SeekFrom::Start(0))?;
            Ok(BufReader::with_capacity(64 * 1024, &file))
        };
        session::for_each(&mut open()?, limits, &mut drop)?;
        if mode == OpenMode::Replace {
            self.clear(false);
        }
        let (first, reserved) = reserved.unwrap_or((ExchangeId(0), 0));
        let mut next = 0u64;
        let mut next_id = || {
            let id = if next < reserved {
                ExchangeId(first.0 + next)
            } else {
                ExchangeId::next()
            };
            next += 1;
            id
        };
        let mut ids = Vec::new();
        let mut batch = Vec::with_capacity(LOAD_BATCH);
        session::for_each(&mut open()?, limits, &mut |record| {
            batch.push(record);
            if batch.len() >= LOAD_BATCH {
                ids.extend(self.insert_records_with(std::mem::take(&mut batch), &mut next_id));
            }
        })?;
        ids.extend(self.insert_records_with(batch, &mut next_id));
        Ok(ids)
    }

    /// The exchanges an autosave bounded by `max_exchanges` / `max_bytes` keeps (0 = no
    /// limit): the newest ones that fit (captured body bytes counted), plus every pinned
    /// exchange. `None` when everything fits.
    pub fn newest_within(&self, max_exchanges: u64, max_bytes: u64) -> Option<Vec<ExchangeId>> {
        if max_exchanges == 0 && max_bytes == 0 {
            return None;
        }
        let inner = self.inner.read();
        let mut kept = Vec::new();
        let mut count = 0u64;
        let mut bytes = 0u64;
        let mut full = false;
        let mut dropped = false;
        for (id, rec) in inner.records.iter().rev() {
            if rec.summary.pinned {
                kept.push(*id);
                continue;
            }
            if !full {
                let size = record_bytes(rec);
                let over_count = max_exchanges != 0 && count >= max_exchanges;
                let over_bytes = max_bytes != 0 && bytes.saturating_add(size) > max_bytes;
                if over_count || over_bytes {
                    full = true;
                } else {
                    count += 1;
                    bytes = bytes.saturating_add(size);
                    kept.push(*id);
                    continue;
                }
            }
            dropped = true;
        }
        if !dropped {
            return None;
        }
        kept.reverse();
        Some(kept)
    }

    /// [`Store::insert_records`] with ids from `next_id`.
    fn insert_records_with(
        &self,
        items: Vec<NewRecord>,
        next_id: &mut dyn FnMut() -> ExchangeId,
    ) -> Vec<ExchangeId> {
        let records: Vec<Box<Record>> = items
            .into_iter()
            .map(|n| Box::new(Record::from_new(next_id(), n)))
            .collect();
        let ids = records.iter().map(|r| r.id()).collect();
        let over = {
            let mut guard = self.inner.write();
            let notify = self.has_listeners();
            for rec in records {
                self.add_record(&mut guard, rec, notify);
            }
            guard.over_budget()
        };
        if over {
            self.maybe_spill();
        }
        ids
    }
}

/// Captured body and message bytes of a record (what it adds to a session, roughly).
fn record_bytes(rec: &Record) -> u64 {
    let messages: u64 = rec.messages.iter().map(|m| m.message.size).sum();
    rec.request_body.captured_len() + rec.response_body.captured_len() + messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Checkout bug #2"), "checkout-bug-2");
        assert_eq!(slug("  --Hello__World--  "), "hello-world");
        assert_eq!(slug("日本語"), "session");
        assert_eq!(slug(""), "session");
        assert_eq!(slug("ÄPI Tests"), "pi-tests");
        let long = slug(&"abc ".repeat(40));
        assert!(long.len() <= MAX_SLUG && !long.ends_with('-'), "{long}");
        assert!(is_valid_id(&long));
    }

    #[test]
    fn ids_are_validated() {
        for ok in ["autosave", "a", "checkout-bug-2", "0"] {
            assert!(is_valid_id(ok), "{ok}");
        }
        for bad in ["", "-x", "A", "a/b", "../x", "a.b", "a b", &"x".repeat(65)] {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn names_are_validated() {
        assert_eq!(validate_name("  My session ").unwrap(), "My session");
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
        assert!(validate_name(&"é".repeat(MAX_NAME_CHARS)).is_ok());
    }
}
