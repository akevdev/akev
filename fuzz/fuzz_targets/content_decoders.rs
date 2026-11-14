//! `cargo fuzz run content_decoders`: the structured body decoders (protobuf without a
//! schema, CBOR, MessagePack) shown in the UI and to agents.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Ok(fields) = akev_content::protobuf::decode_raw(data) {
        let _ = akev_content::protobuf::to_json(&fields);
    }
    let _ = akev_content::cbor::to_json(data);
    let _ = akev_content::msgpack::to_json(data);
});
