//! The protobuf schema registry: `.proto` files and descriptor sets the user imported
//! (`/api/protos`), compiled into one descriptor pool, plus the URL → message type mappings
//! (`/api/protos/mappings`). Body views ask it which message type to decode a body as
//! ([`ProtoRegistry::plan`]).
//!
//! Persistence (unless the session is ephemeral): `data_dir/protos/files/<name>` holds every
//! imported file under its name (a relative path), `data_dir/protos/mappings.json` the
//! mappings. Changes are written atomically before they're applied.
//!
//! Choosing a message type for a body, first match wins:
//! 1. a `messageType=` / `proto=` / `message=` parameter of its Content-Type;
//! 2. gRPC / gRPC-Web content: the method named by the path (`/package.Service/Method`), its
//!    input type for requests and output type for responses;
//! 3. the first enabled mapping whose pattern matches the URL and side.

pub mod compile;
pub mod mapping;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use akev_content::schema::{self, SchemaPlan, SchemaSource};
use akev_core::Side;
use arc_swap::ArcSwap;
use bytes::Bytes;
use parking_lot::Mutex;
use serde::Serialize;

pub use compile::{Compiled, ProtoFileInfo, ProtoFileKind};
pub use mapping::ProtoMapping;

use crate::persist;

/// Largest file accepted (descriptor sets of big APIs with all imports run to a few MB).
pub const MAX_FILE_SIZE: usize = 16 << 20;
/// Most files kept.
pub const MAX_FILES: usize = 2000;
/// Most bytes kept in all files together.
pub const MAX_TOTAL_SIZE: usize = 128 << 20;
/// Longest file name (relative path) accepted.
pub const MAX_NAME_LEN: usize = 255;

const FILES_DIR: &str = "files";
const MAPPINGS_FILE: &str = "mappings.json";

/// `GET /api/protos` (see the HTTP API reference, akev.dev/docs/api).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoRegistryInfo {
    pub files: Vec<ProtoFileInfo>,
    pub mappings: Vec<ProtoMapping>,
    /// Every message type the files define (for the mapping editor).
    pub message_types: Vec<String>,
}

/// A file to add.
#[derive(Clone, Debug)]
pub struct NewFile {
    pub name: String,
    pub bytes: Bytes,
}

/// Error of a registry change (the registry is unchanged).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    /// The request is invalid (bad name, too large, unknown file...).
    #[error("{0}")]
    Invalid(String),
    /// No such file.
    #[error("no schema file named {0:?}")]
    NotFound(String),
    /// Writing to the data directory failed.
    #[error("could not save the schema files: {0}")]
    Io(String),
}

struct State {
    files: BTreeMap<String, compile::SourceFile>,
    mappings: Vec<ProtoMapping>,
}

/// The schema registry (see the module docs).
pub struct ProtoRegistry {
    /// `data_dir/protos` (None: in memory only).
    dir: Option<PathBuf>,
    /// Source of truth; held while a change is compiled and saved, so changes apply in order.
    state: Mutex<State>,
    compiled: ArcSwap<Compiled>,
    mappings: ArcSwap<Vec<ProtoMapping>>,
    /// Whether schemas are used for decoding (Akev Pro). When off, [`ProtoRegistry::plan`]
    /// finds nothing; the files and mappings stay stored.
    enabled: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for ProtoRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoRegistry")
            .field("dir", &self.dir)
            .field("files", &self.compiled.load().files.len())
            .finish_non_exhaustive()
    }
}

