//! Compiling the stored schema files into one descriptor pool with protox, and describing the
//! result (services, methods, messages, enums and errors per file).
//!
//! * `.proto` sources are compiled at runtime by protox (a pure Rust protobuf compiler); the
//!   well-known `google/protobuf/*.proto` imports are built in.
//! * Descriptor sets (`protoc --descriptor_set_out`, `buf build -o`) are decoded and serve both
//!   as schemas of their own and as imports for `.proto` sources.
//! * Imports are resolved by exact name first. When none matches, a unique stored file whose
//!   path is a suffix of the import (or vice versa, or with the same file name) is used, so a
//!   flat folder of files dropped into the UI compiles even though its imports say
//!   `common/money.proto`. That file is then compiled under the name it is imported as.
//! * Every file is compiled independently: a broken file gets its errors (with line and column
//!   where protox reports a location) and the others still work.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use akev_content::schema::{DescriptorPool, GrpcMethodInfo};
use protox::file::{ChainFileResolver, DescriptorSetFileResolver, File, FileResolver};
use serde::Serialize;

/// Kind of a stored schema file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtoFileKind {
    /// `.proto` source text.
    Proto,
    /// A serialized `google.protobuf.FileDescriptorSet`.
    DescriptorSet,
}

impl ProtoFileKind {
    /// `.proto` names are sources, everything else is a descriptor set.
    pub fn for_name(name: &str) -> Self {
        if name.to_ascii_lowercase().ends_with(".proto") {
            ProtoFileKind::Proto
        } else {
            ProtoFileKind::DescriptorSet
        }
    }
}

/// One stored file, as handed to [`compile`].
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub kind: ProtoFileKind,
    pub bytes: bytes::Bytes,
    /// When it was added (unix ms).
    pub added_at: f64,
}

/// A compile error (`ProtoError` in the HTTP API reference).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoError {
    /// The stored file the error is in.
    pub file: String,
    /// 1-based line, when known.
    pub line: Option<u32>,
    /// 1-based column, when known.
    pub column: Option<u32>,
    pub message: String,
}

/// A gRPC method of a service (`ProtoMethod`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoMethod {
    pub name: String,
    /// The request path: `/package.Service/Method`.
    pub path: String,
    pub input_type: String,
    pub output_type: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

/// A service (`ProtoService`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoService {
    /// Fully-qualified name.
    pub name: String,
    pub methods: Vec<ProtoMethod>,
}

/// One stored file and what it defines (`ProtoFileInfo`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoFileInfo {
    /// Stored name (a relative path, e.g. `shop/v1/catalog.proto`).
    pub name: String,
    pub kind: ProtoFileKind,
    pub size: usize,
    /// Unix ms.
    pub added_at: f64,
    /// The name it is compiled under when imports refer to it by another path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_name: Option<String>,
    /// `package` of a `.proto` source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The `.proto` files a descriptor set holds (well-known `google/protobuf/*` left out).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contains: Vec<String>,
    pub services: Vec<ProtoService>,
    /// Fully-qualified message types (nested ones included, map entries excluded).
    pub messages: Vec<String>,
    /// Fully-qualified enum types.
    pub enums: Vec<String>,
    /// Compile errors (empty = the file compiled).
    pub errors: Vec<ProtoError>,
}

/// The compiled registry.
#[derive(Clone, Debug, Default)]
pub struct Compiled {
    pub pool: DescriptorPool,
    pub files: Vec<ProtoFileInfo>,
    /// Every message type defined by the stored files, sorted.
    pub message_types: Vec<String>,
}

/// Most errors reported per file.
const MAX_ERRORS_PER_FILE: usize = 20;

