// Shared by `build.rs` (through `include!`) and the crate's tests, so it only uses `std` and
// `sha2`.

/// Fingerprint of the Java agent sources: SHA-256 (first 16 bytes, hex) over every `.java`
/// file under `<java_dir>/src` and `<java_dir>/MANIFEST.MF`, by sorted relative path, with
/// line endings normalised to `\n` (so Windows checkouts hash the same). `build.rs` stamps it
/// into the jar manifest as `Akev-Source-Hash`.
pub(crate) fn java_sources_hash(java_dir: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};

    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect(&path, out)?;
            } else if path.extension().is_some_and(|e| e == "java") {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(&java_dir.join("src"), &mut files)?;
    files.push(java_dir.join("MANIFEST.MF"));
    let mut named: Vec<(String, std::path::PathBuf)> = files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(java_dir)
                .unwrap_or(&p)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (rel, p)
        })
        .collect();
    named.sort();
    let mut hasher = Sha256::new();
    for (name, path) in named {
        let raw = std::fs::read(&path)?;
        let mut text = Vec::with_capacity(raw.len());
        for (i, b) in raw.iter().enumerate() {
            if *b == b'\r' && raw.get(i + 1) == Some(&b'\n') {
                continue;
            }
            text.push(*b);
        }
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(&text);
    }
    Ok(hasher.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
