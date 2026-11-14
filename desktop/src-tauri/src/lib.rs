//! Akev desktop shell.
//!
//! The desktop app runs the complete Akev core (`akev_api::start`: proxy, store, rules,
//! interceptors, API) in-process on a random loopback port, then opens a native window
//! pointing at the UI served by that core. The session token and desktop flag are injected
//! via `window.__AKEV__` before any page script runs, so the UI is byte-for-byte the same as
//! the browser build but can use native features (`window.__TAURI__`: file dialogs,
//! notifications, opener) when present. Copying uses the web clipboard API, so there's no
//! clipboard plugin (its Wayland probing only logged warnings on GNOME).
//!
//! Native menu events (macOS menu bar) are forwarded to the UI as the `akev://menu` event
//! with the menu item id as payload (`nav-traffic`, `import`, `export-har`, `save-session`,
//! `settings`, `command-palette`, `toggle-recording`, `clear-traffic`, `compose-new`,
//! `shortcuts`).

use std::sync::Arc;

use parking::Zoom;
use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, Wry};

mod parking {
    /// Current zoom factor of the main webview.
    pub struct Zoom(pub std::sync::Mutex<f64>);
}

struct DesktopState {
    akev: Arc<akev_api::AkevHandle>,
}

/// How the main window is framed, passed to the UI as `window.__AKEV__.windowChrome`.
///
/// On Linux and Windows the system title bar doesn't match the app (GTK draws a plain one
/// inside the window under Wayland), so by default the window has no system decorations and
/// the UI draws the window buttons in its top toolbar: on Linux in the order of the desktop's
/// `gtk-decoration-layout`, on Windows minimise, maximise, close. The Settings → Title bar
/// choice ("system") brings the system title bar back, as does a Linux layout with buttons on
/// the left (the UI only places them on the right). macOS keeps its native title bar.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowChrome {
    /// The window starts without system decorations.
    custom: bool,
    /// `gtk-decoration-layout`, e.g. `appmenu:minimize,maximize,close` (Linux only; without
    /// one the UI draws minimise, maximise, close on the right).
    layout: Option<String>,
}

/// Settings → Title bar is "System" (the UI preference `titleBar`).
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn prefers_system_title_bar(settings: &akev_core::settings::Settings) -> bool {
    settings.ui.get("titleBar").and_then(|v| v.as_str()) == Some("system")
}

#[cfg(target_os = "linux")]
fn window_chrome(settings: &akev_core::settings::Settings) -> WindowChrome {
    use gtk::prelude::GtkSettingsExt;
    let layout = gtk::Settings::default()
        .and_then(|s| s.gtk_decoration_layout())
        .map(|s| s.to_string());
    let custom =
        !prefers_system_title_bar(settings) && layout.as_deref().is_none_or(buttons_on_the_right);
    WindowChrome { custom, layout }
}

/// Undecorated windows stay resizable from their edges (Tauri handles that on Windows) and
/// keep the system shadow, and rounded corners on Windows 11.
#[cfg(windows)]
fn window_chrome(settings: &akev_core::settings::Settings) -> WindowChrome {
    WindowChrome {
        custom: !prefers_system_title_bar(settings),
        layout: None,
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn window_chrome(_settings: &akev_core::settings::Settings) -> WindowChrome {
    WindowChrome {
        custom: false,
        layout: None,
    }
}

/// Whether a `gtk-decoration-layout` puts no window buttons on the left (`left:right`; no
/// colon means everything is on the left).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn buttons_on_the_right(layout: &str) -> bool {
    let left = layout.split_once(':').map_or(layout, |(left, _)| left);
    !left
        .split(',')
        .any(|b| matches!(b.trim(), "minimize" | "maximize" | "close"))
}

/// Header carrying the suggested file name (URI-encoded) for [`save_file`].
const FILE_NAME_HEADER: &str = "x-akev-file-name";

/// Save a file the UI produced (an export, a body, generated code) where the user chooses.
///
/// The UI sends the bytes as the raw request body and the suggested name in
/// [`FILE_NAME_HEADER`]. The native Save dialog is shown here rather than in the page, so the
/// page can only ever write to a path the user has just picked. `false` when cancelled.
#[tauri::command]
async fn save_file(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("expected the file's bytes as the request body".into());
    };
    let data = data.clone();
    let name = request
        .headers()
        .get(FILE_NAME_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(suggested_file_name)
        .unwrap_or_else(|| "download".into());
    tracing::debug!(name, bytes = data.len(), "save requested");

    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_file_name(&name);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file(move |path| {
        let _ = tx.send(path);
    });
    let Some(path) = rx.await.map_err(|e| e.to_string())? else {
        tracing::debug!(name, "save cancelled");
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, data))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            tracing::warn!(error = %e, "saving a file failed");
            format!("couldn't save the file: {e}")
        })?;
    tracing::debug!(name, "file saved");
    Ok(true)
}