/// Compile `files` (stored name → file).
pub fn compile(files: &BTreeMap<String, SourceFile>) -> Compiled {
    let mut errors: BTreeMap<String, Vec<ProtoError>> = BTreeMap::new();

    // Sources (parsed once to find their imports) and descriptor sets.
    let mut sources: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    // `package` of every source that parses (shown even when it doesn't compile).
    let mut packages: HashMap<String, String> = HashMap::new();
    let mut sets: Vec<(String, Vec<String>, DescriptorSetFileResolver)> = Vec::new();
    for (name, file) in files {
        match file.kind {
            ProtoFileKind::Proto => {
                let Ok(text) = std::str::from_utf8(&file.bytes) else {
                    add_error(
                        &mut errors,
                        plain_error(name, "the file is not valid UTF-8"),
                    );
                    continue;
                };
                match File::from_source(name, text) {
                    Ok(parsed) => {
                        let fdp = parsed.file_descriptor_proto();
                        if !fdp.package().is_empty() {
                            packages.insert(name.clone(), fdp.package().to_string());
                        }
                        sources.insert(name.clone(), (text.to_string(), fdp.dependency.clone()));
                    }
                    Err(e) => {
                        let names = HashMap::from([(name.clone(), (name.clone(), text))]);
                        for err in convert_error(&e, name, &names) {
                            add_error(&mut errors, err);
                        }
                    }
                }
            }
            ProtoFileKind::DescriptorSet => match decode_set(&file.bytes) {
                Ok((names, resolver)) => sets.push((name.clone(), names, resolver)),
                Err(message) => add_error(&mut errors, plain_error(name, &message)),
            },
        }
    }

    let set_files: BTreeSet<&str> = sets
        .iter()
        .flat_map(|(_, names, _)| names.iter().map(String::as_str))
        .collect();
    let aliases = import_aliases(&sources, &set_files);
    // canonical (compile) name → (stored name, text)
    let mut by_canonical: HashMap<String, (String, &str)> = HashMap::new();
    let mut canonical_of: HashMap<&str, String> = HashMap::new();
    for (stored, (text, _)) in &sources {
        let canonical = aliases
            .iter()
            .find(|(_, s)| s == stored)
            .map_or_else(|| stored.clone(), |(import, _)| import.clone());
        canonical_of.insert(stored.as_str(), canonical.clone());
        by_canonical.insert(canonical, (stored.clone(), text.as_str()));
    }

    let mut chain = ChainFileResolver::new();
    chain.add(MemResolver(
        by_canonical
            .iter()
            .map(|(c, (_, text))| (c.clone(), text.to_string()))
            .collect(),
    ));
    let set_members: Vec<(String, Vec<String>)> = sets
        .iter()
        .map(|(stored, names, _)| (stored.clone(), names.clone()))
        .collect();
    for (_, _, resolver) in sets {
        chain.add(resolver);
    }
    chain.add(protox::file::GoogleFileResolver::new());
    let mut compiler = protox::Compiler::with_file_resolver(chain);

    let error_names: HashMap<String, (String, &str)> = by_canonical
        .iter()
        .map(|(c, (s, t))| (c.clone(), (s.clone(), *t)))
        .collect();
    for stored in sources.keys() {
        let canonical = &canonical_of[stored.as_str()];
        if let Err(e) = compiler.open_file(canonical) {
            let errs = convert_error(&e, stored, &error_names);
            if errs.iter().all(|err| &err.file != stored) {
                // The error is in an imported file (reported there too).
                let other = errs.first().map_or("an import", |e| e.file.as_str());
                add_error(
                    &mut errors,
                    plain_error(stored, &format!("imports {other}, which has errors")),
                );
            }
            for err in errs {
                add_error(&mut errors, err);
            }
        }
    }
    for (stored, names) in &set_members {
        for name in names {
            if let Err(e) = compiler.open_file(name) {
                let mut err = plain_error(stored, &format!("{name}: {e}"));
                err.message = err.message.trim().to_string();
                add_error(&mut errors, err);
            }
        }
    }
    let pool = compiler.descriptor_pool();

    let mut infos = Vec::with_capacity(files.len());
    let mut all_messages = BTreeSet::new();
    for (name, file) in files {
        let mut info = ProtoFileInfo {
            name: name.clone(),
            kind: file.kind,
            size: file.bytes.len(),
            added_at: file.added_at,
            import_name: None,
            package: None,
            contains: Vec::new(),
            services: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
            errors: errors.remove(name).unwrap_or_default(),
        };
        let members: Vec<String> = match file.kind {
            ProtoFileKind::Proto => match canonical_of.get(name.as_str()) {
                Some(canonical) => {
                    if canonical != name {
                        info.import_name = Some(canonical.clone());
                    }
                    vec![canonical.clone()]
                }
                None => Vec::new(),
            },
            ProtoFileKind::DescriptorSet => {
                let names = set_members
                    .iter()
                    .find(|(s, _)| s == name)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_default();
                info.contains = names
                    .iter()
                    .filter(|n| !n.starts_with("google/protobuf/"))
                    .cloned()
                    .collect();
                info.contains.clone()
            }
        };
        for member in &members {
            let Some(fd) = pool.get_file_by_name(member) else {
                continue;
            };
            if file.kind == ProtoFileKind::Proto && !fd.package_name().is_empty() {
                info.package = Some(fd.package_name().to_string());
            }
            for service in fd.services() {
                info.services.push(ProtoService {
                    name: service.full_name().to_string(),
                    methods: service
                        .methods()
                        .map(|m| {
                            let g = GrpcMethodInfo::new(&m);
                            ProtoMethod {
                                path: format!("/{}/{}", g.service, g.method),
                                name: g.method,
                                input_type: g.input_type,
                                output_type: g.output_type,
                                client_streaming: g.client_streaming,
                                server_streaming: g.server_streaming,
                            }
                        })
                        .collect(),
                });
            }
            let mut stack: Vec<_> = fd.messages().collect();
            for e in fd.enums() {
                info.enums.push(e.full_name().to_string());
            }
            while let Some(m) = stack.pop() {
                if !m.is_map_entry() {
                    info.messages.push(m.full_name().to_string());
                }
                for e in m.child_enums() {
                    info.enums.push(e.full_name().to_string());
                }
                stack.extend(m.child_messages());
            }
        }
        if info.package.is_none() {
            info.package = packages.remove(name.as_str());
        }
        info.messages.sort();
        info.enums.sort();
        all_messages.extend(info.messages.iter().cloned());
        infos.push(info);
    }
    Compiled {
        pool,
        files: infos,
        message_types: all_messages.into_iter().collect(),
    }
}

