//! Which files AI agents (MCP) may make Akev read or write.
//!
//! `akev_export_har` writes files and rules read them (`file` body sources, `map-local`), so a
//! prompt-injected agent could otherwise exfiltrate `~/.ssh` through a mock rule and
//! `akev_send_request`, or overwrite any file. MCP-originated paths are therefore confined to:
//!
//! * the exports directory, `<data dir>/exports` (created on demand), where relative export
//!   paths land;
//! * folders the user allowed with `--mcp-allow-path <dir>` (`AkevOptions::mcp_allow_paths`).
//!
//! Paths are resolved like the operation itself would (`~` expanded, relative rule paths
//! against the working directory) and canonicalised: `..` and symbolic links can't leave a
//! root. Rules created in the UI or through the REST API are not restricted — the user is in
//! control there — and rules an agent merely passes back unchanged keep working.

use akev_core::rules::{
    Action, BodySource, BodyTransform, RequestTransform, ResponseTransform, Rule, RuleSet,
};
use std::path::{Component, Path, PathBuf};

/// The folders MCP may use.
#[derive(Debug, Clone)]
pub struct McpFiles {
    exports: PathBuf,
    /// `--mcp-allow-path` folders, canonical (missing ones are dropped with a warning).
    allowed: Vec<PathBuf>,
}

impl McpFiles {
    /// `data_dir/exports` plus `allowed` (which must exist).
    pub fn new(data_dir: &Path, allowed: &[PathBuf]) -> Self {
        let allowed = allowed
            .iter()
            .filter_map(|dir| match expand_home(dir).canonicalize() {
                Ok(c) if c.is_dir() => Some(c),
                Ok(c) => {
                    tracing::warn!(
                        "--mcp-allow-path {}: not a folder; AI agents can't use it",
                        c.display()
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        "--mcp-allow-path {}: {e}; AI agents can't use it",
                        dir.display()
                    );
                    None
                }
            })
            .collect();
        McpFiles {
            exports: data_dir.join("exports"),
            allowed,
        }
    }

    /// The exports directory (not created).
    pub fn exports_dir(&self) -> &Path {
        &self.exports
    }

    /// The `--mcp-allow-path` folders in use, for display.
    pub fn allowed_dirs(&self) -> Vec<String> {
        self.allowed
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }

    /// Canonical roots (the exports directory once it exists, then the allowed folders).
    fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.exports.canonicalize().into_iter().collect();
        roots.extend(self.allowed.iter().cloned());
        roots
    }

    /// The roots for messages: `"/data/exports" or "/home/me/fixtures"`.
    fn describe_roots(&self) -> String {
        let mut names = vec![self.exports.display().to_string()];
        names.extend(self.allowed.iter().map(|p| p.display().to_string()));
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(" or ")
    }

    fn inside_roots(&self, canonical: &Path) -> bool {
        self.roots().iter().any(|root| canonical.starts_with(root))
    }

    fn outside_message(&self, what: &str, raw: &str) -> String {
        format!(
            "{what} {raw:?} is outside the folders AI agents may use ({}). Use a path inside one of them, or ask the user to start Akev with --mcp-allow-path <folder>.",
            self.describe_roots()
        )
    }

    /// Where an agent's export to `raw` goes: relative paths land in the exports directory
    /// (created on demand); absolute ones must be inside a root.
    pub fn export_path(&self, raw: &str) -> Result<PathBuf, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("`path` must not be empty".into());
        }
        let expanded = expand_home(Path::new(raw));
        let path = if expanded.is_absolute() {
            expanded
        } else {
            std::fs::create_dir_all(&self.exports).map_err(|e| {
                format!(
                    "could not create the exports folder {}: {e}",
                    self.exports.display()
                )
            })?;
            self.exports.join(expanded)
        };
        if path.file_name().is_none() || raw.ends_with('/') || raw.ends_with('\\') {
            return Err(format!("`path` {raw:?} must name a file"));
        }
        let canonical =
            canonicalize_lenient(&path).map_err(|e| format!("can't use `path` {raw:?}: {e}"))?;
        if canonical.is_dir() {
            return Err(format!("`path` {raw:?} is a folder; name a file inside it"));
        }
        if !self.inside_roots(&canonical) {
            return Err(self.outside_message("`path`", raw));
        }
        Ok(canonical)
    }

    /// Whether a rule may read `raw` (resolved like the rules engine does: `~` expanded,
    /// relative to the working directory).
    fn readable(&self, raw: &str) -> bool {
        let expanded = expand_home(Path::new(raw.trim()));
        let path = if expanded.is_absolute() {
            expanded
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(expanded),
                Err(_) => return false,
            }
        };
        canonicalize_lenient(&path).is_ok_and(|c| self.inside_roots(&c))
    }

    /// Check the files the rules in `new` read, for a rule set coming from an agent. A rule
    /// that is unchanged from `current` (same id, matchers and action — e.g. one the user
    /// made in the UI, passed back by `akev_set_rules`) is accepted as it is.
    pub fn check_rules(&self, new: &RuleSet, current: &RuleSet) -> Result<(), String> {
        let existing = current.walk();
        for (rule, _) in new.walk() {
            let unchanged = existing.iter().any(|(old, _)| {
                old.id == rule.id && old.matchers == rule.matchers && old.action == rule.action
            });
            if unchanged {
                continue;
            }
            for path in rule_files(rule) {
                if !self.readable(path) {
                    let name = rule
                        .title
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or(&rule.id);
                    return Err(format!(
                        "Rule {name:?} was not applied: {}",
                        self.outside_message("it reads", path)
                    ));
                }
            }
        }
        Ok(())
    }
}