impl ProtoRegistry {
    /// An empty registry kept in memory only (ephemeral sessions, tests).
    pub fn in_memory() -> Self {
        ProtoRegistry {
            dir: None,
            state: Mutex::new(State {
                files: BTreeMap::new(),
                mappings: Vec::new(),
            }),
            compiled: ArcSwap::from_pointee(Compiled::default()),
            mappings: ArcSwap::from_pointee(Vec::new()),
            enabled: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Use the schemas for decoding or not (see the `enabled` field).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Load the registry persisted in `dir` (`data_dir/protos`) and compile it. Unreadable
    /// files and an invalid `mappings.json` are skipped with a warning.
    pub fn load(dir: PathBuf) -> Self {
        let mut files = BTreeMap::new();
        let root = dir.join(FILES_DIR);
        let mut stack = vec![root.clone()];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(name) = relative_name(&root, &path) else {
                    continue;
                };
                if name.starts_with('.') || name.contains("/.") || validate_name(&name).is_err() {
                    continue; // temp files of an interrupted write, foreign files
                }
                match std::fs::read(&path) {
                    Ok(bytes) if bytes.len() <= MAX_FILE_SIZE => {
                        let added_at = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map_or(0.0, |d| d.as_millis() as f64);
                        files.insert(
                            name.clone(),
                            compile::SourceFile {
                                kind: ProtoFileKind::for_name(&name),
                                bytes: Bytes::from(bytes),
                                added_at,
                            },
                        );
                    }
                    Ok(_) => tracing::warn!("skipping schema file {name}: too large"),
                    Err(e) => tracing::warn!("skipping schema file {name}: {e}"),
                }
            }
        }
        let mappings = match std::fs::read(dir.join(MAPPINGS_FILE)) {
            Ok(bytes) => serde_json::from_slice::<Vec<ProtoMapping>>(&bytes)
                .map_err(|e| e.to_string())
                .and_then(mapping::normalize)
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "ignoring invalid {}: {e}",
                        dir.join(MAPPINGS_FILE).display()
                    );
                    Vec::new()
                }),
            Err(_) => Vec::new(),
        };
        let compiled = compile::compile(&files);
        ProtoRegistry {
            dir: Some(dir),
            compiled: ArcSwap::from_pointee(compiled),
            mappings: ArcSwap::from_pointee(mappings.clone()),
            state: Mutex::new(State { files, mappings }),
            enabled: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// The registry as the API describes it.
    pub fn info(&self) -> ProtoRegistryInfo {
        let compiled = self.compiled.load();
        ProtoRegistryInfo {
            files: compiled.files.clone(),
            mappings: self.mappings.load().as_ref().clone(),
            message_types: compiled.message_types.clone(),
        }
    }

    /// The mappings.
    pub fn mappings(&self) -> Vec<ProtoMapping> {
        self.mappings.load().as_ref().clone()
    }

    /// Add (or replace, by name) files, then recompile. Files that don't compile are kept
    /// (with their errors) so they can be fixed by uploading them again. CPU and disk work:
    /// call from a blocking task.
    pub fn add(&self, new: Vec<NewFile>) -> Result<ProtoRegistryInfo, RegistryError> {
        if new.is_empty() {
            return Err(RegistryError::Invalid("no files given".into()));
        }
        let mut prepared = Vec::with_capacity(new.len());
        for f in new {
            let name = normalize_name(&f.name).map_err(RegistryError::Invalid)?;
            if f.bytes.len() > MAX_FILE_SIZE {
                return Err(RegistryError::Invalid(format!(
                    "{name} is larger than {} MB",
                    MAX_FILE_SIZE >> 20
                )));
            }
            if f.bytes.is_empty() && ProtoFileKind::for_name(&name) == ProtoFileKind::DescriptorSet
            {
                return Err(RegistryError::Invalid(format!("{name} is empty")));
            }
            prepared.push((name, f.bytes));
        }
        let mut state = self.state.lock();
        let mut files = state.files.clone();
        let now = now_ms();
        for (name, bytes) in &prepared {
            files.insert(
                name.clone(),
                compile::SourceFile {
                    kind: ProtoFileKind::for_name(name),
                    bytes: bytes.clone(),
                    added_at: now,
                },
            );
        }
        if files.len() > MAX_FILES {
            return Err(RegistryError::Invalid(format!(
                "at most {MAX_FILES} schema files are supported"
            )));
        }
        let total: usize = files.values().map(|f| f.bytes.len()).sum();
        if total > MAX_TOTAL_SIZE {
            return Err(RegistryError::Invalid(format!(
                "schema files may hold at most {} MB together",
                MAX_TOTAL_SIZE >> 20
            )));
        }
        // A name can't be both a file and a folder.
        for (name, _) in &prepared {
            let as_dir = format!("{name}/");
            if files.keys().any(|k| k.starts_with(&as_dir))
                || ancestors(name).any(|a| files.contains_key(a))
            {
                return Err(RegistryError::Invalid(format!(
                    "{name} clashes with another file's folder"
                )));
            }
        }
        if let Some(dir) = &self.dir {
            for (name, bytes) in &prepared {
                write_file(&dir.join(FILES_DIR), name, bytes)
                    .map_err(|e| RegistryError::Io(e.to_string()))?;
            }
        }
        state.files = files;
        self.recompile(&state);
        Ok(self.info())
    }

    /// Remove one file (`Some(name)`) or all of them (`None`), then recompile.
    pub fn remove(&self, name: Option<&str>) -> Result<ProtoRegistryInfo, RegistryError> {
        let mut state = self.state.lock();
        let names: Vec<String> = match name {
            Some(n) => {
                let n = normalize_name(n).map_err(|_| RegistryError::NotFound(n.to_string()))?;
                if !state.files.contains_key(&n) {
                    return Err(RegistryError::NotFound(n));
                }
                vec![n]
            }
            None => state.files.keys().cloned().collect(),
        };
        if let Some(dir) = &self.dir {
            let root = dir.join(FILES_DIR);
            for n in &names {
                match std::fs::remove_file(root.join(n)) {
                    Ok(()) => remove_empty_parents(&root, n),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(RegistryError::Io(e.to_string())),
                }
            }
        }
        for n in &names {
            state.files.remove(n);
        }
        self.recompile(&state);
        Ok(self.info())
    }

    /// The bytes of a stored file.
    pub fn file(&self, name: &str) -> Option<(ProtoFileKind, Bytes)> {
        let name = normalize_name(name).ok()?;
        let state = self.state.lock();
        state.files.get(&name).map(|f| (f.kind, f.bytes.clone()))
    }

    /// Replace the mappings (validated and normalized, see [`mapping::normalize`]).
    pub fn set_mappings(
        &self,
        mappings: Vec<ProtoMapping>,
    ) -> Result<Vec<ProtoMapping>, RegistryError> {
        let mappings = mapping::normalize(mappings).map_err(RegistryError::Invalid)?;
        let mut state = self.state.lock();
        if let Some(dir) = &self.dir {
            persist::ensure_dir(dir).map_err(|e| RegistryError::Io(e.to_string()))?;
            persist::write_json_atomic(&dir.join(MAPPINGS_FILE), &mappings, 0o644)
                .map_err(|e| RegistryError::Io(e.to_string()))?;
        }
        state.mappings = mappings.clone();
        self.mappings.store(Arc::new(mappings.clone()));
        Ok(mappings)
    }

    fn recompile(&self, state: &State) {
        self.compiled
            .store(Arc::new(compile::compile(&state.files)));
    }

    /// The message type to decode the `side` body of a request to `url` (`path` = its
    /// path) with, given the body's Content-Type. Cheap when the registry is empty.
    pub fn plan(
        &self,
        side: Side,
        url: &str,
        path: &str,
        content_type: Option<&str>,
    ) -> Option<SchemaPlan> {
        if !self.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let compiled = self.compiled.load();
        if compiled.files.is_empty() {
            return None;
        }
        let pool = &compiled.pool;
        if let Some(plan) = content_type.and_then(|ct| schema::plan_for_content_type(pool, ct)) {
            return Some(plan);
        }
        let grpc = content_type.is_some_and(|ct| {
            ct.trim_start()
                .get(..16)
                .is_some_and(|p| p.eq_ignore_ascii_case("application/grpc"))
        });
        if grpc && let Some(plan) = schema::plan_for_grpc_path(pool, path, side) {
            return Some(plan);
        }
        let mappings = self.mappings.load();
        if mappings.is_empty() {
            return None;
        }
        let target = mapping::Target::new(url);
        mappings.iter().find_map(|m| {
            if !m.matches(&target, side) {
                return None;
            }
            let message = pool.get_message_by_name(&m.message_type)?;
            let mut plan = SchemaPlan::new(message, SchemaSource::Mapping);
            plan.mapping = Some(m.pattern.clone());
            Some(plan)
        })
    }
}