fn add_error(errors: &mut BTreeMap<String, Vec<ProtoError>>, e: ProtoError) {
    let list = errors.entry(e.file.clone()).or_default();
    if list.len() < MAX_ERRORS_PER_FILE && !list.contains(&e) {
        list.push(e);
    }
}

/// Decode a `FileDescriptorSet`: the names of its files and a resolver serving them.
fn decode_set(bytes: &[u8]) -> Result<(Vec<String>, DescriptorSetFileResolver), String> {
    use prost::Message as _;
    use protox::prost_reflect::{prost, prost_types};
    let set = prost_types::FileDescriptorSet::decode(bytes)
        .map_err(|e| format!("not a protobuf descriptor set (FileDescriptorSet): {e}"))?;
    let names: Vec<String> = set.file.iter().map(|f| f.name().to_string()).collect();
    if names.is_empty() || names.iter().any(String::is_empty) {
        return Err(
            "not a protobuf descriptor set: it holds no named files (create one with `protoc --include_imports --descriptor_set_out=…` or `buf build -o …`)"
                .into(),
        );
    }
    let resolver = DescriptorSetFileResolver::decode(bytes)
        .map_err(|e| format!("not a protobuf descriptor set: {e}"))?;
    Ok((names, resolver))
}

/// Import paths that name no stored file exactly, mapped to the one stored source that is
/// meant (unique suffix or file-name match).
fn import_aliases(
    sources: &BTreeMap<String, (String, Vec<String>)>,
    set_files: &BTreeSet<&str>,
) -> Vec<(String, String)> {
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    for (_, imports) in sources.values() {
        wanted.extend(imports.iter().map(String::as_str));
    }
    let mut aliases: Vec<(String, String)> = Vec::new();
    for import in wanted {
        if sources.contains_key(import)
            || set_files.contains(import)
            || import.starts_with("google/protobuf/")
        {
            continue;
        }
        let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
        let candidates: Vec<&String> = sources
            .keys()
            .filter(|s| {
                import.ends_with(&format!("/{s}"))
                    || s.ends_with(&format!("/{import}"))
                    || base(s) == base(import)
            })
            .collect();
        if let [only] = candidates.as_slice()
            // A file can only be compiled under one name.
            && !aliases.iter().any(|(_, s)| s == *only)
            // ...and keeps its own name when something imports it by that.
            && !wanted_exactly(sources, only)
        {
            aliases.push((import.to_string(), (*only).clone()));
        }
    }
    aliases
}

