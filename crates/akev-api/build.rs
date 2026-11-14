//! Build script: make sure the embedded UI directory exists so `rust-embed` can compile
//! before the web UI has been built (`ui/dist` is produced by `bun run build` in `ui/`).

use std::path::PathBuf;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Akev</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 15px/1.5 system-ui, sans-serif; max-width: 40rem; margin: 15vh auto; padding: 0 1.5rem; }
  code { font: 13px ui-monospace, monospace; padding: .1em .35em; border-radius: 4px; background: rgba(127,127,127,.18); }
</style>
</head>
<body>
<h1>Akev is running</h1>
<p>The web UI has not been built into this binary. Build it with
<code>cd ui &amp;&amp; bun install &amp;&amp; bun run build</code> and rebuild Akev.</p>
<p>The HTTP API, live stream and MCP endpoint are fully available (see <a href="https://akev.dev/docs/api/">akev.dev/docs/api</a>).</p>
</body>
</html>
"#;

fn main() {
    let manifest_dir = match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => return,
    };
    let dist = manifest_dir.join("..").join("..").join("ui").join("dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AKEV_ALLOW_PLACEHOLDER_UI");
    let has_ui = std::fs::read_to_string(dist.join("index.html"))
        .is_ok_and(|html| !html.contains("<h1>Akev is running</h1>"));
    // An optimised build is something someone might ship: it must contain the real UI.
    let release = std::env::var("PROFILE").is_ok_and(|p| p == "release");
    let allow_placeholder = std::env::var("AKEV_ALLOW_PLACEHOLDER_UI").is_ok_and(|v| v == "1");
    if release && !has_ui && !allow_placeholder {
        println!(
            "cargo:warning=ui/dist has no built web UI. Run `make ui` (or `cd ui && bun install && bun run build`) first, or set AKEV_ALLOW_PLACEHOLDER_UI=1 for a build without it."
        );
        std::process::exit(1);
    }
    if !dist.exists() {
        if let Err(err) = std::fs::create_dir_all(&dist) {
            println!("cargo:warning=could not create {}: {err}", dist.display());
            return;
        }
        if let Err(err) = std::fs::write(dist.join("index.html"), PLACEHOLDER) {
            println!("cargo:warning=could not write placeholder UI: {err}");
        }
    }
}
