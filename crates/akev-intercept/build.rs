//! Builds the JVM agent jar (`java/`) into `$OUT_DIR/akev-agent.jar`, which the crate embeds.
//!
//! The sources are compiled with a JDK (`javac` + `jar` from `$JAVA_HOME/bin` or `PATH`) for
//! Java 8 (`--release 8`, or `-source/-target 8` on a Java 8 `javac`); the jar's manifest
//! carries `Akev-Source-Hash` (`src/jvm/source_hash.rs`). No binary jar is committed.
//!
//! Without a JDK the build still succeeds, with an empty jar: JVM attach then reports that
//! this build has no agent (cfg `akev_no_agent`). Official builds set
//! `AKEV_AGENT_REQUIRE_JDK=1`, which turns that into a build error.
// A build script should fail loudly; panicking is how it reports errors.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

include!("src/jvm/source_hash.rs");

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let java_dir = manifest_dir.join("java");
    println!("cargo:rerun-if-changed=java");
    println!("cargo:rerun-if-changed=src/jvm/source_hash.rs");
    println!("cargo::rustc-check-cfg=cfg(akev_no_agent)");
    for var in ["JAVA_HOME", "PATH", "AKEV_AGENT_REQUIRE_JDK"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let hash = java_sources_hash(&java_dir).expect("hash the Java agent sources");
    let target = out_dir.join("akev-agent.jar");
    match build_jar(&java_dir, &out_dir, &hash) {
        Some(jar) => {
            std::fs::copy(&jar, &target).expect("copy the agent jar");
        }
        None if flag("AKEV_AGENT_REQUIRE_JDK") => {
            panic!(
                "AKEV_AGENT_REQUIRE_JDK is set but the JVM agent couldn't be built (see the warnings above); install a JDK 8+ or set JAVA_HOME"
            );
        }
        None => {
            println!(
                "cargo:warning=no JDK: this build has no JVM agent, so JVM attach is unavailable (install a JDK 8+ to include it)"
            );
            println!("cargo:rustc-cfg=akev_no_agent");
            std::fs::write(&target, b"").expect("write the empty agent jar");
        }
    }
}

fn flag(name: &str) -> bool {
    env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// `javac` and `jar` from `$JAVA_HOME/bin`, else from `PATH`.
fn jdk_tools() -> Option<(PathBuf, PathBuf)> {
    let exe = |name: &str| {
        if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        }
    };
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
    if let Some(home) = env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        let bin = PathBuf::from(home).join("bin");
        candidates.push((bin.join(exe("javac")), bin.join(exe("jar"))));
    }
    candidates.push((PathBuf::from(exe("javac")), PathBuf::from(exe("jar"))));
    candidates.into_iter().find(|(javac, _)| {
        Command::new(javac)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "java") {
            out.push(path);
        }
    }
}

fn build_jar(java_dir: &Path, out_dir: &Path, hash: &str) -> Option<PathBuf> {
    let (javac, jar) = jdk_tools()?;
    let classes = out_dir.join("agent-classes");
    let _ = std::fs::remove_dir_all(&classes);
    std::fs::create_dir_all(&classes).ok()?;
    let mut files = Vec::new();
    sources(&java_dir.join("src"), &mut files);
    files.sort();

    let compile = |release: &[&str]| {
        let _ = std::fs::remove_dir_all(&classes);
        let _ = std::fs::create_dir_all(&classes);
        Command::new(&javac)
            .args([
                "-encoding",
                "UTF-8",
                "-Xlint:all,-options",
                "-g:source,lines",
            ])
            .args(release)
            .arg("-d")
            .arg(&classes)
            .args(&files)
            .output()
    };
    let mut result = compile(&["--release", "8"]);
    if !result.as_ref().is_ok_and(|o| o.status.success()) {
        // Java 8's javac has no --release.
        result = compile(&["-source", "8", "-target", "8"]);
    }
    match result {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            println!(
                "cargo:warning=javac failed: {}",
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
            );
            return None;
        }
        Err(e) => {
            println!("cargo:warning=couldn't run javac ({e})");
            return None;
        }
    }

    let mut manifest = std::fs::read_to_string(java_dir.join("MANIFEST.MF")).ok()?;
    manifest = manifest.replace("\r\n", "\n");
    if !manifest.ends_with('\n') {
        manifest.push('\n');
    }
    manifest.push_str(&format!("Akev-Source-Hash: {hash}\n"));
    let manifest_path = out_dir.join("agent-MANIFEST.MF");
    std::fs::write(&manifest_path, manifest).ok()?;
    let out = out_dir.join("akev-agent.built.jar");
    let _ = std::fs::remove_file(&out);
    // `0`: store entries uncompressed, so the manifest stamp is greppable in the jar bytes.
    let status = Command::new(&jar)
        .arg("c0fm")
        .arg(&out)
        .arg(&manifest_path)
        .arg("-C")
        .arg(&classes)
        .arg(".")
        .output();
    match status {
        Ok(o) if o.status.success() && out.is_file() => Some(out),
        Ok(o) => {
            println!(
                "cargo:warning=jar failed: {}",
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
            );
            None
        }
        Err(e) => {
            println!("cargo:warning=couldn't run jar ({e})");
            None
        }
    }
}
