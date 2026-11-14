// A build script should fail loudly; panicking is how it reports errors.
#![allow(clippy::expect_used)]

fn main() {
    // The UI is loaded from the in-process daemon, which Tauri treats as a remote URL: its calls
    // to app commands go through the ACL like plugin commands do. List the commands so Tauri
    // generates `allow-<command>` permissions (granted in capabilities/default.json).
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["save_file", "install_update"]),
        ),
    )
    .expect("tauri-build failed");
}
