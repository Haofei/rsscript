//! Checked-in compatibility evidence for the Artifact/Provider boundary.
//!
//! This intentionally uses only the reviewed SDK modules and the canonical
//! `WireValue` Provider path. Legacy v1 bytecode remains a reader fixture, but
//! new Provider authors do not need the dynamic `NativeValue` adapter to prove
//! replacement compatibility.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rsscript_sdk::{
    ProviderRegistry, TerminationReason,
    artifact::ArtifactVerifier,
    compile::{Compiler, FrontendInputSnapshot},
    provider_api::{
        BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
        ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
        ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, RUNTIME_ABI_VERSION,
        ResourceCleanupContract, WireInterpreterFn, WireValue,
    },
    runtime::{ExecutionRequest, Runtime},
};

#[derive(Debug, serde::Deserialize)]
struct ProviderReplacementFixture {
    symbol: String,
    value: String,
    old_reader_fixture: String,
}

const SOURCE: &str = include_str!("corpus/compatibility/provider_replacement.rss");
const INTERFACE: &str = include_str!("corpus/compatibility/echo.rssi");

fn fixture() -> ProviderReplacementFixture {
    toml::from_str(include_str!(
        "corpus/compatibility/provider_replacement.toml"
    ))
    .expect("compatibility corpus manifest is valid TOML")
}

fn signature(effect: DataEffect) -> FunctionSignature {
    FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "value".into(),
            effect,
            ty: "String".into(),
            retained: false,
        }],
        result: "String".into(),
        asynchronous: false,
    }
}

fn descriptor(
    provider_id: impl Into<String>,
    symbol: ExternalSymbol,
    signature: FunctionSignature,
) -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: provider_id.into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol,
            signature,
            entry: "echo".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    }
}

fn compatible_provider(
    provider_id: &str,
    symbol: &ExternalSymbol,
    response: &str,
) -> ProviderRegistry {
    let signature = signature(DataEffect::Read);
    let descriptor = descriptor(provider_id, symbol.clone(), signature.clone());
    let response = response.to_string();
    let mut registry = ProviderRegistry::default();
    registry
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol.clone(),
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new(move |args| match args.as_slice() {
                        [WireValue::String { value: _ }] => Ok(WireValue::String {
                            value: response.clone(),
                        }),
                        _ => Err(ProviderError::invalid_argument(
                            "echo expected exactly one String argument",
                        )),
                    }),
                },
            )]),
        )
        .expect("canonical Provider implementation satisfies its descriptor");
    registry
}

#[test]
fn artifact_provider_compatibility_corpus_is_fail_closed_and_provider_neutral() {
    let fixture = fixture();
    assert_eq!(
        fixture.old_reader_fixture, "rsscript-bytecode/fixtures/v1/reference.rssbundle.base64",
        "the manifest must point at the checked-in deployed-reader fixture"
    );

    let input =
        FrontendInputSnapshot::from_sources([("main.rss", SOURCE)], [("echo.rssi", INTERFACE)]);
    let compiler = Compiler;
    let first = compiler
        .compile_snapshot(&input)
        .expect("compatibility corpus source compiles");
    let second = compiler
        .compile_snapshot(&input)
        .expect("the immutable input compiles repeatedly");
    let first_bytes = first.bundle_bytes().expect("first bundle serializes");
    assert_eq!(
        first_bytes,
        second.bundle_bytes().expect("second bundle serializes"),
        "same source/interface snapshot must produce byte-identical provider-neutral bundles"
    );

    let admitted = ArtifactVerifier
        .verify_bytes(&first_bytes)
        .expect("fresh bundle verifies through the standalone Artifact boundary")
        .admit_trusted_input();
    let symbol = ExternalSymbol::new(&fixture.symbol).expect("fixture external symbol is valid");

    // Provider signature mismatches are link failures, never script failures:
    // the callable must not run while the Artifact is being preflighted.
    let called = Arc::new(AtomicBool::new(false));
    let called_by_provider = Arc::clone(&called);
    let incompatible_signature = signature(DataEffect::Take);
    let incompatible_descriptor = descriptor(
        "compatibility.incompatible",
        symbol.clone(),
        incompatible_signature.clone(),
    );
    let mut incompatible = ProviderRegistry::default();
    incompatible
        .register(
            &incompatible_descriptor,
            BTreeMap::from([(
                symbol.clone(),
                ProviderFunction {
                    signature: incompatible_signature,
                    callable: WireInterpreterFn::new(move |_| {
                        called_by_provider.store(true, Ordering::SeqCst);
                        Ok(WireValue::String {
                            value: "unreachable".into(),
                        })
                    }),
                },
            )]),
        )
        .expect("incompatible Provider remains internally self-consistent");
    let link_error = match Runtime::new(incompatible).link(&admitted) {
        Ok(_) => panic!("an ABI-mismatched Provider must fail before execution"),
        Err(error) => error,
    };
    assert!(link_error.to_string().contains("ImportSignatureMismatch"));
    assert!(!called.load(Ordering::SeqCst));

    // Two distinct compatible Provider identities can link the exact same
    // verified Artifact. Their host implementation is deliberately different
    // only in identity; the script result remains a property of its declared
    // interface contract, not of the Artifact bytes.
    for provider_id in ["compatibility.memory", "compatibility.production"] {
        let report = Runtime::new(compatible_provider(provider_id, &symbol, &fixture.value))
            .link(&admitted)
            .expect("compatible replacement Provider links")
            .execute(ExecutionRequest::default());
        assert_eq!(report.termination_reason(), TerminationReason::Completed);
        assert_eq!(report.value(), Some(fixture.value.as_str()));
        assert_eq!(report.telemetry.provider_functions.len(), 1);
        assert_eq!(
            report.telemetry.provider_functions[0].provider_id,
            provider_id
        );
    }

    // The frozen v1 fixture is intentionally not regenerated from current
    // compiler output. It proves reader compatibility independently from the
    // current v1 writer.
    let legacy_bundle = STANDARD
        .decode(
            include_str!("../../rsscript-bytecode/fixtures/v1/reference.rssbundle.base64").trim(),
        )
        .expect("checked-in v1 compatibility bundle is base64");
    let legacy = ArtifactVerifier
        .verify_bytes(&legacy_bundle)
        .expect("deployed v1 bundle remains readable")
        .admit_trusted_input();
    let legacy_report = Runtime::default()
        .link(&legacy)
        .expect("v1 fixture has no external imports")
        .execute(ExecutionRequest::default());
    assert_eq!(
        legacy_report.termination_reason(),
        TerminationReason::Completed
    );
    assert_eq!(legacy_report.value(), Some("42"));
}
