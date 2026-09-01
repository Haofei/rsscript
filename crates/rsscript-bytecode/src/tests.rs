use super::*;
use proptest::prelude::*;
use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, WireType,
};

const TEST_CATALOG_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const TEST_SOURCE_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn round_trip_requires_intact_artifact() {
    let payload = minimal_payload();
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload.clone(),
    )
    .expect("artifact");
    let bytes = artifact.to_bytes().expect("bytes");
    let verified = BytecodeVerifier::default()
        .verify(&bytes)
        .expect("verified");
    assert_eq!(verified.artifact().payload, payload);
    assert!(verified.typed_executable_facts().is_none());

    let mut corrupt = bytes;
    *corrupt.last_mut().expect("non-empty") ^= 1;
    assert!(BytecodeVerifier::default().verify(&corrupt).is_err());
}

#[test]
fn optional_typed_facts_round_trip_through_verifier_owned_wrapper() {
    let payload = minimal_payload();
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");
    let facts = TypedExecutableFactsV1 {
        schema: TYPED_EXECUTABLE_FACTS_SCHEMA_V1.to_owned(),
        executable_hash: artifact.header.executable_hash.clone(),
        bytecode_isa_version: artifact.header.bytecode_isa_version,
        runtime_abi_version: artifact.header.runtime_abi_version,
        interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
        imports_hash: typed_facts_imports_hash(&artifact).expect("imports hash"),
        functions: vec![],
        layouts: vec![],
    };
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach facts");
    let verified = BytecodeVerifier::default()
        .verify(&artifact.to_bytes().expect("encode artifact"))
        .expect("verify artifact and facts");
    assert_eq!(
        verified
            .typed_executable_facts()
            .expect("verified facts")
            .facts(),
        &facts
    );
}

#[test]
fn malformed_recognized_typed_facts_fail_closed() {
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .expect("artifact");
    let mut bytes = artifact.to_bytes().expect("artifact bytes");
    bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
    append_test_section(&mut bytes, SECTION_TYPED_EXECUTABLE_FACTS, 0, b"not-cbor");
    assert!(matches!(
        BytecodeVerifier::default().verify(&bytes),
        Err(BytecodeError::InvalidTypedExecutableFacts(_))
    ));
}

#[test]
fn typed_facts_digest_binding_rejects_tampering() {
    let payload = minimal_payload();
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");
    artifact.typed_executable_facts = Some(
        encode_typed_executable_facts(&TypedExecutableFactsV1 {
            schema: TYPED_EXECUTABLE_FACTS_SCHEMA_V1.to_owned(),
            executable_hash: format!("sha256:{}", "f".repeat(64)),
            bytecode_isa_version: artifact.header.bytecode_isa_version,
            runtime_abi_version: artifact.header.runtime_abi_version,
            interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
            imports_hash: typed_facts_imports_hash(&artifact).expect("imports hash"),
            functions: vec![],
            layouts: vec![],
        })
        .expect("encode facts"),
    );
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::TypedFactsBindingMismatch("executable hash"))
    ));
}

#[test]
fn typed_facts_have_an_independent_size_limit() {
    let payload = minimal_payload();
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");
    let facts = TypedExecutableFactsV1 {
        schema: TYPED_EXECUTABLE_FACTS_SCHEMA_V1.to_owned(),
        executable_hash: artifact.header.executable_hash.clone(),
        bytecode_isa_version: artifact.header.bytecode_isa_version,
        runtime_abi_version: artifact.header.runtime_abi_version,
        interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
        imports_hash: typed_facts_imports_hash(&artifact).expect("imports hash"),
        functions: vec![],
        layouts: vec![],
    };
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach facts");
    let limits = BytecodeLimits {
        max_typed_facts_bytes: 1,
        ..BytecodeLimits::default()
    };
    assert!(matches!(
        BytecodeVerifier::new(limits).verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::LimitExceeded("typed facts bytes"))
    ));
}

#[test]
fn typed_facts_cannot_be_transplanted_between_import_catalogs() {
    let symbol = ExternalSymbol::new("host.test.value").expect("symbol");
    let make = |result: rsscript_abi_model::WireType| {
        let signature = FunctionSignature {
            parameters: vec![],
            result,
            asynchronous: false,
        };
        BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![ExternalImport {
                symbol: symbol.clone(),
                signature: signature.clone(),
                signature_hash: signature.hash(),
                abi_version: RUNTIME_ABI_VERSION,
            }],
            external_call_payload(symbol.as_str()),
        )
        .expect("artifact")
    };
    let mut source = make(rsscript_abi_model::WireType::Unit);
    let facts = empty_function_facts(&source, 1);
    source
        .attach_typed_executable_facts(&facts)
        .expect("attach source facts");

    let mut recipient = make(rsscript_abi_model::WireType::Int {
        bits: 64,
        signed: true,
    });
    recipient.typed_executable_facts = source.typed_executable_facts;
    let error = BytecodeVerifier::default()
        .verify(&recipient.to_bytes().expect("recipient bytes"))
        .expect_err("transplanted facts must fail");
    assert!(
        matches!(
            error,
            BytecodeError::TypedFactsBindingMismatch("imports hash")
        ),
        "{error:?}"
    );
}

