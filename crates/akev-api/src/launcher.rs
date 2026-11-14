//! Opening the web UI without putting the API token on a command line.
//!
//! The UI's login URL (`{api_url}/?token=…`) carries the API token. Handing it to a browser
//! launcher (`xdg-open`, `open`, `start`) puts it into the argv of every process in that
//! chain, and on Linux `/proc/<pid>/cmdline` is readable by every local user. Instead — like
//! Jupyter — Akev writes a small redirect page into the (user-only) data directory with mode
//! 0600 and opens *that file*: only its path appears on command lines, and only the user can
//! read the token inside it.
//!
//! The page is overwritten on every launch and removed when the daemon it points at shuts
//! down (if it still points at that daemon). It holds nothing that `daemon.json` (same
//! directory, same mode) doesn't already hold.

use std::path::{Path, PathBuf};

/// File name of the launcher page inside the data directory.
pub const UI_LAUNCHER_FILE: &str = "open-ui.html";

/// The URL that opens the UI already authenticated: `{api_url}/?token=<token>` with the
/// token percent-encoded (user-chosen tokens may contain `&`, `+`, `#`, ...).
pub fn ui_url_for(api_url: &str, token: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("token", token)
        .finish();
    format!("{}/?{query}", api_url.trim_end_matches('/'))
}

/// Path of the launcher page in `data_dir`.
pub fn ui_launcher_path(data_dir: &Path) -> PathBuf {
    data_dir.join(UI_LAUNCHER_FILE)
}

/// Write the launcher page for `ui_url` into `data_dir` (created user-only if missing) and
/// return its absolute path. Open the result with [`ui_launcher_url`] or as a plain path.
pub fn write_ui_launcher(data_dir: &Path, ui_url: &str) -> std::io::Result<PathBuf> {
    crate::persist::ensure_dir(data_dir)?;
    let path = ui_launcher_path(data_dir);
    crate::persist::write_atomic(&path, launcher_html(ui_url).as_bytes(), 0o600)?;
    std::path::absolute(&path)
}

/// `file://` URL of a launcher page (falls back to the plain path when it can't be expressed
/// as a URL). Contains no secret.
pub fn ui_launcher_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|()| path.display().to_string())
}

/// Remove the launcher page if it still opens `ui_url` (another instance sharing the data
/// directory may have replaced it with its own).
pub(crate) fn remove_ui_launcher_if_owned(data_dir: &Path, ui_url: &str) -> std::io::Result<()> {
    let path = ui_launcher_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(content) if content == launcher_html(ui_url) => std::fs::remove_file(&path),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The redirect page. A meta refresh works without JavaScript and from `file://` in every
/// browser; the link is the manual fallback.
pub(crate) fn launcher_html(ui_url: &str) -> String {
    let url = html_escape(ui_url);
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"referrer\" content=\"no-referrer\">\
<meta http-equiv=\"refresh\" content=\"0;url={url}\">\
<title>Opening Akev…</title>\
<style>:root{{color-scheme:light dark}}body{{font:15px/1.5 system-ui,sans-serif;max-width:36rem;margin:15vh auto;padding:0 1.5rem}}</style>\
</head><body><p>Opening Akev… If nothing happens, <a href=\"{url}\">open the Akev UI</a>.</p></body></html>\n"
    )
}

use akev_core::time::html_escape;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_urls_encode_the_token() {
        assert_eq!(
            ui_url_for("http://127.0.0.1:45456", "abc123"),
            "http://127.0.0.1:45456/?token=abc123"
        );
        assert_eq!(
            ui_url_for("http://127.0.0.1:1/", "a+b&c#d<e>"),
            "http://127.0.0.1:1/?token=a%2Bb%26c%23d%3Ce%3E"
        );
        // The server decodes it back to the original token.
        let url = url::Url::parse(&ui_url_for("http://h:1", "a+b&c#d")).unwrap();
        assert_eq!(
            crate::auth::query_token(url.query().unwrap()).as_deref(),
            Some("a+b&c#d")
        );
    }

    #[test]
    fn launcher_page_redirects_and_escapes() {
        let html = launcher_html("http://127.0.0.1:1/?token=x&y=\"<z>\"");
        assert!(html.contains("http-equiv=\"refresh\""));
        assert!(html.contains("0;url=http://127.0.0.1:1/?token=x&amp;y=&quot;&lt;z&gt;&quot;"));
        assert!(!html.contains("<z>"));
        assert!(html.contains("no-referrer"));
    }

    #[test]
    fn launcher_file_is_private_and_holds_the_token_not_its_url() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("nested");
        let ui = ui_url_for("http://127.0.0.1:45456", "s3cr3t-token");
        let path = write_ui_launcher(&data, &ui).unwrap();
        assert!(path.is_absolute());
        assert_eq!(path, std::path::absolute(ui_launcher_path(&data)).unwrap());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("s3cr3t-token"));
        // What gets handed to the browser launcher (argv) carries no secret.
        let target = ui_launcher_url(&path);
        assert!(target.starts_with("file://"), "{target}");
        assert!(!target.contains("s3cr3t-token"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let dir_mode = std::fs::metadata(&data).unwrap().permissions().mode();
            assert_eq!(dir_mode & 0o777, 0o700);
        }
        // Rewriting replaces it; removal only happens for the owner.
        let other = ui_url_for("http://127.0.0.1:45457", "other");
        remove_ui_launcher_if_owned(&data, &other).unwrap();
        assert!(path.exists());
        remove_ui_launcher_if_owned(&data, &ui).unwrap();
        assert!(!path.exists());
        remove_ui_launcher_if_owned(&data, &ui).unwrap();
    }
}
