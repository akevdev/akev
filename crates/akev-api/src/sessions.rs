//! Saved sessions (`data_dir/sessions/`, see `akev_store::SessionLibrary`) and autosave.
//!
//! **Autosave** (opt-in, `settings.sessions`): a background task writes the captured traffic
//! to the "Autosave" session every `autosaveIntervalSecs` while it changed (the store's
//! generation counter), and once more on graceful shutdown. Writes stream from the store to
//! a temp file on the blocking pool (the proxy only ever pushes events into the store's
//! channel, so capture never waits for a save) and replace the previous autosave atomically.
//! An autosave keeps the newest exchanges within `autosaveMaxExchanges` /
//! `autosaveMaxBytes` (pinned ones always).
//!
//! The first autosave of a run moves the previous run's autosave aside to "Autosave
//! (previous run)" unless it was restored into this run, so it's never silently overwritten.
//!
//! **Restore on startup** (`restoreOnStartup`, while autosave is on): ids for the autosave's
//! exchanges are reserved before the proxy starts, then the session loads in the background,
//! so the API comes up at once and restored traffic still sorts before anything captured
//! meanwhile. When it's done, `Status.restoredSession` describes it and connected clients get
//! a `session-restored` message.

use crate::app::App;
use crate::stream::messages::ServerMessage;
use akev_core::ExchangeId;
use akev_core::settings::SessionSettings;
use akev_store::{AUTOSAVE_ID, SessionInfo, SessionLibrary};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Name of the session library directory in the data directory.
pub const SESSIONS_DIR: &str = "sessions";

/// How long the final autosave on shutdown may take.
const SHUTDOWN_SAVE_TIMEOUT: Duration = Duration::from_secs(120);

/// The session restored at startup (`Status.restoredSession`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredSession {
    /// Library id (`autosave`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Exchanges loaded.
    pub exchanges: u64,
    /// When the session was saved (unix ms).
    pub saved_at: f64,
}

/// A restore prepared before the proxy starts (ids reserved).
#[derive(Debug)]
pub(crate) struct PendingRestore {
    info: SessionInfo,
    first: ExchangeId,
}

/// Session library + autosave state of the running app.
#[derive(Debug)]
pub(crate) struct Sessions {
    pub library: Arc<SessionLibrary>,
    /// Store generation written by the last successful autosave.
    saved_generation: Mutex<Option<u64>>,
    /// The autosave file holds this run's traffic (written or restored by it).
    owns_autosave: AtomicBool,
    /// A restore was prepared and hasn't finished: autosaves wait for it (writing before it
    /// loaded would replace the session being restored).
    restore_pending: AtomicBool,
    /// Serializes restore and autosave writes.
    writing: tokio::sync::Mutex<()>,
    /// Wakes the autosave loop (settings changed).
    wake: tokio::sync::Notify,
    restored: RwLock<Option<RestoredSession>>,
    /// Last autosave error reported to the user (reported again only when it changes).
    last_error: Mutex<Option<String>>,
}