#[test]
fn typed_call_target_must_match_the_bound_instruction() {
    let symbol = ExternalSymbol::new("host.test.value").expect("symbol");
    let signature = FunctionSignature {
        parameters: vec![],
        result: rsscript_abi_model::WireType::Unit,
        asynchronous: false,
    };
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![ExternalImport {
            symbol: symbol.clone(),
            signature: signature.clone(),
            signature_hash: signature.hash(),
            abi_version: RUNTIME_ABI_VERSION,
        }],
        external_call_payload(symbol.as_str()),
    )
    .expect("artifact");
    let mut facts = empty_function_facts(&artifact, 1);
    facts.functions[0].call_sites.push(TypedCallSiteV1 {
        instruction: 0,
        target: TypedCallTargetV1::Provider(1),
        parameters: vec![],
        result: TypedFactTypeV1::Known(rsscript_abi_model::WireType::Unit),
        parameter_effects: vec![],
        type_parameters: vec![],
        type_arguments: vec![],
    });
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attachment checks only envelope binding");
    let error = BytecodeVerifier::default()
        .verify(&artifact.to_bytes().expect("artifact bytes"))
        .expect_err("mismatched target must fail");
    assert!(
        matches!(
            error,
            BytecodeError::InvalidTypedExecutableFacts(ref message)
                if message.contains("out of range")
        ),
        "{error:?}"
    );
}

#[test]
fn v1_known_functions_cannot_claim_erased_generic_substitutions() {
    let symbol = ExternalSymbol::new("host.test.value").expect("symbol");
    let signature = FunctionSignature {
        parameters: vec![],
        result: rsscript_abi_model::WireType::Unit,
        asynchronous: false,
    };
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![ExternalImport {
            symbol: symbol.clone(),
            signature: signature.clone(),
            signature_hash: signature.hash(),
            abi_version: RUNTIME_ABI_VERSION,
        }],
        external_call_payload(symbol.as_str()),
    )
    .expect("artifact");
    let mut facts = empty_function_facts(&artifact, 1);
    facts.functions[0].generic_substitutions = vec![rsscript_abi_model::WireType::Int {
        bits: 64,
        signed: true,
    }];
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach bound facts");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::InvalidTypedExecutableFacts(message))
            if message.contains("does not prove function generic substitutions")
    ));
}

#[test]
fn typed_facts_must_cover_every_executable_call() {
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        known_call_payload(),
    )
    .expect("artifact");
    let facts = two_function_facts(&artifact);
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach omitted call facts");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::InvalidTypedExecutableFacts(message))
            if message.contains("completely cover")
    ));
}

#[test]
fn known_call_signature_is_rederived() {
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        known_call_payload(),
    )
    .expect("artifact");
    let mut facts = two_function_facts(&artifact);
    facts.functions[0].call_sites.push(TypedCallSiteV1 {
        instruction: 0,
        target: TypedCallTargetV1::KnownFunction(1),
        parameters: vec![TypedFactTypeV1::Known(WireType::Bool)],
        result: TypedFactTypeV1::Known(WireType::Bool),
        parameter_effects: vec![TypedDataEffectV1::Read],
        type_parameters: vec![],
        type_arguments: vec![],
    });
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach forged call facts");
    let error = BytecodeVerifier::default()
        .verify(&artifact.to_bytes().expect("artifact bytes"))
        .expect_err("forged static signature must fail");
    assert!(
        matches!(
            error,
            BytecodeError::InvalidTypedExecutableFacts(ref message)
                if message.contains("parameter or result")
        ),
        "{error:?}"
    );
}

#[test]
fn known_call_mutation_effect_is_rederived_from_mut_args() {
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        known_call_payload(),
    )
    .expect("artifact");
    let mut facts = two_function_facts(&artifact);
    let scalar = facts.functions[1].registers[0].ty.clone();
    facts.functions[0].call_sites.push(TypedCallSiteV1 {
        instruction: 0,
        target: TypedCallTargetV1::KnownFunction(1),
        parameters: vec![scalar.clone()],
        result: scalar,
        parameter_effects: vec![TypedDataEffectV1::Mutate],
        type_parameters: vec![],
        type_arguments: vec![],
    });
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach forged effect facts");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::InvalidTypedExecutableFacts(message))
            if message.contains("mutation effects")
    ));
}

