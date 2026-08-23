//! Raw-byte hardening for the independently verifiable Artifact boundary.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript_bytecode::{BytecodeArtifact, BytecodeVerifier};

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 * 1024 {
        return;
    }

    let _ = BytecodeArtifact::from_bytes(data);
    if let Ok(verified) = BytecodeVerifier::default().verify(data) {
        let canonical = verified
            .artifact()
            .to_bytes()
            .expect("a verified Artifact must serialize");
        assert_eq!(
            canonical, data,
            "verified Artifact encoding is not canonical"
        );
        BytecodeVerifier::default()
            .verify(&canonical)
            .expect("a verified Artifact must remain verified after round-trip");
    }
});