fn wanted_exactly(sources: &BTreeMap<String, (String, Vec<String>)>, name: &str) -> bool {
    sources
        .values()
        .any(|(_, imports)| imports.iter().any(|i| i == name))
}

/// Serves `.proto` sources from memory by their compile name.
struct MemResolver(HashMap<String, String>);

impl FileResolver for MemResolver {
    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        match self.0.get(name) {
            Some(text) => File::from_source(name, text),
            None => Err(protox::Error::file_not_found(name)),
        }
    }
}

fn plain_error(file: &str, message: &str) -> ProtoError {
    ProtoError {
        file: file.to_string(),
        line: None,
        column: None,
        message: message.to_string(),
    }
}

/// A protox error (and the errors related to it) with file names mapped back to stored names
/// and label offsets turned into line / column. `names`: compile name → (stored name, text).
fn convert_error(
    e: &protox::Error,
    fallback_file: &str,
    names: &HashMap<String, (String, &str)>,
) -> Vec<ProtoError> {
    use miette::Diagnostic;
    let (file, text) = match e.file().and_then(|f| names.get(f).map(|n| (f, n))) {
        Some((_, (stored, text))) => (stored.clone(), Some(*text)),
        None => (
            e.file()
                .map(|f| {
                    names
                        .get(f)
                        .map_or_else(|| f.to_string(), |(stored, _)| stored.clone())
                })
                .unwrap_or_else(|| fallback_file.to_string()),
            None,
        ),
    };
    let mut out = Vec::new();
    let mut push = |diag: &dyn Diagnostic| {
        let offset = diag.labels().and_then(|mut l| l.next()).map(|l| l.offset());
        let (line, column) = match (offset, text) {
            (Some(offset), Some(text)) => {
                let (l, c) = line_col(text, offset);
                (Some(l), Some(c))
            }
            _ => (None, None),
        };
        let mut message = diag.to_string();
        if let Some(help) = diag.help() {
            message = format!("{message} ({help})");
        }
        out.push(ProtoError {
            file: file.clone(),
            line,
            column,
            message,
        });
    };
    push(e);
    if let Some(related) = e.related() {
        for r in related.take(MAX_ERRORS_PER_FILE) {
            push(r);
        }
    }
    out
}

