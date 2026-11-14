//! `cargo fuzz run http1_response`: see `akev_proxy::fuzzing::http1_response`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    akev_proxy::fuzzing::http1_response(data);
});