/// Download and install the newest release, then restart into it. The updater verifies the
/// package's signature against the public key in `tauri.conf.json` before installing. `false`
/// when there is nothing newer.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("couldn't check for the update: {e}"))?
    else {
        return Ok(false);
    };
    tracing::info!(version = %update.version, "installing update");
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("couldn't install the update: {e}"))?;
    // Restore the system proxy and stop launched browsers before the new version starts.
    if let Some(state) = app.try_state::<DesktopState>() {
        state.akev.shutdown().await;
    }
    app.restart()
}

/// The file name to suggest: URI-decoded, last path component only, never empty.
fn suggested_file_name(encoded: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy();
    let name = decoded
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches('.')
        .to_string();
    if name.is_empty() {
        "download".into()
    } else {
        name
    }
}

/// Log to stderr and to `<data dir>/logs/akev.log`, remember recent events for diagnostics,
/// and write crash reports to `<data dir>/crashes/`.
fn init_tracing() {
    use akev_api::observability;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,akev=info"));
    let stderr = tracing_subscriber::fmt::layer()
        .log_internal_errors(false)
        .with_filter(filter);
    let data_dir = akev_api::default_data_dir();
    let file = data_dir
        .as_deref()
        .and_then(|dir| match observability::open_log(dir) {
            Ok(writer) => Some(
                observability::file_layer(writer).with_filter(observability::file_filter(None)),
            ),
            Err(e) => {
                eprintln!("Can't write the log file in {}: {e}", dir.display());
                None
            }
        });
    let recent = observability::RecentEvents.with_filter(observability::file_filter(None));
    let _ = tracing_subscriber::registry()
        .with(stderr)
        .with(file)
        .with(recent)
        .try_init();
    if let Some(dir) = data_dir {
        observability::install_panic_hook(dir.join(observability::CRASH_DIR));
    }
}

/// Text of the dialog shown when the Akev core can't start.
fn startup_error_message(error: &anyhow::Error, log_file: Option<&std::path::Path>) -> String {
    let mut reason = format!("{error:#}");
    if let Some(first) = reason.get(..1) {
        let upper = first.to_uppercase();
        reason.replace_range(..1, &upper);
    }
    if !reason.ends_with('.') {
        reason.push('.');
    }
    match log_file {
        Some(path) => format!(
            "{reason}\n\nThe log file has more details: {}",
            path.display()
        ),
        None => reason,
    }
}

/// Show a native error dialog for a failed start, then quit when it's closed.
fn report_startup_failure(app: &AppHandle, error: &anyhow::Error) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    tracing::error!("Akev couldn't start: {error:#}");
    let log_file = akev_api::observability::log_file_path();
    let message = startup_error_message(error, log_file.as_deref());
    let handle = app.clone();
    app.dialog()
        .message(message)
        .title("Akev couldn't start")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::Ok)
        .show(move |_| handle.exit(1));
}

/// Capture files Akev was asked to open before it was ready (macOS can send them early).
struct PendingFiles(std::sync::Mutex<Vec<std::path::PathBuf>>);

