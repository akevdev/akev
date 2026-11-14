//! `cargo fuzz run proxy_protocol`: see `akev_proxy::fuzzing::proxy_protocol`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    akev_proxy::fuzzing::proxy_protocol(data);
});
