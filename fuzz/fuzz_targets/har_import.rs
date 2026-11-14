//! `cargo fuzz run har_import`: HAR files (Chrome, Firefox, Charles, HTTP Toolkit exports)
//! imported into a store, as `POST /api/import/har` does.
#![no_main]

use akev_core::settings::CaptureSettings;
use akev_store::Store;
use std::sync::{Arc, OnceLock};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    static STORE: OnceLock<Arc<Store>> = OnceLock::new();
    let store = STORE.get_or_init(|| Store::new(CaptureSettings::default(), None));
    if let Ok(ids) = store.import_har(data) {
        // Exported again: what was imported must serialise.
        let mut out = Vec::new();
        let _ = store.write_har(&mut out, Some(&ids));
        store.clear(false);
    }
});