#[test]
fn register_claims_are_intersected_with_literal_and_return_types() {
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        scalar_return_payload(),
    )
    .expect("artifact");
    let mut facts = empty_function_facts(&artifact, 1);
    facts.functions[0].registers[0].ty = TypedFactTypeV1::Known(WireType::Float { bits: 64 });
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach forged register facts");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::InvalidTypedExecutableFacts(message))
            if message.contains("independently derived")
    ));
}

#[test]
fn typed_layouts_are_exact_references_to_executable_layouts() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [],
        "function_ids": {},
        "resource_drop_functions": {},
        "types": {
            "Point": {"name": "Point", "fields": [
                {"name": "x", "type_name": "owned Int"}
            ]}
        },
        "variant_layouts": {
            "Choice": {"name": "Choice", "variants": [
                {"name": "Some", "fields": [
                    {"name": "value", "type_name": "String"}
                ]}
            ]}
        },
        "native_signatures": {},
        "closure_identity_observable": false
    }))
    .expect("payload");
    let mut artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");
    let mut facts = empty_function_facts(&artifact, 0);
    facts.functions.clear();
    facts.layouts = vec![
        TypedLayoutV1 {
            layout_id: 0,
            name: "Choice".to_owned(),
            kind: TypedLayoutKindV1::Variant,
            fields: vec![TypedLayoutFieldV1 {
                case: Some("Some".to_owned()),
                name: "value".to_owned(),
                ty: TypedFactTypeV1::Known(WireType::String),
            }],
        },
        TypedLayoutV1 {
            layout_id: 1,
            name: "Point".to_owned(),
            kind: TypedLayoutKindV1::Record,
            fields: vec![TypedLayoutFieldV1 {
                case: None,
                name: "x".to_owned(),
                ty: TypedFactTypeV1::Known(WireType::Qualified {
                    qualifier: rsscript_abi_model::WireQualifier::Owned,
                    value: Box::new(WireType::Bool),
                }),
            }],
        },
    ];
    artifact
        .attach_typed_executable_facts(&facts)
        .expect("attach forged layout facts");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("artifact bytes")),
        Err(BytecodeError::InvalidTypedExecutableFacts(message))
            if message.contains("case, field name, or type")
    ));
}

#[test]
fn language_compatibility_is_independent_from_compiler_provenance() {
    let compatibility = BytecodeCompatibility::default();
    assert!(compatibility.language.matches(
        &Version::parse(LANGUAGE_SEMANTICS_VERSION).expect("declared language semantics version")
    ));
    assert!(
        !compatibility
            .language
            .matches(&Version::parse("0.2.0").expect("test version"))
    );
    assert_eq!(BYTECODE_SCHEMA, "rsscript.bytecode.v1");
    assert_eq!(BYTECODE_CONTAINER_FORMAT_VERSION, 1);
    assert_eq!(
        u16::from_le_bytes([BYTECODE_MAGIC[6], BYTECODE_MAGIC[7]]),
        BYTECODE_CONTAINER_FORMAT_VERSION
    );
    assert_eq!(BYTECODE_ISA_VERSION, 1);
    assert_eq!(CORE_LIBRARY_ABI_VERSION, 1);
}

#[test]
fn verification_observes_cancellation_and_deadline_before_decoding() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        BytecodeVerifier::default().verify_with_context(
            b"not bytecode",
            VerificationContext {
                cancellation: Some(&cancellation),
                deadline: None,
            },
        ),
        Err(BytecodeError::Cancelled)
    ));
    assert!(matches!(
        BytecodeVerifier::default().verify_with_context(
            b"not bytecode",
            VerificationContext {
                cancellation: None,
                deadline: Some(MonotonicDeadline::at(
                    std::time::Instant::now() - std::time::Duration::from_millis(1),
                )),
            },
        ),
        Err(BytecodeError::DeadlineExceeded)
    ));
}

#[test]
fn verifier_errors_expose_stable_machine_codes() {
    assert_eq!(
        BytecodeError::InvalidMagic.code(),
        BytecodeErrorCode::InvalidMagic
    );
    assert_eq!(
        serde_json::to_string(&BytecodeError::InvalidMagic.code()).unwrap(),
        "\"invalid_magic\""
    );
    assert_eq!(
        BytecodeError::UnsupportedBytecodeIsa {
            artifact: 9,
            verifier: 1,
        }
        .code(),
        BytecodeErrorCode::UnsupportedBytecodeIsa
    );
    assert_eq!(
        BytecodeError::UnsupportedRuntimeAbi {
            artifact: 9,
            runtime: 1,
        }
        .code(),
        BytecodeErrorCode::UnsupportedRuntimeAbi
    );
    assert_eq!(
        BytecodeError::UnsupportedCoreLibraryAbi {
            artifact: 9,
            runtime: 1,
        }
        .code(),
        BytecodeErrorCode::UnsupportedCoreLibraryAbi
    );
}