/// 1-based line and column (in characters) of a byte offset.
fn line_col(text: &str, offset: usize) -> (u32, u32) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..offset].chars().count() + 1;
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> BTreeMap<String, SourceFile> {
        list.iter()
            .map(|(n, t)| {
                (
                    n.to_string(),
                    SourceFile {
                        kind: ProtoFileKind::for_name(n),
                        bytes: bytes::Bytes::copy_from_slice(t.as_bytes()),
                        added_at: 1.0,
                    },
                )
            })
            .collect()
    }

    const MONEY: &str = "syntax = \"proto3\";\npackage shop.common;\nmessage Money { string currency = 1; int64 units = 2; }\nenum Currency { CURRENCY_UNSPECIFIED = 0; EUR = 1; }\n";
    const CATALOG: &str = "syntax = \"proto3\";\npackage shop.v1;\nimport \"common/money.proto\";\nimport \"google/protobuf/timestamp.proto\";\nmessage Item {\n  message Variant { string sku = 1; }\n  enum Status { UNKNOWN = 0; OK = 1; }\n  string id = 1;\n  shop.common.Money price = 2;\n  map<string, int32> stock = 3;\n  google.protobuf.Timestamp at = 4;\n}\nmessage GetItemRequest { string id = 1; }\nservice Catalog {\n  rpc GetItem(GetItemRequest) returns (Item);\n  rpc Watch(stream GetItemRequest) returns (stream Item);\n}\n";

    #[test]
    fn compiles_imports_and_describes_files() {
        let c = compile(&files(&[
            ("common/money.proto", MONEY),
            ("shop/v1/catalog.proto", CATALOG),
        ]));
        assert!(c.files.iter().all(|f| f.errors.is_empty()), "{:?}", c.files);
        let catalog = c
            .files
            .iter()
            .find(|f| f.name == "shop/v1/catalog.proto")
            .unwrap();
        assert_eq!(catalog.package.as_deref(), Some("shop.v1"));
        assert_eq!(
            catalog.messages,
            vec![
                "shop.v1.GetItemRequest",
                "shop.v1.Item",
                "shop.v1.Item.Variant"
            ]
        );
        assert_eq!(catalog.enums, vec!["shop.v1.Item.Status"]);
        assert_eq!(catalog.services.len(), 1);
        let svc = &catalog.services[0];
        assert_eq!(svc.name, "shop.v1.Catalog");
        assert_eq!(svc.methods[0].path, "/shop.v1.Catalog/GetItem");
        assert_eq!(svc.methods[0].input_type, "shop.v1.GetItemRequest");
        assert!(svc.methods[1].client_streaming && svc.methods[1].server_streaming);
        assert_eq!(c.message_types.len(), 4);
        assert!(
            c.pool
                .get_message_by_name("google.protobuf.Timestamp")
                .is_some()
        );
        assert!(!c.message_types.iter().any(|m| m.starts_with("google.")));
    }

    #[test]
    fn flat_uploads_resolve_imports_by_file_name() {
        let c = compile(&files(&[
            ("money.proto", MONEY),
            ("catalog.proto", CATALOG),
        ]));
        assert!(c.files.iter().all(|f| f.errors.is_empty()), "{:?}", c.files);
        let money = c.files.iter().find(|f| f.name == "money.proto").unwrap();
        assert_eq!(money.import_name.as_deref(), Some("common/money.proto"));
        assert!(c.pool.get_message_by_name("shop.v1.Item").is_some());
    }

    #[test]
    fn errors_have_line_and_column_and_spare_other_files() {
        let broken = "syntax = \"proto3\";\npackage bad;\nmessage A {\n  strin name = 1;\n}\n";
        let c = compile(&files(&[
            ("bad.proto", broken),
            ("common/money.proto", MONEY),
        ]));
        let bad = c.files.iter().find(|f| f.name == "bad.proto").unwrap();
        assert_eq!(bad.errors.len(), 1, "{:?}", bad.errors);
        assert_eq!(bad.package.as_deref(), Some("bad"), "known from the parse");
        let e = &bad.errors[0];
        assert_eq!((e.line, e.column), (Some(4), Some(3)), "{e:?}");
        assert!(e.message.contains("strin"), "{}", e.message);
        assert!(c.pool.get_message_by_name("shop.common.Money").is_some());

        // Syntax errors.
        let c = compile(&files(&[("s.proto", "syntax = \"proto3\";\nmessage {\n")]));
        let e = &c.files[0].errors[0];
        assert_eq!(e.line, Some(2), "{e:?}");

        // Missing import.
        let c = compile(&files(&[("shop/v1/catalog.proto", CATALOG)]));
        let errs = &c.files[0].errors;
        assert!(
            errs.iter()
                .any(|e| e.message.contains("common/money.proto") && e.line == Some(3)),
            "{errs:?}"
        );

        // An error in an imported file shows on both.
        let broken_money =
            "syntax = \"proto3\";\npackage shop.common;\nmessage Money { Nope x = 1; }\n";
        let c = compile(&files(&[
            ("common/money.proto", broken_money),
            ("shop/v1/catalog.proto", CATALOG),
        ]));
        let catalog = c
            .files
            .iter()
            .find(|f| f.name == "shop/v1/catalog.proto")
            .unwrap();
        assert!(
            catalog
                .errors
                .iter()
                .any(|e| e.message.contains("which has errors")),
            "{:?}",
            catalog.errors
        );
        let money = c
            .files
            .iter()
            .find(|f| f.name == "common/money.proto")
            .unwrap();
        assert!(
            money.errors.iter().any(|e| e.line == Some(3)),
            "{:?}",
            money.errors
        );

        // Not UTF-8.
        let mut f = files(&[]);
        f.insert(
            "x.proto".into(),
            SourceFile {
                kind: ProtoFileKind::Proto,
                bytes: bytes::Bytes::from_static(&[0xff, 0xfe]),
                added_at: 0.0,
            },
        );
        assert!(compile(&f).files[0].errors[0].message.contains("UTF-8"));
    }

    #[test]
    fn duplicate_definitions_are_reported() {
        let c = compile(&files(&[("a.proto", MONEY), ("b/money.proto", MONEY)]));
        let total: usize = c.files.iter().map(|f| f.errors.len()).sum();
        assert!(total >= 1, "{:?}", c.files);
        assert!(c.pool.get_message_by_name("shop.common.Money").is_some());
    }

    #[test]
    fn descriptor_sets_are_schemas_and_imports() {
        // Build a descriptor set from the sources.
        let c = compile(&files(&[
            ("common/money.proto", MONEY),
            ("shop/v1/catalog.proto", CATALOG),
        ]));
        let set = c.pool.encode_to_vec();
        let mut f = BTreeMap::new();
        f.insert(
            "api.desc".to_string(),
            SourceFile {
                kind: ProtoFileKind::DescriptorSet,
                bytes: bytes::Bytes::from(set.clone()),
                added_at: 2.0,
            },
        );
        let c2 = compile(&f);
        let info = &c2.files[0];
        assert!(info.errors.is_empty(), "{:?}", info.errors);
        assert_eq!(info.kind, ProtoFileKind::DescriptorSet);
        assert_eq!(
            info.contains,
            vec!["common/money.proto", "shop/v1/catalog.proto"]
        );
        assert_eq!(info.services[0].name, "shop.v1.Catalog");
        assert!(info.messages.contains(&"shop.v1.Item".to_string()));

        // A source importing a file of the set.
        let user = "syntax = \"proto3\";\npackage user;\nimport \"common/money.proto\";\nmessage Wallet { shop.common.Money balance = 1; }\n";
        f.insert(
            "user.proto".into(),
            SourceFile {
                kind: ProtoFileKind::Proto,
                bytes: bytes::Bytes::from_static(user.as_bytes()),
                added_at: 3.0,
            },
        );
        let c3 = compile(&f);
        assert!(
            c3.files.iter().all(|f| f.errors.is_empty()),
            "{:?}",
            c3.files
        );
        assert!(c3.pool.get_message_by_name("user.Wallet").is_some());

        // Garbage.
        let mut g = BTreeMap::new();
        g.insert(
            "x.desc".to_string(),
            SourceFile {
                kind: ProtoFileKind::DescriptorSet,
                bytes: bytes::Bytes::from_static(b"\xff\xff\xff"),
                added_at: 0.0,
            },
        );
        let e = &compile(&g).files[0].errors[0];
        assert!(e.message.contains("descriptor set"), "{e:?}");
    }

    #[test]
    fn line_columns() {
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
        assert_eq!(line_col("é\nx", 3), (2, 1));
        assert_eq!(line_col("é", 1), (1, 1), "offset inside a character");
        assert_eq!(line_col("abc", 99), (1, 4));
    }
}