/// File types Akev opens (the bundle's file associations).
const CAPTURE_EXTENSIONS: &[&str] = &["har", "akev", "chlsj", "saz"];

/// The capture files among command-line arguments (file associations on Windows and Linux
/// start the app, or the running instance, with the file's path).
fn capture_files<S: AsRef<str>>(args: impl IntoIterator<Item = S>) -> Vec<std::path::PathBuf> {
    args.into_iter()
        .skip(1)
        .filter(|a| !a.as_ref().starts_with('-'))
        .map(|a| std::path::PathBuf::from(a.as_ref()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| CAPTURE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect()
}

/// Import capture files into the traffic list, then tell the UI (`akev://imported`, with the
/// file, how many exchanges and the first id, or the error).
fn open_files(app: &AppHandle, paths: Vec<std::path::PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<DesktopState>() else {
        if let Some(pending) = app.try_state::<PendingFiles>()
            && let Ok(mut list) = pending.0.lock()
        {
            list.extend(paths);
        }
        return;
    };
    let handle = state.akev.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for path in paths {
            let h = handle.clone();
            let p = path.clone();
            let result = tauri::async_runtime::spawn_blocking(move || h.import_file(&p)).await;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let payload = match result {
                Ok(Ok(ids)) => serde_json::json!({
                    "file": name,
                    "count": ids.len(),
                    "firstId": ids.first().map(|id| id.0),
                }),
                Ok(Err(e)) => serde_json::json!({ "file": name, "error": format!("{e:#}") }),
                Err(e) => serde_json::json!({ "file": name, "error": e.to_string() }),
            };
            tracing::info!(%payload, "opened capture file");
            focus_main(&app);
            let _ = app.emit_to("main", "akev://imported", payload);
        }
    });
}

/// The tray icon: show the window, pause or resume recording, quit. Best effort: desktops
/// without a tray (or without libayatana-appindicator on Linux) simply don't get one.
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    #[cfg(target_os = "linux")]
    quiet_appindicator_deprecation();
    let show = item(app, "tray-show", "Show Akev", None)?;
    let recording = item(app, "tray-recording", "Pause recording", None)?;
    let quit = item(app, "tray-quit", "Quit Akev", None)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &recording, &separator, &quit])?;
    app.manage(TrayRecordingItem(recording));
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Akev")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                focus_main(tray.app_handle());
            }
            sync_tray(tray.app_handle());
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    sync_tray(app);
    Ok(())
}

/// libayatana-appindicator 0.6, which Tauri's tray uses on Linux, prints "libayatana-appindicator
/// is deprecated…" the first time a tray icon is made. That's addressed to the tray library's
/// authors and means nothing to someone running Akev, so drop that one message; every other
/// message from the library still goes to GLib's default handler.
#[cfg(target_os = "linux")]
fn quiet_appindicator_deprecation() {
    use gtk::glib;
    glib::log_set_handler(
        Some("libayatana-appindicator"),
        glib::LogLevels::LEVEL_WARNING,
        false,
        false,
        |domain, level, message| {
            if !message.starts_with("libayatana-appindicator is deprecated") {
                glib::log_default_handler(domain, level, Some(message));
            }
        },
    );
}

/// The tray's recording item, relabelled to match the current state.
struct TrayRecordingItem(tauri::menu::MenuItem<Wry>);

fn sync_tray(app: &AppHandle) {
    if let (Some(item), Some(state)) = (
        app.try_state::<TrayRecordingItem>(),
        app.try_state::<DesktopState>(),
    ) {
        let label = if state.akev.is_recording() {
            "Pause recording"
        } else {
            "Resume recording"
        };
        let _ = item.0.set_text(label);
    }
}

fn focus_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn set_zoom(app: &AppHandle, f: impl FnOnce(f64) -> f64) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let zoom = app.state::<Zoom>();
    let Ok(mut current) = zoom.0.lock() else {
        return;
    };
    let next = f(*current).clamp(0.5, 2.5);
    if window.set_zoom(next).is_ok() {
        *current = next;
    }
}