#[test]
fn verifier_rejects_incompatible_language_and_runtime_versions() {
    let future_language = BytecodeArtifact::new(
        "9.0.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .unwrap();
    assert!(matches!(
        BytecodeVerifier::default().verify(&future_language.to_bytes().unwrap()),
        Err(BytecodeError::UnsupportedLanguageVersion(version)) if version == "9.0.0"
    ));

    let future_abi = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION + 1,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .unwrap();
    assert!(matches!(
        BytecodeVerifier::default().verify(&future_abi.to_bytes().unwrap()),
        Err(BytecodeError::UnsupportedRuntimeAbi { artifact, runtime })
            if artifact == RUNTIME_ABI_VERSION + 1 && runtime == RUNTIME_ABI_VERSION
    ));

    let mut future_isa = future_abi.clone();
    future_isa.header.bytecode_isa_version = BYTECODE_ISA_VERSION + 1;
    future_isa.checksum = future_isa.compute_checksum().unwrap();
    assert!(matches!(
        BytecodeVerifier::default().verify(&future_isa.to_bytes().unwrap()),
        Err(BytecodeError::UnsupportedBytecodeIsa { artifact, verifier })
            if artifact == BYTECODE_ISA_VERSION + 1 && verifier == BYTECODE_ISA_VERSION
    ));

    let mut future_corelib = future_abi;
    future_corelib.header.core_library_abi_version = CORE_LIBRARY_ABI_VERSION + 1;
    future_corelib.checksum = future_corelib.compute_checksum().unwrap();
    assert!(matches!(
        BytecodeVerifier::default().verify(&future_corelib.to_bytes().unwrap()),
        Err(BytecodeError::UnsupportedCoreLibraryAbi { artifact, runtime })
            if artifact == CORE_LIBRARY_ABI_VERSION + 1 && runtime == CORE_LIBRARY_ABI_VERSION
    ));
}

#[test]
fn compatibility_ranges_and_unknown_container_majors_are_explicit() {
    let previous_minor = BytecodeArtifact::new(
        "0.0.9",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .unwrap();
    let compatibility = BytecodeCompatibility {
        language: VersionReq::parse(">=0.0.0, <0.2.0").unwrap(),
        ..BytecodeCompatibility::default()
    };
    BytecodeVerifier::with_compatibility(BytecodeLimits::default(), compatibility)
        .verify(&previous_minor.to_bytes().unwrap())
        .expect("declared N-1 language range accepts a compatible artifact");

    let mut unknown_container = previous_minor.to_bytes().unwrap();
    unknown_container[6] = BYTECODE_CONTAINER_FORMAT_VERSION.saturating_add(1) as u8;
    assert!(matches!(
        BytecodeArtifact::from_bytes(&unknown_container),
        Err(BytecodeError::InvalidMagic)
    ));
}

#[test]
fn artifact_sections_and_instruction_payload_use_binary_cbor() {
    let payload = minimal_payload();
    assert_ne!(payload.first(), Some(&b'{'));
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");
    let bytes = artifact.to_bytes().expect("bytes");
    let first_section_data = BYTECODE_MAGIC.len() + 2 + SECTION_HEADER_BYTES;
    assert_ne!(bytes.get(first_section_data), Some(&b'{'));
    BytecodeVerifier::default().verify(&bytes).unwrap();
}

#[test]
fn named_variant_instructions_must_match_the_declared_layout_table() {
    let payload = |case: &str| {
        encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {},
                    "code": [
                        {"MakeVariant": {"dst": 0, "layout": {"name": case, "field_names": []}, "fields": []}},
                        {"Return": {"src": 0}}
                    ]
                }],
                "function_ids": {"main": 0},
                "resource_drop_functions": {},
                "types": {},
                "variant_layouts": {
                    "State": {"name": "State", "variants": [
                        {"name": "Ready", "fields": []}
                    ]}
                },
                "native_signatures": {"main": {"params": [], "return_type": "State"}},
                "closure_identity_observable": false
            }))
            .expect("payload")
    };
    let artifact = |payload| {
        BytecodeArtifact::new(
            LANGUAGE_SEMANTICS_VERSION,
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .expect("artifact")
    };
    BytecodeVerifier::default()
        .verify(&artifact(payload("Ready")).to_bytes().unwrap())
        .expect("declared case is valid");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact(payload("Missing")).to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("undeclared variant `Missing`")
    ));
}

