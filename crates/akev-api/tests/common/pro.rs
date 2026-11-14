//! A test-only Akev Pro licence for suites that exercise Pro features (`#[path]`-included).
//!
//! The key is signed with the publicly known test seed (bytes 0x00..0x1f), which only an
//! instance started with these options trusts; release builds never do.
#![allow(dead_code)]

use akev_api::AkevOptions;

/// Public key of the test seed.
pub const TEST_PUBLIC_KEY: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";
/// Pro licence for "Akev tests" covering builds released until 2099-12-31.
pub const TEST_KEY: &str = "AKEV1.eyJ2IjoxLCJpZCI6InRlc3QtcHJvIiwiZW1haWwiOiJ0ZXN0c0BleGFtcGxlLmNvbSIsIm5hbWUiOiJBa2V2IHRlc3RzIiwicGxhbiI6InBybyIsImlzc3VlZCI6IjIwMjYtMTEtMTEiLCJ1cGRhdGVzVW50aWwiOiIyMDk5LTEyLTMxIn0.GiZXJs-HK18qTi-5jlZfdX_hLY9AN8n3G2orSMd5TMF_gtSt0MWyih9-JKolMitgefYZVSz_Tmg7r6Fa1PSyDw";

/// Default options with the test key trusted but no licence (Free).
pub fn free() -> AkevOptions {
    AkevOptions {
        license_public_keys: vec![TEST_PUBLIC_KEY.into()],
        ..AkevOptions::default()
    }
}

/// Default options with the test Pro licence.
pub fn options() -> AkevOptions {
    AkevOptions {
        license: Some(TEST_KEY.into()),
        ..free()
    }
}
