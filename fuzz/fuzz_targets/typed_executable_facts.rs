//! Raw-byte hardening for the optional typed executable facts section.

#![no_main]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use libfuzzer_sys::fuzz_target;
use rsscript_bytecode::{
    BytecodeArtifact, BytecodeLimits, TypedExecutableFactsVerifierV1, TypedFactsLimits,
    encode_typed_executable_facts,
};
use rsscript_sdk::ArtifactBundle;

fn reference_artifact() -> BytecodeArtifact {
    let encoded =
        include_str!("../../crates/rsscript-bytecode/fixtures/v1/reference.rssbundle.base64");
    let bytes = STANDARD
        .decode(encoded.trim())
        .expect("checked-in v1 fixture is valid base64");
    let bundle = ArtifactBundle::from_bytes(&bytes)
        .expect("checked-in v1 fixture is a canonical Artifact Bundle");
    BytecodeArtifact::from_bytes(bundle.artifact_bytes())
        .expect("checked-in v1 fixture contains a canonical bytecode artifact")
}

fuzz_target!(|data: &[u8]| {
    let limits = TypedFactsLimits::from(BytecodeLimits::default());
    if data.len() > limits.max_bytes.saturating_add(1) {
        return;
    }
    let artifact = reference_artifact();
    let verifier = TypedExecutableFactsVerifierV1::new(limits);
    if let Ok(verified) = verifier.verify(data, &artifact) {
        let canonical = encode_typed_executable_facts(verified.facts())
            .expect("verified typed facts must encode canonically");
        assert_eq!(canonical, data, "accepted typed facts must be canonical");
        verifier
            .verify(&canonical, &artifact)
            .expect("verified typed facts must remain verified after round-trip");
    }
});