#[test]
fn unknown_optional_sections_are_forward_compatible() {
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .expect("artifact");
    let mut bytes = artifact.to_bytes().expect("bytes");
    bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
    append_test_section(&mut bytes, 63, 0, b"future metadata");

    BytecodeVerifier::default()
        .verify(&bytes)
        .expect("optional section should be ignored");
}

#[test]
fn unknown_required_sections_fail_closed() {
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .expect("artifact");
    let mut bytes = artifact.to_bytes().expect("bytes");
    bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
    append_test_section(&mut bytes, 63, SECTION_REQUIRED, b"future semantics");

    assert!(matches!(
        BytecodeVerifier::default().verify(&bytes),
        Err(BytecodeError::UnknownRequiredSection(63))
    ));
}

#[test]
fn malformed_binary_metadata_section_is_rejected() {
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        minimal_payload(),
    )
    .expect("artifact");
    let bytes = artifact.to_bytes().expect("bytes");
    let header_offset = BYTECODE_MAGIC.len() + 2;
    let data_offset = header_offset + SECTION_HEADER_BYTES;
    let data_length = u64::from_be_bytes(
        bytes[header_offset + 2..header_offset + 10]
            .try_into()
            .expect("section length"),
    ) as usize;
    let mut rewritten = Vec::new();
    rewritten.extend_from_slice(&bytes[..header_offset]);
    let mut header = Vec::with_capacity(data_length + 1);
    header.push(b' ');
    header.extend_from_slice(&bytes[data_offset..data_offset + data_length]);
    append_test_section(&mut rewritten, SECTION_HEADER, SECTION_REQUIRED, &header);
    rewritten.extend_from_slice(&bytes[data_offset + data_length..]);

    assert!(BytecodeVerifier::default().verify(&rewritten).is_err());
}

#[test]
fn verifier_rejects_unknown_instruction_with_a_valid_envelope() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "main",
            "params": 0,
            "captures": 0,
            "regs": 1,
            "local_regs": {},
            "code": [{"FutureOpcode": {"dst": 0}}]
        }],
        "function_ids": {"main": 0},
        "resource_drop_functions": {},
        "types": {},
        "native_signatures": {},
        "closure_identity_observable": false
    }))
    .expect("payload");
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");

    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("bytes")),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("unknown opcode")
    ));
}

#[test]
fn verifier_rejects_missing_or_unknown_instruction_fields() {
    for instruction in [
        serde_json::json!({"LoadUnit": {}}),
        serde_json::json!({"LoadUnit": {"dst": 0, "future": true}}),
    ] {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "main", "params": 0, "captures": 0, "regs": 1,
                "local_regs": {}, "code": [instruction.clone()]
            }],
            "function_ids": {"main": 0}, "resource_drop_functions": {},
            "types": {}, "native_signatures": {}, "closure_identity_observable": false
        }))
        .unwrap();
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .unwrap();
        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("fields differ")
        ));
    }
}

#[test]
fn every_known_opcode_has_an_exact_field_contract() {
    for opcode in KNOWN_OPCODES {
        assert!(
            !instruction_fields(opcode).is_empty(),
            "missing verifier field contract for {opcode}"
        );
    }
}

#[test]
fn verifier_rejects_inconsistent_type_and_function_metadata() {
    let artifact_for = |types: serde_json::Value, signatures: serde_json::Value| {
        BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {},
                    "code": [{"LoadUnit": {"dst": 0}}, {"Return": {"src": 0}}]
                }],
                "function_ids": {"main": 0}, "resource_drop_functions": {},
                "types": types, "native_signatures": signatures,
                "closure_identity_observable": false
            }))
            .unwrap(),
        )
        .unwrap()
    };

    let bad_type = artifact_for(
        serde_json::json!({"Expected": {"name": "Different", "fields": []}}),
        serde_json::json!({"main": {"params": [], "return_type": "Unit"}}),
    );
    assert!(matches!(
        BytecodeVerifier::default().verify(&bad_type.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("does not match metadata name")
    ));

    let bad_signature = artifact_for(
        serde_json::json!({}),
        serde_json::json!({"main": {"params": ["Int"], "return_type": "Unit"}}),
    );
    assert!(matches!(
        BytecodeVerifier::default().verify(&bad_signature.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("expected 0")
    ));
}

#[test]
fn resource_drop_fields_are_verified_implicit_inputs() {
    let artifact_for = |locals: serde_json::Value| {
        BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "<drop:File>", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": locals, "code": [{"Return": {"src": 0}}]
                }],
                "function_ids": {}, "resource_drop_functions": {"File": 0},
                "types": {"File": {"name": "File", "fields": [
                    {"name": "path", "type_name": "String"}
                ]}},
                "native_signatures": {}, "closure_identity_observable": false
            }))
            .unwrap(),
        )
        .unwrap()
    };

    BytecodeVerifier::default()
        .verify(
            &artifact_for(serde_json::json!({"path": 0}))
                .to_bytes()
                .unwrap(),
        )
        .expect("resource fields initialize drop registers");
    assert!(matches!(
        BytecodeVerifier::default().verify(
            &artifact_for(serde_json::json!({})).to_bytes().unwrap()
        ),
        Err(BytecodeError::InvalidPayload(message))
            if message.contains("missing field register `path`")
    ));
}