/// `~` and `~/…` → the home directory (as the rules engine expands them).
fn expand_home(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    let rest = if s == "~" {
        Some("")
    } else {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    };
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Canonicalise a path that may not exist yet: the deepest existing ancestor is
/// canonicalised (resolving `..` and symbolic links), and the missing rest is appended after
/// lexical normalisation (nothing that doesn't exist can be a link).
fn canonicalize_lenient(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(base) => {
                let mut out = base;
                for part in rest.iter().rev() {
                    let p = Path::new(part);
                    match p.components().next() {
                        Some(Component::ParentDir) => {
                            out.pop();
                        }
                        Some(Component::CurDir) | None => {}
                        _ => out.push(p),
                    }
                }
                return Ok(out);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                    // `..` at the end of a missing path, or a root that doesn't exist.
                    match existing.components().next_back() {
                        Some(Component::ParentDir) => {
                            rest.push("..".into());
                            if !existing.pop() {
                                return Err(e);
                            }
                            continue;
                        }
                        _ => return Err(e),
                    }
                };
                rest.push(name);
                if !existing.pop() {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Every file path a rule reads: `file` body sources (mocks, WebSocket rejections, body
/// replacements in transforms) and the `map-local` root.
fn rule_files(rule: &Rule) -> Vec<&str> {
    fn source<'a>(src: &'a BodySource, out: &mut Vec<&'a str>) {
        if let BodySource::File { path } = src {
            out.push(path);
        }
    }
    fn transform<'a>(body: Option<&'a BodyTransform>, out: &mut Vec<&'a str>) {
        if let Some(BodyTransform::Replace { body }) = body {
            source(body, out);
        }
    }
    fn transforms<'a>(
        request: Option<&'a RequestTransform>,
        response: Option<&'a ResponseTransform>,
        out: &mut Vec<&'a str>,
    ) {
        transform(request.and_then(|r| r.body.as_ref()), out);
        transform(response.and_then(|r| r.body.as_ref()), out);
    }
    let mut out = Vec::new();
    match &rule.action {
        Action::Mock { body, .. } | Action::WsReject { body, .. } => source(body, &mut out),
        Action::MapLocal { path, .. } => out.push(path),
        Action::Passthrough { request, response }
        | Action::Forward {
            request, response, ..
        }
        | Action::Transform { request, response } => {
            transforms(request.as_ref(), response.as_ref(), &mut out)
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::rules::{RuleItem, StringMatch};

    fn rule(id: &str, action: Action) -> RuleItem {
        RuleItem::Rule(Rule {
            id: id.into(),
            title: Some(format!("rule {id}")),
            enabled: true,
            matchers: vec![akev_core::rules::Matcher::Url {
                pattern: StringMatch {
                    op: akev_core::rules::MatchOp::Contains,
                    value: "example".into(),
                    case_sensitive: false,
                    negate: false,
                },
            }],
            action,
            max_matches: None,
            note: None,
        })
    }

    fn mock_file(path: &str) -> Action {
        Action::Mock {
            status: 200,
            status_message: None,
            headers: Default::default(),
            body: BodySource::File { path: path.into() },
        }
    }

    fn set(items: Vec<RuleItem>) -> RuleSet {
        RuleSet { version: 1, items }
    }

    #[test]
    fn exports_land_in_the_exports_folder() {
        let data = tempfile::tempdir().unwrap();
        let files = McpFiles::new(data.path(), &[]);
        let p = files.export_path("traffic.har").unwrap();
        assert_eq!(
            p,
            data.path()
                .canonicalize()
                .unwrap()
                .join("exports/traffic.har")
        );
        assert!(data.path().join("exports").is_dir(), "created on demand");
        // `..` can't climb out, absolute paths elsewhere are refused.
        let err = files.export_path("../settings.json").unwrap_err();
        assert!(err.contains("outside the folders"), "{err}");
        assert!(err.contains("--mcp-allow-path"), "{err}");
        assert!(files.export_path("/etc/passwd").is_err());
        assert!(files.export_path("~/.ssh/authorized_keys").is_err());
        assert!(files.export_path("sub/../../x.har").is_err());
        // Inside is fine, also spelled with `..` that stays inside.
        assert!(files.export_path("a/../b.har").is_ok());
        assert!(files.export_path("").is_err());
        assert!(files.export_path("dir/").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cant_escape() {
        let data = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let files = McpFiles::new(data.path(), &[]);
        std::fs::create_dir_all(data.path().join("exports")).unwrap();
        std::os::unix::fs::symlink(outside.path(), data.path().join("exports/out")).unwrap();
        let err = files.export_path("out/x.har").unwrap_err();
        assert!(err.contains("outside"), "{err}");
        std::os::unix::fs::symlink(
            outside.path().join("f.txt"),
            data.path().join("exports/link.har"),
        )
        .unwrap();
        std::fs::write(outside.path().join("f.txt"), b"x").unwrap();
        assert!(files.export_path("link.har").is_err());
    }

    #[test]
    fn allowed_folders_extend_the_roots() {
        let data = tempfile::tempdir().unwrap();
        let fixtures = tempfile::tempdir().unwrap();
        let files = McpFiles::new(
            data.path(),
            &[fixtures.path().to_path_buf(), data.path().join("missing")],
        );
        let target = fixtures.path().join("out.har");
        assert_eq!(
            files.export_path(target.to_str().unwrap()).unwrap(),
            fixtures.path().canonicalize().unwrap().join("out.har")
        );
        let body = fixtures.path().join("body.json");
        std::fs::write(&body, b"{}").unwrap();
        let ok = set(vec![rule("a", mock_file(body.to_str().unwrap()))]);
        files.check_rules(&ok, &RuleSet::default()).unwrap();
    }

    #[test]
    fn agent_rules_may_only_read_inside_the_roots() {
        let data = tempfile::tempdir().unwrap();
        let files = McpFiles::new(data.path(), &[]);
        let passwd = set(vec![rule("x", mock_file("/etc/passwd"))]);
        let err = files.check_rules(&passwd, &RuleSet::default()).unwrap_err();
        assert!(
            err.contains("rule x") && err.contains("/etc/passwd"),
            "{err}"
        );
        for action in [
            Action::MapLocal {
                path: "~/.ssh".into(),
                strip_prefix: None,
                index: None,
                fallback_to_upstream: false,
                headers: Default::default(),
            },
            Action::Transform {
                request: None,
                response: Some(ResponseTransform {
                    body: Some(BodyTransform::Replace {
                        body: BodySource::File {
                            path: "../../../../etc/hosts".into(),
                        },
                    }),
                    ..Default::default()
                }),
            },
            Action::WsReject {
                status: 400,
                body: BodySource::File {
                    path: "/etc/shadow".into(),
                },
            },
        ] {
            let rules = set(vec![rule("y", action)]);
            assert!(files.check_rules(&rules, &RuleSet::default()).is_err());
        }
        // Inside the exports folder is fine.
        std::fs::create_dir_all(data.path().join("exports")).unwrap();
        let inside = data.path().join("exports/mock.json");
        let ok = set(vec![rule("z", mock_file(inside.to_str().unwrap()))]);
        files.check_rules(&ok, &RuleSet::default()).unwrap();
        // Rules without files are always fine.
        let plain = set(vec![rule("p", Action::Close)]);
        files.check_rules(&plain, &RuleSet::default()).unwrap();
    }

    #[test]
    fn unchanged_user_rules_are_passed_back() {
        let data = tempfile::tempdir().unwrap();
        let files = McpFiles::new(data.path(), &[]);
        // The user made this rule in the UI.
        let current = set(vec![rule("user", mock_file("/srv/fixtures/a.json"))]);
        // The agent adds a rule and sends the user's rule back unchanged (maybe disabled).
        let mut back = current.clone();
        if let RuleItem::Rule(r) = &mut back.items[0] {
            r.enabled = false;
            r.title = Some("renamed".into());
        }
        back.items.push(rule("new", Action::Close));
        files.check_rules(&back, &current).unwrap();
        // But it can't point the user's rule (or a copy under another id) elsewhere.
        let mut moved = current.clone();
        if let RuleItem::Rule(r) = &mut moved.items[0] {
            r.action = mock_file("/etc/passwd");
        }
        assert!(files.check_rules(&moved, &current).is_err());
        let copy = set(vec![rule("copy", mock_file("/srv/fixtures/a.json"))]);
        assert!(files.check_rules(&copy, &current).is_err());
    }

    #[test]
    fn lenient_canonicalisation() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        assert_eq!(
            canonicalize_lenient(&dir.path().join("a/b/../c")).unwrap(),
            base.join("a/c")
        );
        assert_eq!(
            canonicalize_lenient(&dir.path().join("x/..")).unwrap(),
            base
        );
        std::fs::create_dir(dir.path().join("real")).unwrap();
        assert_eq!(
            canonicalize_lenient(&dir.path().join("real/../real/f")).unwrap(),
            base.join("real/f")
        );
    }
}