impl Sessions {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Sessions {
            library: Arc::new(SessionLibrary::new(dir)),
            saved_generation: Mutex::new(None),
            owns_autosave: AtomicBool::new(false),
            restore_pending: AtomicBool::new(false),
            writing: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            restored: RwLock::new(None),
            last_error: Mutex::new(None),
        }
    }

    /// The session restored at startup, once loaded.
    pub(crate) fn restored(&self) -> Option<RestoredSession> {
        self.restored.read().clone()
    }

    /// Settings changed: the autosave loop re-reads its interval.
    pub(crate) fn settings_changed(&self) {
        self.wake.notify_one();
    }

    /// When the settings ask for it and an autosave exists, reserve ids for its exchanges.
    /// Call before the proxy starts (blocking: reads the sidecar, or scans a session without
    /// one).
    pub(crate) fn prepare_restore(&self, settings: &SessionSettings) -> Option<PendingRestore> {
        if !(settings.autosave && settings.restore_on_startup) {
            return None;
        }
        let info = match self.library.info(AUTOSAVE_ID) {
            Ok(info) => info,
            Err(akev_store::LibraryError::NotFound) => return None,
            Err(e) => {
                tracing::warn!("cannot restore the autosaved session: {e}");
                return None;
            }
        };
        let first = ExchangeId::reserve(info.exchanges);
        self.restore_pending.store(true, Ordering::Release);
        Some(PendingRestore { info, first })
    }

    /// Load a prepared restore into the store (in the background of a running app).
    async fn restore(&self, app: &Arc<App>, pending: PendingRestore) {
        let _writing = self.writing.lock().await;
        let library = self.library.clone();
        let store = app.store.clone();
        let PendingRestore { info, first } = pending;
        let reserved = info.exchanges;
        let loaded = tokio::task::spawn_blocking(move || {
            library.restore(AUTOSAVE_ID, &store, first, reserved)
        })
        .await;
        match loaded {
            Ok(Ok(ids)) => {
                // The autosave now lives in the store: later autosaves may replace it.
                self.owns_autosave.store(true, Ordering::Release);
                let restored = RestoredSession {
                    id: info.id,
                    name: info.name,
                    exchanges: ids.len() as u64,
                    saved_at: info.updated_at,
                };
                tracing::info!(
                    exchanges = restored.exchanges,
                    "restored the autosaved session"
                );
                *self.restored.write() = Some(restored.clone());
                app.broadcast(&ServerMessage::SessionRestored { session: &restored });
            }
            Ok(Err(e)) => {
                tracing::warn!("could not restore the autosaved session: {e}");
                app.notice(
                    "warning",
                    &format!("Could not restore the autosaved session: {e}"),
                );
            }
            Err(e) => tracing::warn!("restore task failed: {e}"),
        }
        self.restore_pending.store(false, Ordering::Release);
    }

    /// Write the autosave now if the traffic changed since the last one. Returns the new
    /// session info when something was written.
    pub(crate) async fn autosave(
        &self,
        app: &App,
        settings: &SessionSettings,
    ) -> Option<SessionInfo> {
        let _writing = self.writing.lock().await;
        if self.restore_pending.load(Ordering::Acquire) {
            return None;
        }
        let store = app.store.clone();
        let generation = store.generation();
        if *self.saved_generation.lock() == Some(generation) {
            return None;
        }
        let owns = self.owns_autosave.load(Ordering::Acquire);
        if !owns && store.stats().exchanges == 0 {
            // Nothing captured yet: keep the previous run's autosave as it is.
            *self.saved_generation.lock() = Some(generation);
            return None;
        }
        let library = self.library.clone();
        let (max_exchanges, max_bytes) =
            (settings.autosave_max_exchanges, settings.autosave_max_bytes);
        let written = tokio::task::spawn_blocking(move || {
            if !owns {
                library.rotate()?;
            }
            let ids = store.newest_within(max_exchanges, max_bytes);
            library.autosave(&store, ids.as_deref())
        })
        .await;
        match written {
            Ok(Ok(info)) => {
                self.owns_autosave.store(true, Ordering::Release);
                *self.saved_generation.lock() = Some(generation);
                *self.last_error.lock() = None;
                tracing::debug!(
                    exchanges = info.exchanges,
                    bytes = info.size,
                    "autosaved traffic"
                );
                Some(info)
            }
            Ok(Err(e)) => {
                self.report_error(app, e.to_string());
                None
            }
            Err(e) => {
                self.report_error(app, format!("autosave task failed: {e}"));
                None
            }
        }
    }

    fn report_error(&self, app: &App, error: String) {
        tracing::warn!("autosave failed: {error}");
        let mut last = self.last_error.lock();
        if last.as_deref() != Some(error.as_str()) {
            app.notice("warning", &format!("Autosave failed: {error}"));
            *last = Some(error);
        }
    }

    /// The final autosave on graceful shutdown (bounded in time).
    pub(crate) async fn autosave_on_shutdown(&self, app: &App) {
        let settings = app.settings().sessions;
        if !settings.autosave {
            return;
        }
        if tokio::time::timeout(SHUTDOWN_SAVE_TIMEOUT, self.autosave(app, &settings))
            .await
            .is_err()
        {
            tracing::warn!("timed out writing the final autosave");
        }
    }
}

/// Start the autosave task (restoring a prepared session first).
pub(crate) fn spawn(app: &Arc<App>, pending: Option<PendingRestore>) {
    let a = app.clone();
    let task = tokio::spawn(async move {
        let sessions = a.sessions.clone();
        if let Some(pending) = pending {
            sessions.restore(&a, pending).await;
        }
        let mut shutdown = a.shutdown_receiver();
        loop {
            if *shutdown.borrow() {
                break;
            }
            let settings = a.settings().sessions;
            let wait = if settings.autosave {
                Duration::from_secs(settings.autosave_interval_secs.max(1))
            } else {
                Duration::from_secs(3600)
            };
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                    continue;
                }
                _ = sessions.wake.notified() => continue,
                _ = tokio::time::sleep(wait) => {}
            }
            let settings = a.settings().sessions;
            if settings.autosave {
                sessions.autosave(&a, &settings).await;
            }
        }
    });
    app.tasks.lock().push(task);
}

/// Session library directory for a data directory.
pub(crate) fn library_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join(SESSIONS_DIR)
}