use akev_core::time::now_ms;

/// Validate a file name: a relative path of `/`-separated segments made of letters, digits,
/// `.`, `_`, `-`, `+` and `@`, no empty, `.` or `..` segments, no hidden files.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the file name is empty".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "the file name is longer than {MAX_NAME_LEN} characters"
        ));
    }
    for segment in name.split('/') {
        if segment.is_empty() || segment.starts_with('.') {
            return Err(format!(
                "invalid file name {name:?}: use a relative path like `shop/v1/catalog.proto`"
            ));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '@'))
        {
            return Err(format!(
                "invalid file name {name:?}: only letters, digits, `.`, `_`, `-`, `+`, `@` and `/` are allowed"
            ));
        }
    }
    Ok(())
}

/// Trim, turn `\` into `/` and drop a leading `./` or `/`, then validate.
pub fn normalize_name(name: &str) -> Result<String, String> {
    let mut n = name.trim().replace('\\', "/");
    while let Some(rest) = n.strip_prefix("./").or_else(|| n.strip_prefix('/')) {
        n = rest.to_string();
    }
    validate_name(&n)?;
    Ok(n)
}

fn ancestors(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('/').map(move |(i, _)| &name[..i])
}

fn relative_name(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

fn write_file(root: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        persist::ensure_dir(parent)?;
    }
    persist::write_atomic(&path, bytes, 0o644)
}

