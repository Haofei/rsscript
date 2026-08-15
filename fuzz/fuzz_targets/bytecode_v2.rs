//! Raw-byte hardening for the typed numeric v2 executable payload.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript_bytecode::v2::{encode_program, BytecodeV2Verifier};

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 * 1024 {
        return;
    }

    if let Ok(verified) = BytecodeV2Verifier::default().verify_payload(data) {
        let canonical = encode_program(verified.program())
            .expect("a verified v2 program must have a canonical encoding");
        assert_eq!(canonical, data, "verified v2 payload is not canonical");
        BytecodeV2Verifier::default()
            .verify_payload(&canonical)
            .expect("a canonical v2 payload must remain verified after round-trip");
    }
});