#[test]
fn explicit_resource_scope_markers_must_balance_before_return() {
    let artifact_for = |include_release: bool| {
        let mut code = vec![
            serde_json::json!({"LoadUnit": {"dst": 0}}),
            serde_json::json!({"ResourceAcquire": {"resource": 0}}),
        ];
        if include_release {
            code.push(serde_json::json!({"ResourceDrop": {"resource": 0}}));
        }
        code.push(serde_json::json!({"Return": {"src": 0}}));
        BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {}, "code": code
                }],
                "function_ids": {"main": 0}, "resource_drop_functions": {},
                "types": {},
                "native_signatures": {"main": {"params": [], "return_type": "Unit"}},
                "closure_identity_observable": false
            }))
            .expect("payload"),
        )
        .expect("artifact")
    };
    BytecodeVerifier::default()
        .verify(&artifact_for(true).to_bytes().unwrap())
        .expect("balanced explicit resource scope");
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact_for(false).to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("returns with live resource scopes")
    ));
}

#[test]
fn verifier_rejects_out_of_range_register_with_a_valid_envelope() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "main",
            "params": 0,
            "captures": 0,
            "regs": 1,
            "local_regs": {},
            "code": [{"LoadUnit": {"dst": 1}}]
        }],
        "function_ids": {"main": 0},
        "resource_drop_functions": {},
        "types": {},
        "native_signatures": {},
        "closure_identity_observable": false
    }))
    .expect("payload");
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .expect("artifact");

    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().expect("bytes")),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("register 1")
    ));
}

#[test]
fn verifier_rejects_uninitialized_register_reads() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "main", "params": 0, "captures": 0, "regs": 1,
            "local_regs": {}, "code": [{"Return": {"src": 0}}]
        }],
        "function_ids": {"main": 0}, "resource_drop_functions": {},
        "types": {}, "native_signatures": {}, "closure_identity_observable": false
    }))
    .unwrap();
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .unwrap();

    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message))
            if message.contains("reads uninitialized register 0")
    ));
}

#[test]
fn verifier_accepts_only_well_formed_optional_source_map_entries() {
    let mut payload = serde_json::json!({
        "functions": [{
            "name": "main", "params": 0, "captures": 0, "regs": 1,
            "local_regs": {}, "code": [
                {"LoadUnit": {"dst": 0}},
                {"Return": {"src": 0}}
            ]
        }],
        "function_ids": {"main": 0}, "resource_drop_functions": {},
        "types": {},
        "native_signatures": {"main": {"params": [], "return_type": "Unit"}},
        "closure_identity_observable": false,
        "source_map": [{
            "function": 0, "instruction": 0, "file": "main.rss",
            "line": 1, "column": 1, "length": 2
        }]
    });
    let artifact_for = |payload: &serde_json::Value| {
        BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            encode_executable_payload(payload).expect("source-map payload"),
        )
        .expect("source-map artifact")
    };
    BytecodeVerifier::default()
        .verify(&artifact_for(&payload).to_bytes().expect("source-map bytes"))
        .expect("well-formed source map verifies");

    payload["source_map"][0]["instruction"] = serde_json::json!(2);
    assert!(matches!(
        BytecodeVerifier::default()
            .verify(&artifact_for(&payload).to_bytes().expect("bad source-map bytes")),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("source_map entry 0 references missing instruction")
    ));
}

#[test]
fn verifier_intersects_register_state_at_control_flow_joins() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "main", "params": 0, "captures": 0, "regs": 2,
            "local_regs": {},
            "code": [
                {"LoadBool": {"dst": 0, "value": true}},
                {"JumpIfBool": {"cond": 0, "expected": true, "target": 3}},
                {"LoadInt": {"dst": 1, "value": 7}},
                {"Return": {"src": 1}}
            ]
        }],
        "function_ids": {"main": 0}, "resource_drop_functions": {},
        "types": {}, "native_signatures": {}, "closure_identity_observable": false
    }))
    .unwrap();
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .unwrap();

    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message))
            if message.contains("reads uninitialized register 1")
    ));
}