fn item(
    app: &AppHandle,
    id: &str,
    label: &str,
    accel: Option<&str>,
) -> tauri::Result<tauri::menu::MenuItem<Wry>> {
    let mut b = MenuItemBuilder::with_id(id, label);
    if let Some(a) = accel {
        b = b.accelerator(a);
    }
    b.build(app)
}

/// Native menu bar. Only installed on macOS, where apps are expected to have one; on Linux and
/// Windows the UI's command palette and shortcuts replace it (a GTK/Win32 menubar would clash
/// with the UI's design).
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let about = PredefinedMenuItem::about(app, Some("About Akev"), None)?;
    let app_menu = SubmenuBuilder::new(app, "Akev")
        .item(&about)
        .separator()
        .item(&item(app, "settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item(
            app,
            "compose-new",
            "New Request",
            Some("CmdOrCtrl+T"),
        )?)
        .separator()
        .item(&item(
            app,
            "import",
            "Import HAR or Session…",
            Some("CmdOrCtrl+O"),
        )?)
        .item(&item(
            app,
            "export-har",
            "Export Traffic as HAR…",
            Some("CmdOrCtrl+Shift+E"),
        )?)
        .item(&item(
            app,
            "save-session",
            "Save Session…",
            Some("CmdOrCtrl+S"),
        )?)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item(app, "nav-traffic", "Traffic", Some("CmdOrCtrl+1"))?)
        .item(&item(app, "nav-rules", "Rules", Some("CmdOrCtrl+2"))?)
        .item(&item(app, "nav-compose", "Compose", Some("CmdOrCtrl+3"))?)
        .item(&item(app, "nav-connect", "Connect", Some("CmdOrCtrl+4"))?)
        .separator()
        .item(&item(
            app,
            "command-palette",
            "Command Palette…",
            Some("CmdOrCtrl+K"),
        )?)
        .item(&item(
            app,
            "toggle-recording",
            "Pause / Resume Recording",
            Some("CmdOrCtrl+Shift+R"),
        )?)
        .item(&item(
            app,
            "clear-traffic",
            "Clear Traffic",
            Some("CmdOrCtrl+Shift+X"),
        )?)
        .separator()
        .item(&item(app, "zoom-in", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&item(app, "zoom-out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&item(
            app,
            "zoom-reset",
            "Actual Size",
            Some("CmdOrCtrl+0"),
        )?)
        .separator()
        .item(&item(app, "reload", "Reload UI", None)?)
        .fullscreen()
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;
    let help = SubmenuBuilder::new(app, "Help")
        .item(&item(app, "shortcuts", "Keyboard Shortcuts", None)?)
        .item(&item(app, "docs", "Documentation", None)?)
        .item(&item(app, "report-issue", "Report an Issue…", None)?)
        .build()?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window, &help])
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "tray-show" => focus_main(app),
        "tray-quit" => app.exit(0),
        "tray-recording" => {
            if let Some(state) = app.try_state::<DesktopState>() {
                state.akev.set_recording(!state.akev.is_recording());
            }
            sync_tray(app);
        }
        "zoom-in" => set_zoom(app, |z| z + 0.1),
        "zoom-out" => set_zoom(app, |z| z - 0.1),
        "zoom-reset" => set_zoom(app, |_| 1.0),
        "reload" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.eval("window.location.reload()");
            }
        }
        "docs" => open_url(app, "https://akev.dev/docs"),
        "report-issue" => open_url(app, concat!(env!("CARGO_PKG_REPOSITORY"), "/issues/new")),
        other => {
            focus_main(app);
            let _ = app.emit_to("main", "akev://menu", other.to_string());
        }
    }
}