/// Remove the folders left empty by deleting `name` (up to, not including, `root`).
fn remove_empty_parents(root: &Path, name: &str) {
    let mut dir = root.join(name);
    while dir.pop() && dir.starts_with(root) && dir != root {
        if std::fs::remove_dir(&dir).is_err() {
            break; // not empty
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mapping::MappingSide;
    use super::*;

    const CATALOG: &str = "syntax = \"proto3\";\npackage shop.v1;\nmessage Item { string id = 1; string name = 2; }\nmessage GetItemRequest { string id = 1; }\nservice Catalog { rpc GetItem(GetItemRequest) returns (Item); }\n";

    fn new(name: &str, text: &str) -> NewFile {
        NewFile {
            name: name.into(),
            bytes: Bytes::copy_from_slice(text.as_bytes()),
        }
    }

    #[test]
    fn names() {
        assert_eq!(
            normalize_name(" ./shop\\v1/a.proto ").unwrap(),
            "shop/v1/a.proto"
        );
        assert_eq!(normalize_name("/abs.proto").unwrap(), "abs.proto");
        for bad in [
            "",
            "a//b.proto",
            "../x.proto",
            "a/../b",
            ".hidden",
            "a/.b",
            "a b.proto",
            "x\0",
        ] {
            assert!(normalize_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name(&"a".repeat(300)).is_err());
        assert_eq!(
            ancestors("a/b/c.proto").collect::<Vec<_>>(),
            vec!["a", "a/b"]
        );
    }

    #[test]
    fn plans_in_order_of_precedence() {
        let reg = ProtoRegistry::in_memory();
        assert!(reg.plan(Side::Request, "https://x/a", "/a", None).is_none());
        reg.add(vec![new("catalog.proto", CATALOG)]).unwrap();
        // gRPC path.
        let p = reg
            .plan(
                Side::Request,
                "https://x/shop.v1.Catalog/GetItem",
                "/shop.v1.Catalog/GetItem",
                Some("application/grpc"),
            )
            .unwrap();
        assert_eq!(p.message.full_name(), "shop.v1.GetItemRequest");
        assert_eq!(p.source, SchemaSource::Grpc);
        // ...only for gRPC content.
        assert!(
            reg.plan(
                Side::Request,
                "https://x/shop.v1.Catalog/GetItem",
                "/shop.v1.Catalog/GetItem",
                Some("application/x-protobuf"),
            )
            .is_none()
        );
        // Content-Type parameter.
        let p = reg
            .plan(
                Side::Response,
                "https://x/shop.v1.Catalog/GetItem",
                "/shop.v1.Catalog/GetItem",
                Some("application/grpc; messageType=shop.v1.GetItemRequest"),
            )
            .unwrap();
        assert_eq!(p.source, SchemaSource::ContentType);
        // Mappings: first enabled match whose type exists.
        reg.set_mappings(vec![
            ProtoMapping {
                id: String::new(),
                enabled: true,
                pattern: "x.test/items/*".into(),
                side: MappingSide::Response,
                message_type: "shop.v1.Missing".into(),
            },
            ProtoMapping {
                id: String::new(),
                enabled: true,
                pattern: "x.test/items/*".into(),
                side: MappingSide::Response,
                message_type: "shop.v1.Item".into(),
            },
        ])
        .unwrap();
        let p = reg
            .plan(Side::Response, "https://x.test/items/1", "/items/1", None)
            .unwrap();
        assert_eq!(p.message.full_name(), "shop.v1.Item");
        assert_eq!(p.mapping.as_deref(), Some("x.test/items/*"));
        assert!(
            reg.plan(Side::Request, "https://x.test/items/1", "/items/1", None)
                .is_none()
        );
    }

    #[test]
    fn persists_files_and_mappings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("protos");
        let reg = ProtoRegistry::load(root.clone());
        assert!(reg.info().files.is_empty());
        let info = reg
            .add(vec![
                new("shop/v1/catalog.proto", CATALOG),
                new("bad.proto", "message {"),
            ])
            .unwrap();
        assert_eq!(info.files.len(), 2);
        assert!(!info.files[0].errors.is_empty(), "bad.proto sorts first");
        assert_eq!(
            info.message_types,
            vec!["shop.v1.GetItemRequest", "shop.v1.Item"]
        );
        reg.set_mappings(vec![ProtoMapping {
            id: "m1".into(),
            enabled: true,
            pattern: "/items/*".into(),
            side: MappingSide::Both,
            message_type: "shop.v1.Item".into(),
        }])
        .unwrap();
        assert!(root.join("files/shop/v1/catalog.proto").is_file());

        let again = ProtoRegistry::load(root.clone());
        let info = again.info();
        assert_eq!(info.files.len(), 2);
        assert_eq!(info.mappings[0].id, "m1");
        let plan = again
            .plan(Side::Response, "https://x.test/items/1", "/items/1", None)
            .unwrap();
        assert_eq!(plan.message.full_name(), "shop.v1.Item");
        assert_eq!(
            again.file("shop/v1/catalog.proto").unwrap().1,
            Bytes::from_static(CATALOG.as_bytes())
        );

        // Replace by name, delete one, delete all.
        again
            .add(vec![new(
                "shop/v1/catalog.proto",
                "syntax = \"proto3\";\npackage shop.v1;\nmessage Other {}\n",
            )])
            .unwrap();
        assert_eq!(again.info().message_types, vec!["shop.v1.Other"]);
        again.remove(Some("bad.proto")).unwrap();
        assert!(!root.join("files/bad.proto").exists());
        assert_eq!(
            again.remove(Some("bad.proto")).unwrap_err(),
            RegistryError::NotFound("bad.proto".into())
        );
        again.remove(None).unwrap();
        assert!(again.info().files.is_empty());
        assert!(
            !root.join("files/shop").exists(),
            "empty folders are removed"
        );
        assert!(ProtoRegistry::load(root).info().files.is_empty());
    }

    #[test]
    fn rejects_invalid_changes() {
        let reg = ProtoRegistry::in_memory();
        assert!(matches!(reg.add(vec![]), Err(RegistryError::Invalid(_))));
        assert!(matches!(
            reg.add(vec![new("../evil.proto", CATALOG)]),
            Err(RegistryError::Invalid(_))
        ));
        assert!(matches!(
            reg.add(vec![NewFile {
                name: "big.desc".into(),
                bytes: Bytes::from(vec![0u8; MAX_FILE_SIZE + 1]),
            }]),
            Err(RegistryError::Invalid(_))
        ));
        assert!(matches!(
            reg.add(vec![NewFile {
                name: "empty.desc".into(),
                bytes: Bytes::new(),
            }]),
            Err(RegistryError::Invalid(_))
        ));
        reg.add(vec![new("a.proto", CATALOG)]).unwrap();
        let err = reg.add(vec![new("a.proto/b.proto", CATALOG)]).unwrap_err();
        assert!(err.to_string().contains("clashes"), "{err}");
        assert!(matches!(
            reg.set_mappings(vec![ProtoMapping {
                id: String::new(),
                enabled: true,
                pattern: String::new(),
                side: MappingSide::Both,
                message_type: "a.B".into(),
            }]),
            Err(RegistryError::Invalid(_))
        ));
        assert_eq!(reg.info().files.len(), 1, "unchanged");
    }
}