#[test]
fn verifier_counts_captures_and_parameters_in_the_input_window() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "closure", "params": 1, "captures": 1, "regs": 1,
            "local_regs": {}, "code": []
        }],
        "function_ids": {"closure": 0}, "resource_drop_functions": {},
        "types": {}, "native_signatures": {}, "closure_identity_observable": false
    }))
    .unwrap();
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .unwrap();

    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message))
            if message.contains("parameters and captures")
    ));
}

#[test]
fn verifier_rejects_import_whose_structural_signature_disagrees_with_hash() {
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "message".into(),
            effect: DataEffect::Read,
            ty: "String".into(),
            retained: false,
        }],
        result: "Unit".into(),
        asynchronous: false,
    };
    let wrong_hash = FunctionSignature {
        parameters: vec![],
        result: "Unit".into(),
        asynchronous: false,
    }
    .hash();
    let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![ExternalImport {
                symbol: ExternalSymbol::new("host.log.emit").unwrap(),
                signature,
                signature_hash: wrong_hash,
                abi_version: RUNTIME_ABI_VERSION,
            }],
            encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {},
                    "code": [{"CallExternal": {"dst": 0, "key": "host.log.emit", "args": [], "mut_args": []}}]
                }],
                "function_ids": {"main": 0},
                "resource_drop_functions": {}, "types": {}, "native_signatures": {},
                "closure_identity_observable": false
            }))
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
        Err(BytecodeError::ImportSignatureHashMismatch)
    ));
}

#[test]
fn verifier_rejects_static_call_arity_mismatch() {
    let payload = encode_executable_payload(&serde_json::json!({
        "functions": [
            {
                "name": "main", "params": 0, "captures": 0, "regs": 1,
                "local_regs": {},
                "code": [{"CallKnown": {"dst": 0, "function": 1, "args": [], "mut_args": []}}]
            },
            {
                "name": "callee", "params": 1, "captures": 0, "regs": 1,
                "local_regs": {}, "code": [{"Return": {"src": 0}}]
            }
        ],
        "function_ids": {"main": 0, "callee": 1}, "resource_drop_functions": {},
        "types": {}, "native_signatures": {}, "closure_identity_observable": false
    }))
    .unwrap();
    let artifact = BytecodeArtifact::new(
        "0.1.0",
        "0.1.0",
        TEST_CATALOG_DIGEST,
        RUNTIME_ABI_VERSION,
        TEST_SOURCE_DIGEST,
        vec![],
        payload,
    )
    .unwrap();

    assert!(matches!(
        BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("expected 1")
    ));
}

#[test]
fn verifier_rejects_external_argument_and_effect_mismatch() {
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "value".into(),
            effect: DataEffect::Mut,
            ty: "Int".into(),
            retained: false,
        }],
        result: "Unit".into(),
        asynchronous: false,
    };
    let symbol = ExternalSymbol::new("host.test.mutate").unwrap();
    let artifact_for = |args: serde_json::Value, mut_args: serde_json::Value| {
        BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![ExternalImport {
                symbol: symbol.clone(),
                signature: signature.clone(),
                signature_hash: signature.hash(),
                abi_version: RUNTIME_ABI_VERSION,
            }],
            encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 1, "captures": 0, "regs": 2,
                    "local_regs": {},
                    "code": [{"CallExternal": {
                        "dst": 1, "key": "host.test.mutate", "args": args, "mut_args": mut_args
                    }}]
                }],
                "function_ids": {"main": 0}, "resource_drop_functions": {},
                "types": {}, "native_signatures": {}, "closure_identity_observable": false
            }))
            .unwrap(),
        )
        .unwrap()
    };

    let missing_argument = artifact_for(serde_json::json!([]), serde_json::json!([]));
    assert!(matches!(
        BytecodeVerifier::default().verify(&missing_argument.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("expected 1")
    ));

    let missing_mut = artifact_for(serde_json::json!([0]), serde_json::json!([]));
    assert!(matches!(
        BytecodeVerifier::default().verify(&missing_mut.to_bytes().unwrap()),
        Err(BytecodeError::InvalidPayload(message)) if message.contains("mut_args differ")
    ));
}

fn append_test_section(bytes: &mut Vec<u8>, kind: u8, flags: u8, data: &[u8]) {
    bytes.push(kind);
    bytes.push(flags);
    bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&Sha256::digest(data));
    bytes.extend_from_slice(data);
}

fn minimal_payload() -> Vec<u8> {
    encode_executable_payload(&serde_json::json!({
        "functions": [],
        "function_ids": {},
        "resource_drop_functions": {},
        "types": {},
        "native_signatures": {},
        "closure_identity_observable": false
    }))
    .expect("minimal payload")
}

