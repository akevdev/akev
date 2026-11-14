//! `cargo fuzz run http1_request`: see `akev_proxy::fuzzing::http1_request`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    akev_proxy::fuzzing::http1_request(data);
});
