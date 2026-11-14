//! `cargo fuzz run socks_handshake`: see `akev_proxy::fuzzing::socks_handshake`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    akev_proxy::fuzzing::socks_handshake(data);
});