fn open_url(app: &AppHandle, url: &str) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_url(url, None::<&str>);
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// What the UI served by the daemon may call. Granted at run time to that exact origin only
/// (the port is chosen at startup), never to other local servers the window might be pointed at.
const UI_PERMISSIONS: &[&str] = &[
    "core:default",
    "core:window:allow-minimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-is-maximized",
    "core:window:allow-close",
    "core:window:allow-start-dragging",
    "core:window:allow-set-decorations",
    // The UI mirrors `document.title` (the current page) into the window title.
    "core:window:allow-set-title",
    "core:event:default",
    "dialog:default",
    "dialog:allow-open",
    "notification:default",
    "opener:default",
    "allow-save-file",
    "window-state:default",
    "allow-install-update",
];

/// `scheme://host:port` of a URL.
fn origin_of(url: &tauri::Url) -> String {
    url.origin().ascii_serialization()
}

/// The page script that hands the UI its connection details, only on the daemon's own origin.
fn init_script(origin: &str, details: &str) -> String {
    format!(
        "if (window.location.origin === {}) {{ window.__AKEV__ = Object.freeze({details}); }}",
        js_string(origin)
    )
}

/// Entry point used by `main.rs` (and by mobile targets, should we ever add them).
pub fn run() {
    init_tracing();

    let app = tauri::Builder::default()
        // A second launch (e.g. double-clicking a .har file) focuses this one and opens it.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            focus_main(app);
            open_files(app, capture_files(argv));
        }))
        .manage(PendingFiles(std::sync::Mutex::new(Vec::new())))
        // Decorations follow the title bar setting, not the last session's window.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::DECORATIONS,
                )
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Zoom(std::sync::Mutex::new(1.0)))
        .invoke_handler(tauri::generate_handler![save_file, install_update])
        .setup(|app| {
            let options = akev_api::AkevOptions {
                api_port: 0,
                trial_file: akev_api::default_trial_file(),
                desktop: true,
                ..Default::default()
            };
            let handle = match tauri::async_runtime::block_on(akev_api::start(options)) {
                Ok(handle) => Arc::new(handle),
                Err(e) => {
                    // No window yet: the dialog keeps the app running until it's closed.
                    report_startup_failure(app.handle(), &e);
                    return Ok(());
                }
            };

            let chrome = window_chrome(&handle.settings());
            let url: tauri::Url = handle.ui_url().parse()?;
            let origin = origin_of(&url);
            let details = format!(
                "{{ desktop: true, apiUrl: {}, token: {}, platform: {}, version: {}, windowChrome: Object.freeze({}) }}",
                js_string(&handle.api_url()),
                js_string(handle.token()),
                js_string(std::env::consts::OS),
                js_string(akev_core::VERSION),
                serde_json::to_string(&chrome).unwrap_or_else(|_| "{}".into()),
            );
            let init_script = init_script(&origin, &details);

            let mut capability = tauri::ipc::CapabilityBuilder::new("daemon-ui")
                .window("main")
                .remote(origin.clone())
                .remote(format!("{origin}/*"));
            for permission in UI_PERMISSIONS {
                capability = capability.permission(*permission);
            }
            app.add_capability(capability)?;

            if cfg!(target_os = "macos") {
                let menu = build_menu(app.handle())?;
                app.set_menu(menu)?;
            }

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Akev")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 600.0)
                .decorations(!chrome.custom)
                .initialization_script(&init_script)
                // The window only ever shows the daemon's UI; anything else opens in the browser.
                .on_navigation({
                    let app = app.handle().clone();
                    let origin = origin.clone();
                    move |target| {
                        if origin_of(target) == origin {
                            return true;
                        }
                        if matches!(target.scheme(), "http" | "https" | "mailto") {
                            open_url(&app, target.as_str());
                        }
                        false
                    }
                })
                // Let the web UI receive HTML5 drag & drop (drop a HAR file to import it).
                .disable_drag_drop_handler()
                .build()?;

            tracing::info!(
                api = %handle.api_url(),
                proxy = %handle.proxy_addr(),
                "Akev desktop started"
            );
            app.manage(DesktopState { akev: handle });
            if let Err(e) = build_tray(app.handle()) {
                tracing::info!("no tray icon: {e}");
            }
            // Files given on the command line, or opened before setup finished.
            let mut files = capture_files(std::env::args());
            if let Some(pending) = app.try_state::<PendingFiles>()
                && let Ok(mut list) = pending.0.lock()
            {
                files.append(&mut list);
            }
            open_files(app.handle(), files);
            Ok(())
        })
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("Akev failed to start: {err}");
            eprintln!("Akev failed to start: {err}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| match event {
        RunEvent::ExitRequested { .. } => {
            if let Some(state) = app.try_state::<DesktopState>() {
                let handle = state.akev.clone();
                // Restore system proxy settings, stop launched browsers, flush settings.
                tauri::async_runtime::block_on(async move { handle.shutdown().await });
            }
        }
        // macOS: files opened with Akev from Finder (file associations).
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            let files = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
            open_files(app, files);
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::buttons_on_the_right;

    #[test]
    fn suggested_file_names() {
        use super::suggested_file_name;
        assert_eq!(suggested_file_name("traffic.har"), "traffic.har");
        assert_eq!(
            suggested_file_name("r%C3%A9sum%C3%A9%20v2.json"),
            "résumé v2.json"
        );
        assert_eq!(suggested_file_name("..%2F..%2Fetc%2Fpasswd"), "passwd");
        assert_eq!(suggested_file_name("C:%5CUsers%5Cx%5Cevil.exe"), "evil.exe");
        assert_eq!(suggested_file_name(".bashrc"), "bashrc");
        assert_eq!(suggested_file_name("%2F"), "download");
        assert_eq!(suggested_file_name(""), "download");
    }

    #[test]
    fn the_init_script_only_runs_on_the_daemon_origin() {
        let url: tauri::Url = "http://127.0.0.1:45456/?x=1".parse().unwrap();
        let origin = super::origin_of(&url);
        assert_eq!(origin, "http://127.0.0.1:45456");
        let script = super::init_script(&origin, "{ token: \"t\" }");
        assert!(script.starts_with("if (window.location.origin === \"http://127.0.0.1:45456\")"));
        assert!(script.contains("window.__AKEV__ = Object.freeze({ token: \"t\" })"));
    }

    #[test]
    fn capture_files_from_arguments() {
        let files = super::capture_files([
            "akev-desktop",
            "--flag",
            "/tmp/traffic.HAR",
            "C:\\Users\\x\\s.saz",
            "notes.txt",
            "session.akev",
        ]);
        let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(
            names,
            ["/tmp/traffic.HAR", "C:\\Users\\x\\s.saz", "session.akev"]
        );
        assert!(super::capture_files(["only-the-program"]).is_empty());
    }

    #[test]
    fn startup_error_messages() {
        let err = anyhow::anyhow!(
            "another Akev instance is already using the data directory /d: the akev command (pid 7)"
        );
        let msg =
            super::startup_error_message(&err, Some(std::path::Path::new("/d/logs/akev.log")));
        assert_eq!(
            msg,
            "Another Akev instance is already using the data directory /d: the akev command (pid 7).\n\nThe log file has more details: /d/logs/akev.log"
        );
        let err = anyhow::anyhow!("bind failed").context("failed to start the proxy");
        assert_eq!(
            super::startup_error_message(&err, None),
            "Failed to start the proxy: bind failed."
        );
    }

    #[test]
    fn decoration_layouts() {
        assert!(buttons_on_the_right("appmenu:minimize,maximize,close"));
        assert!(buttons_on_the_right(":close"));
        assert!(buttons_on_the_right("icon,menu:"));
        assert!(!buttons_on_the_right("close,minimize,maximize:"));
        assert!(!buttons_on_the_right("close:appmenu"));
        assert!(!buttons_on_the_right("minimize,maximize,close"));
        assert!(buttons_on_the_right(""));
    }
}
