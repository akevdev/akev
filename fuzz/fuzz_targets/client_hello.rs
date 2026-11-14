//! `cargo fuzz run client_hello`: see `akev_proxy::fuzzing::client_hello`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    akev_proxy::fuzzing::client_hello(data);
});