fn external_call_payload(symbol: &str) -> Vec<u8> {
    encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "main", "params": 0, "captures": 0, "regs": 1,
            "local_regs": {},
            "code": [
                {"CallExternal": {"dst": 0, "key": symbol, "args": [], "mut_args": []}},
                {"Return": {"src": 0}}
            ]
        }],
        "function_ids": {"main": 0},
        "resource_drop_functions": {},
        "types": {},
        "native_signatures": {"main": {"params": [], "return_type": "Unit"}},
        "closure_identity_observable": false
    }))
    .expect("external call payload")
}

fn scalar_return_payload() -> Vec<u8> {
    encode_executable_payload(&serde_json::json!({
        "functions": [{
            "name": "main", "params": 0, "captures": 0, "regs": 1,
            "local_regs": {},
            "code": [
                {"LoadInt": {"dst": 0, "value": 7}},
                {"Return": {"src": 0}}
            ]
        }],
        "function_ids": {"main": 0},
        "resource_drop_functions": {},
        "types": {},
        "native_signatures": {"main": {"params": [], "return_type": "Int"}},
        "closure_identity_observable": false
    }))
    .expect("scalar return payload")
}

fn known_call_payload() -> Vec<u8> {
    encode_executable_payload(&serde_json::json!({
        "functions": [
            {
                "name": "main", "params": 1, "captures": 0, "regs": 2,
                "local_regs": {},
                "code": [
                    {"CallKnown": {"dst": 1, "function": 1, "args": [0], "mut_args": []}},
                    {"Return": {"src": 1}}
                ]
            },
            {
                "name": "identity", "params": 1, "captures": 0, "regs": 1,
                "local_regs": {},
                "code": [{"Return": {"src": 0}}]
            }
        ],
        "function_ids": {"identity": 1, "main": 0},
        "resource_drop_functions": {},
        "types": {},
        "native_signatures": {
            "main": {"params": ["owned Int"], "return_type": "owned Int"},
            "identity": {"params": ["owned Int"], "return_type": "owned Int"}
        },
        "closure_identity_observable": false
    }))
    .expect("known call payload")
}

fn two_function_facts(artifact: &BytecodeArtifact) -> TypedExecutableFactsV1 {
    let scalar = TypedRegisterFactV1 {
        ty: TypedFactTypeV1::Known(WireType::Qualified {
            qualifier: rsscript_abi_model::WireQualifier::Owned,
            value: Box::new(WireType::Int {
                bits: 64,
                signed: true,
            }),
        }),
        ownership: TypedValueOwnershipV1::Owned,
    };
    TypedExecutableFactsV1 {
        schema: TYPED_EXECUTABLE_FACTS_SCHEMA_V1.to_owned(),
        executable_hash: artifact.header.executable_hash.clone(),
        bytecode_isa_version: artifact.header.bytecode_isa_version,
        runtime_abi_version: artifact.header.runtime_abi_version,
        interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
        imports_hash: typed_facts_imports_hash(artifact).expect("imports hash"),
        functions: vec![
            TypedFunctionFactsV1 {
                function_ordinal: 0,
                registers: vec![scalar.clone(), scalar.clone()],
                call_sites: vec![],
                generic_substitutions: vec![],
            },
            TypedFunctionFactsV1 {
                function_ordinal: 1,
                registers: vec![scalar],
                call_sites: vec![],
                generic_substitutions: vec![],
            },
        ],
        layouts: vec![],
    }
}

fn empty_function_facts(artifact: &BytecodeArtifact, registers: usize) -> TypedExecutableFactsV1 {
    TypedExecutableFactsV1 {
        schema: TYPED_EXECUTABLE_FACTS_SCHEMA_V1.to_owned(),
        executable_hash: artifact.header.executable_hash.clone(),
        bytecode_isa_version: artifact.header.bytecode_isa_version,
        runtime_abi_version: artifact.header.runtime_abi_version,
        interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
        imports_hash: typed_facts_imports_hash(artifact).expect("imports hash"),
        functions: vec![TypedFunctionFactsV1 {
            function_ordinal: 0,
            registers: vec![
                TypedRegisterFactV1 {
                    ty: TypedFactTypeV1::Unknown,
                    ownership: TypedValueOwnershipV1::Unknown,
                };
                registers
            ],
            call_sites: vec![],
            generic_substitutions: vec![],
        }],
        layouts: vec![],
    }
}

proptest! {
    #[test]
    fn arbitrary_bounded_input_is_rejected_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let verifier = BytecodeVerifier::new(BytecodeLimits {
            max_artifact_bytes: 2048,
            max_payload_bytes: 1024,
            max_imports: 32,
            max_functions: 32,
            max_registers_per_function: 256,
            max_instructions: 1024,
            max_typed_facts_bytes: 1024,
        });
        let _ = verifier.verify(&bytes);
    }
}
