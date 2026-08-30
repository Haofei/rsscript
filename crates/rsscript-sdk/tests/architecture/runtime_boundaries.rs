use std::collections::BTreeSet;

use crate::support::*;

#[test]
fn embedding_facade_exposes_only_product_level_objects() {
    let root = workspace_root();
    let mut source = sdk_source(&root);
    source.push_str(&read(&root.join("crates/rsscript-artifact/src/lib.rs")));
    for object in [
        "pub struct Compiler",
        "pub struct BuiltArtifact",
        "pub struct VerifiedArtifact",
        "pub struct AdmittedArtifact",
        "pub struct LinkedArtifact",
        "pub struct ArtifactBundle",
        "pub struct ArtifactVerifier",
        "pub struct ExecutionRequest",
        "pub struct Runtime",
        "pub struct ProviderRegistry",
        "pub struct RunLimits",
        "pub struct ExecutionReport",
    ] {
        assert!(
            source.contains(object),
            "missing stable façade object `{object}`"
        );
    }
    for forbidden in ["JitPlan", "RegInstr", "ReviewFinding", "reir"] {
        assert!(
            !source.contains(forbidden),
            "stable embedding façade must not expose `{forbidden}`"
        );
    }
}

#[test]
fn runtime_link_requires_explicit_host_artifact_admission() {
    let root = workspace_root();
    let sdk = sdk_source(&root);
    assert!(
        sdk.contains("artifact: &'artifact AdmittedArtifact"),
        "Runtime::link must only accept host-admitted Artifacts"
    );
    assert!(
        sdk.contains("pub trait ArtifactAdmissionPolicy"),
        "hosts must be able to define non-trusted artifact admission"
    );
    assert!(
        sdk.contains("pub fn admit_trusted_input(self) -> AdmittedArtifact"),
        "trusted input admission must remain explicit in the API name"
    );
}

#[test]
fn vm_core_consumes_owned_ir_not_frontend_internals() {
    let workspace = workspace_root();
    let root = workspace.join("crates/rsscript-vm/src/reg_vm");
    for relative in [
        "bytecode.rs",
        "calls.rs",
        "exec.rs",
        "model.rs",
        "scheduler.rs",
    ] {
        let source = read(&root.join(relative));
        for forbidden in [
            "crate::hir",
            "crate::syntax",
            "crate::semantic",
            "ValidatedProgram",
        ] {
            assert!(
                !source.contains(forbidden),
                "VM core `{relative}` must not consume frontend symbol `{forbidden}`; keep that dependency in compile.rs"
            );
        }
    }

    let vm_manifest: toml::Value =
        toml::from_str(&read(&workspace.join("crates/rsscript-vm/Cargo.toml"))).unwrap();
    let vm_dependencies = dependency_packages(&vm_manifest);
    for required in ["rsscript-bytecode", "rsscript-provider-api"] {
        assert!(
            vm_dependencies.contains(required),
            "VM must depend on `{required}`"
        );
    }
    for forbidden in [
        "rsscript",
        "rsscript-compiler",
        "rsscript-lowering",
        "rsscript-mir",
        "rsscript-semantics",
        "rsscript-syntax",
    ] {
        assert!(
            !vm_dependencies.contains(forbidden),
            "VM must not depend on frontend package `{forbidden}`"
        );
    }
}

#[test]
fn source_shaped_executable_ir_is_physically_deleted() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-vm/Cargo.toml")))
        .expect("VM manifest should parse");
    assert_eq!(
        manifest["dependencies"].get("rsscript-exec-ir"),
        None,
        "the execution-only VM must not retain the source-shaped IR dependency"
    );
    assert!(manifest["features"].get("legacy-exec-ir").is_none());

    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(!vm.contains("legacy-exec-ir") && !vm.contains("rsscript_exec_ir"));
    let bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    assert!(!bytecode.contains("encode_and_verify") && !bytecode.contains("verify_bytes("));

    assert!(!root.join("crates/rsscript-exec-ir").exists());
    assert!(!root.join("crates/rsscript-vm/src/reg_vm/lower.rs").exists());
}

#[test]
fn compiler_lowering_has_no_source_shaped_ir_compatibility_path() {
    let root = workspace_root();
    let lowering: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-lowering/Cargo.toml")))
            .expect("lowering manifest should parse");
    assert!(
        lowering["dependencies"].get("rsscript-exec-ir").is_none()
            && lowering
                .get("features")
                .is_none_or(|features| features.get("legacy-exec-ir").is_none()),
        "the lowering boundary must have no source-shaped compatibility closure"
    );

    let compiler: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert!(compiler["features"].get("legacy-exec-ir").is_none());
}

#[test]
fn vm_public_loader_requires_a_verifier_token() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("pub fn from_verified_bytecode("));
    for forbidden in [
        "pub fn from_bytecode(",
        "pub fn from_bytecode_with_operation(",
    ] {
        assert!(
            !vm.contains(forbidden),
            "VM must not expose raw-byte constructor `{forbidden}`"
        );
    }
}

#[test]
fn bytecode_verifier_is_the_only_payload_validation_owner() {
    let root = workspace_root();
    let vm_bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    for duplicate in [
        "fn verify_payload(",
        "fn verify_wire_unit(",
        "fn verify_instruction(",
        "fn verify_register_field(",
    ] {
        assert!(
            !vm_bytecode.contains(duplicate),
            "VM must not restore duplicate bytecode validation `{duplicate}`"
        );
    }
    assert!(vm_bytecode.contains("VerifiedBytecode"));
    assert!(vm_bytecode.contains("decode_executable_payload"));
}

#[test]
fn compiler_default_dependency_closure_is_host_neutral() {
    let root = workspace_root();
    let facade: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-sdk/Cargo.toml")))
        .expect("embedding compiler manifest should parse");
    assert!(
        facade["features"]["default"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the compiler facade must be frontend-only unless execution is explicitly enabled"
    );
    assert_eq!(
        facade["dependencies"]["rsscript_compiler"]["default-features"].as_bool(),
        Some(false)
    );
    assert_eq!(
        facade["dependencies"]["rsscript-provider-api"]["optional"].as_bool(),
        Some(true)
    );
    assert_eq!(
        facade["package"]["publish"].as_bool(),
        Some(false),
        "the alpha SDK must not advertise a broken crates.io package graph"
    );

    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert_eq!(
        manifest["package"]["name"].as_str(),
        Some("rsscript-compiler")
    );
    assert_eq!(manifest["package"]["publish"].as_bool(), Some(false));
    for forbidden in ["rsscript-runtime", "rsscript-aot-runtime"] {
        assert!(
            manifest["dependencies"].get(forbidden).is_none(),
            "compiler/VM core must not depend on generated-Rust runtime `{forbidden}`"
        );
    }
    assert!(
        manifest["dependencies"].get("rss-native-abi").is_none(),
        "compiler must not depend on the native plugin ABI"
    );
    assert!(
        manifest["dependencies"].get("rss-process-guard").is_none(),
        "compiler must not own child-process execution"
    );
    assert!(
        manifest["dependencies"]
            .get("rsscript-jit-cranelift")
            .is_none()
    );
    assert!(
        manifest["dependencies"].get("rsscript-vm").is_none(),
        "self-host execution belongs to the independent experiments workspace"
    );

    assert!(
        manifest["features"].get("package").is_none(),
        "the pure compiler must not retain a package/persistence compatibility feature"
    );
    let lowering = manifest["features"]["lowering"]
        .as_array()
        .expect("compiler lowering feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(manifest["features"].get("execution").is_none());
    assert!(manifest["features"].get("legacy-exec-ir").is_none());
    assert!(
        manifest["features"].get("selfhost-parity").is_none(),
        "compiler must not expose a research self-host feature"
    );
    let bytecode = manifest["features"]["bytecode"]
        .as_array()
        .expect("compiler bytecode feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(bytecode.contains("lowering"));
    for dependency in ["rsscript-bytecode", "rsscript-codegen-vm"] {
        let feature = format!("dep:{dependency}");
        assert!(
            bytecode.contains(feature.as_str()),
            "bytecode feature must explicitly select `{dependency}`"
        );
    }
    for dependency in ["rsscript-lowering", "rsscript-mir", "sha2"] {
        let feature = format!("dep:{dependency}");
        assert!(
            lowering.contains(feature.as_str()),
            "lowering feature must explicitly select `{dependency}`"
        );
    }
    for dependency in ["fs2", "hex", "libc", "rustix", "tempfile", "toml", "uuid"] {
        assert!(
            manifest["dependencies"].get(dependency).is_none(),
            "pure compiler must not retain package/persistence dependency `{dependency}`"
        );
    }
    let sdk_execution = facade["features"]["execution"]
        .as_array()
        .expect("SDK execution feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(sdk_execution.contains("rsscript_compiler/bytecode"));
    for removed in [
        "dep:rsscript-codegen-vm",
        "dep:rsscript-lowering",
        "dep:rsscript-mir",
    ] {
        assert!(
            !sdk_execution.contains(removed),
            "reviewed SDK execution must use compiler bytecode rather than `{removed}`"
        );
    }
    assert!(
        !sdk_execution.contains("rsscript_compiler/package"),
        "reviewed in-memory SDK execution must not select compiler package capture"
    );
    let sdk_project = facade["features"]["project"]
        .as_array()
        .expect("SDK project feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(
        !sdk_project.contains("rsscript_compiler/package"),
        "reviewed project capture must remain a loader-to-in-memory-compiler path"
    );
    assert!(
        sdk_project.contains("dep:rsscript-project"),
        "project capture must select the dedicated project/input boundary rather than widening normal execution"
    );
    for removed in [
        "base64",
        "chrono",
        "flate2",
        "hmac",
        "percent-encoding",
        "rand",
        "regex",
        "semver",
        "serde_yaml_ng",
        "sha3",
        "toml_edit",
    ] {
        assert!(
            manifest["dependencies"].get(removed).is_none(),
            "unused compiler package dependency `{removed}` must not widen the frontend closure"
        );
    }
}

#[test]
fn concrete_host_providers_are_leaf_composition_packages() {
    let root = workspace_root();
    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml"))).unwrap();
    let compiler_dependencies = normal_dependency_packages(&compiler_manifest);
    let providers = [
        "fs", "env", "process", "http", "time", "entropy", "log", "cli",
    ];
    for provider in providers {
        let manifest_path = root.join("providers").join(provider).join("Cargo.toml");
        let manifest: toml::Value = toml::from_str(&read(&manifest_path)).unwrap();
        let package = package_name(&manifest);
        let dependencies = normal_dependency_packages(&manifest);
        assert!(dependencies.contains("rsscript-provider-api"));
        for forbidden in [
            "rsscript",
            "rsscript-runtime",
            "rsscript-aot-runtime",
            "rsscript-semantics",
            "reir",
            "rsscript-jit-cranelift",
        ] {
            assert!(
                !dependencies.contains(forbidden),
                "provider `{package}` must not depend on `{forbidden}`"
            );
        }
        assert!(
            !compiler_dependencies.contains(package),
            "compiler must not select concrete provider `{package}`"
        );
    }
}

#[test]
fn provider_contracts_can_be_generated_without_the_engine_or_runtime() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-provider-bindgen/Cargo.toml"),
    ))
    .expect("Provider bindgen manifest should parse");
    let dependencies = dependency_packages(&manifest);
    assert!(dependencies.contains("rsscript-abi-model"));
    assert!(dependencies.contains("rsscript-semantics"));
    for forbidden in [
        "rsscript-compiler",
        "rsscript-sdk",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rsscript-provider-api",
        "rsscript-jit-cranelift",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "Provider bindgen must not depend on `{forbidden}`"
        );
    }

    for provider in [
        "cli", "entropy", "env", "fs", "http", "log", "process", "time",
    ] {
        let source = read(&root.join(format!("providers/{provider}/src/lib.rs")));
        assert!(
            source.contains("provider_contract.rs"),
            "Provider `{provider}` must include its generated contract"
        );
        assert!(
            !source.contains("FunctionSignature"),
            "Provider `{provider}` must not hand-author ABI signatures"
        );
        assert!(
            root.join(format!("providers/{provider}/interface/lib.rssi"))
                .is_file(),
            "Provider `{provider}` must own a canonical .rssi interface"
        );
        let provider_manifest: toml::Value = toml::from_str(&read(
            &root.join(format!("providers/{provider}/Cargo.toml")),
        ))
        .unwrap();
        assert_eq!(
            provider_manifest["build-dependencies"]["rsscript-provider-bindgen"]["path"].as_str(),
            Some("../../crates/rsscript-provider-bindgen")
        );
    }
}

#[test]
fn provider_bindgen_consumes_semantic_descriptors_not_syntax() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-provider-bindgen/Cargo.toml"),
    ))
    .expect("provider bindgen manifest should parse");
    let dependencies = dependency_packages(&manifest);
    assert!(dependencies.contains("rsscript-semantics"));
    assert!(!dependencies.contains("rsscript-syntax"));
    let source = read(&root.join("crates/rsscript-provider-bindgen/src/lib.rs"));
    assert!(source.contains("from_descriptor"));
    assert!(!source.contains("parse_source("));
}

#[test]
fn interface_catalog_is_platform_neutral() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-interface-catalog/Cargo.toml"),
    ))
    .expect("interface catalog manifest should parse");
    let dependencies = dependency_packages(&manifest);
    assert!(
        dependencies.is_empty(),
        "the interface catalog must remain data-only"
    );

    let catalog = read(&root.join("crates/rsscript-interface-catalog/src/lib.rs"));
    for forbidden in ["host/", "provider", "policy", "capability"] {
        assert!(
            !catalog.to_ascii_lowercase().contains(forbidden),
            "interface catalog must not contain `{forbidden}`"
        );
    }

    for removed in [
        "stdlib/clock/clock.rssi",
        "stdlib/env/env.rssi",
        "stdlib/fs/directory.rssi",
        "stdlib/fs/file.rssi",
        "stdlib/http/client.rssi",
        "stdlib/process/process.rssi",
        "stdlib/random/random.rssi",
        "stdlib/tempdir/tempdir.rssi",
        "stdlib/workspace/workspace.rssi",
        "packages/async/interface/file.rssi",
        "packages/async/interface/http.rssi",
        "packages/async/interface/process.rssi",
        "packages/async/interface/timer.rssi",
    ] {
        assert!(
            !root.join(removed).exists(),
            "legacy host façade must not return at `{removed}`"
        );
    }
}

#[test]
fn abi_and_provider_crates_keep_one_way_dependencies() {
    let root = workspace_root();
    let abi_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-abi-model/Cargo.toml")))
            .expect("ABI model manifest should parse");
    let provider_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-provider-api/Cargo.toml")))
            .expect("provider API manifest should parse");
    let abi_dependencies = dependency_packages(&abi_manifest);
    let provider_dependencies = dependency_packages(&provider_manifest);

    for forbidden in [
        "rsscript",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rss-native-abi",
        "rss-process-guard",
        "reir",
        "rsscript-jit-cranelift",
    ] {
        assert!(
            !abi_dependencies.contains(forbidden),
            "ABI model must not depend on `{forbidden}`"
        );
        assert!(
            !provider_dependencies.contains(forbidden),
            "provider API must not depend on `{forbidden}`"
        );
    }
    assert!(
        provider_dependencies.contains("rsscript-abi-model"),
        "provider API must consume the shared ABI model"
    );
    let provider_source = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    assert!(
        !provider_source.contains("pub enum NativeValue")
            && !provider_source.contains("pub struct NativeInterpreterFn"),
        "the canonical Provider contract must not retain the retired dynamic compatibility API"
    );
    assert!(
        provider_source.contains("pub details: Option<WireValue>"),
        "canonical Provider errors must not reopen a serde_json ABI escape hatch"
    );
    assert!(
        !provider_source.contains("pub details: Option<serde_json::Value>"),
        "Provider error details must remain structural wire values"
    );
    let provider_manifest = read(&root.join("crates/rsscript-provider-api/Cargo.toml"));
    assert!(
        !provider_manifest.contains("compatibility = []"),
        "the retired Provider compatibility feature must remain deleted"
    );
    let vm_manifest = read(&root.join("crates/rsscript-vm/Cargo.toml"));
    assert!(
        !vm_manifest.contains("features = [\"compatibility\"]"),
        "the register VM must use only the canonical Provider wire API"
    );
}

#[test]
fn official_providers_use_canonical_wire_callables() {
    let root = workspace_root();
    for provider in [
        "cli", "entropy", "env", "fs", "http", "log", "process", "time",
    ] {
        let source = read(&root.join(format!("providers/{provider}/src/lib.rs")));
        assert!(
            source.contains("WireInterpreterFn"),
            "official provider `{provider}` must use the canonical wire callable"
        );
        assert!(
            !source.contains("NativeInterpreterFn") && !source.contains("NativeValue"),
            "official provider `{provider}` must not regress to the legacy dynamic value boundary"
        );
    }
}

#[test]
fn artifact_verifier_owns_instruction_validation() {
    let root = workspace_root();
    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-bytecode/Cargo.toml")))
            .expect("bytecode manifest should parse");
    let dependencies = dependency_packages(&manifest);
    for forbidden in [
        "rsscript-sdk",
        "rsscript-compiler",
        "rsscript-semantics",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rsscript-jit-cranelift",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "artifact verifier must not depend on `{forbidden}`"
        );
    }
    let verifier = read(&root.join("crates/rsscript-bytecode/src/lib.rs"));
    for invariant in [
        "verify_executable_payload",
        "max_functions",
        "max_registers_per_function",
        "max_instructions",
        "unknown opcode",
        "external call table mismatch",
    ] {
        assert!(
            verifier.contains(invariant),
            "artifact verifier must enforce `{invariant}`"
        );
    }
}

#[test]
fn sdk_passes_verified_bytecode_to_the_vm_loader() {
    let root = workspace_root();
    let sdk = sdk_source(&root);
    assert!(sdk.contains("BytecodeVerifier::default()"));
    assert!(sdk.contains("RegVmExecutable::from_verified_bytecode"));
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("pub fn from_verified_bytecode"));
}

#[test]
fn bytecode_language_compatibility_is_not_inferred_from_compiler_version() {
    let root = workspace_root();
    let verifier = read(&root.join("crates/rsscript-bytecode/src/lib.rs"));
    let compatibility = verifier
        .split("impl Default for BytecodeCompatibility")
        .nth(1)
        .and_then(|source| source.split("impl BytecodeVerifier").next())
        .expect("bytecode compatibility default");
    assert!(compatibility.contains("SUPPORTED_LANGUAGE_SEMANTICS"));
    assert!(
        !compatibility.contains("CARGO_PKG_VERSION"),
        "language compatibility must not be derived from compiler provenance"
    );

    let emitter = read(&root.join("crates/rsscript-codegen-vm/src/lib.rs"));
    assert!(emitter.contains("LANGUAGE_SEMANTICS_VERSION"));
    assert!(emitter.contains("compiler_provenance"));
    assert!(verifier.contains("BYTECODE_CONTAINER_FORMAT_VERSION"));

    let analysis = read(&root.join("crates/rsscript-package-review/src/analysis.rs"));
    assert!(analysis.contains("rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION"));
    assert!(
        !analysis.contains("language_version: env!(\"CARGO_PKG_VERSION\")"),
        "neutral analysis must carry language semantics rather than compiler provenance"
    );
}

#[test]
fn typed_mir_has_a_frontend_free_dependency_boundary() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-mir/Cargo.toml")))
        .expect("MIR manifest should parse");
    let dependencies = dependency_packages(&manifest);

    for forbidden in [
        "rsscript",
        "rsscript-compiler",
        "rsscript-syntax",
        "rsscript-semantics",
        "rsscript-lowering",
        "rsscript-vm",
        "rsscript-provider-api",
        "rsscript-runtime",
        "reir",
        "rsscript-jit-cranelift",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "rsscript-mir must not depend on {forbidden}"
        );
    }

    let mir = read(&root.join("crates/rsscript-mir/src/lib.rs"));
    for required in [
        "mir_id!(FunctionId)",
        "mir_id!(BlockId)",
        "mir_id!(ValueId)",
        "mir_id!(PlaceId)",
        "pub struct BasicBlock",
        "pub enum MirInstruction",
        "pub enum MirTerminator",
        "BorrowRead",
        "pub enum MirCallArgument",
        "pub enum MirParameterMode",
        "pub enum MirCallTarget",
        "mir_id!(BuiltinId)",
        "mir_id!(ExternalSymbolId)",
        "pub struct MirClosureCapture",
        "MakeClosure",
        "CallClosure",
        "pub struct MirFunctionSignature",
        "pub struct MirModule",
        "pub fn verify",
    ] {
        assert!(mir.contains(required), "MIR is missing {required}");
    }
    for forbidden in [
        "rsscript_syntax",
        "rsscript_semantics",
        "Unknown",
        "Executable",
    ] {
        assert!(
            !mir.contains(forbidden),
            "MIR must not expose source-shaped escape hatch {forbidden}"
        );
    }
}

#[test]
fn compiler_and_vm_do_not_embed_execution_authority() {
    let root = workspace_root();
    let vm_model = read(&root.join("crates/rsscript-vm/src/reg_vm/model.rs"));
    assert!(
        !vm_model.contains("host_authority"),
        "VM instructions must not carry runner authority policy"
    );

    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(
        !vm.contains("execution_context"),
        "VM core must not own an execution policy context"
    );
    let intrinsics = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/mod.rs"));
    assert!(
        !intrinsics.contains("authorize_intrinsic_host_access"),
        "intrinsic dispatch must be independent of runner policy"
    );
    assert!(
        !root
            .join("crates/rsscript-vm/src/reg_vm/host_adapters.rs")
            .exists()
    );
}

#[test]
fn provider_contract_uses_a_neutral_host_call_context() {
    let root = workspace_root();
    let provider_api = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    assert!(provider_api.contains("pub struct HostCallContext"));
    assert!(provider_api.contains("pub host_context: &'a HostCallContext"));
    assert!(
        !provider_api.contains("ProviderAuthority"),
        "Core Provider ABI must not restore policy-shaped authority types"
    );
}

#[test]
fn vm_core_does_not_embed_filesystem_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    for forbidden in [
        "Directory",
        "FileError",
        "HashSha256File",
        "JsonParseFile",
        "PathReadString",
        "PathWriteString",
        "TempDir",
        "TomlParseFile",
        "YamlParseFile",
    ] {
        assert!(
            !catalog.contains(forbidden),
            "filesystem operation `{forbidden}` must be supplied by an external provider"
        );
    }

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        assert!(
            !source.contains("std::fs"),
            "VM core must not access the filesystem directly: {}",
            path.display()
        );
    }
}

#[test]
fn vm_core_does_not_embed_process_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    assert!(!catalog.contains("{ id = \"Process"));
    assert!(!catalog.contains("{ namespace = \"Process\""));

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in ["std::process::Command", "RegIntrinsic::Process"] {
            assert!(
                !source.contains(forbidden),
                "VM core must not execute child processes directly: {} contains `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn vm_core_does_not_embed_network_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    for prefix in ["Http", "Tcp", "WebSocket"] {
        assert!(!catalog.contains(&format!("{{ id = \"{prefix}")));
        assert!(!catalog.contains(&format!("{{ namespace = \"{prefix}")));
    }

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in [
            "std::net",
            "TcpStream",
            "RegIntrinsic::Http",
            "RegIntrinsic::WebSocket",
        ] {
            assert!(
                !source.contains(forbidden),
                "VM core must not access the network directly: {} contains `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn vm_core_does_not_embed_time_logging_or_os_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    for prefix in ["Deadline", "InstantElapsed", "Log", "OsClose", "Timer"] {
        assert!(
            !catalog.contains(&format!("{{ id = \"{prefix}")),
            "host operation `{prefix}` must be supplied by an external provider"
        );
    }
    for namespace in ["Deadline", "Instant", "Log", "OS", "Timer"] {
        assert!(!catalog.contains(&format!("{{ namespace = \"{namespace}\"")));
    }

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in [
            "SystemTime",
            "std::thread::sleep",
            "RegIntrinsic::Log",
            "RegIntrinsic::Timer",
        ] {
            assert!(
                !source.contains(forbidden),
                "VM core must not read ambient time or emit host logs: {} contains `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn rust_aot_lowering_does_not_restore_removed_host_abi_types() {
    let root = workspace_root();
    let lowering_root = root.join("experiments/aot-backend/src/rust_lower");
    let forbidden = [
        "rsscript_runtime::File",
        "rsscript_runtime::Http",
        "rsscript_runtime::Process",
        "rsscript_runtime::RssTcp",
        "rsscript_runtime::RssWebSocket",
        "rsscript_runtime::TempDir",
        "rsscript_runtime::RssInstant",
        "rsscript_runtime::RssDeadline",
        "runtime_struct_constructor",
        "is_file_open_expr",
    ];
    for path in rust_files_below(&lowering_root) {
        let source = read(&path);
        for symbol in forbidden {
            assert!(
                !source.contains(symbol),
                "experimental AOT lowering must not restore host ABI `{symbol}` in {}",
                path.display()
            );
        }
    }
}

#[test]
fn generated_aot_abi_does_not_expose_wall_clock_or_timer_services() {
    let root = workspace_root();
    let runtime = read(&root.join("experiments/aot-runtime/src/lib.rs"));
    let abi_macro = runtime
        .split("macro_rules! runtime_abi_exports")
        .nth(1)
        .and_then(|source| source.split("/// Exact compatibility surface").next())
        .expect("generated AOT ABI macro");
    for forbidden in [
        "RssInstant",
        "clock_now",
        "clock_system_unix_ms",
        "instant_elapsed",
        "RssDeadline",
        "deadline_after",
        "TimerError",
        "TimerSleepPending",
        "timer_sleep",
        "OperationContext",
    ] {
        assert!(
            !abi_macro.contains(forbidden),
            "generated AOT ABI must obtain host time through a provider: `{forbidden}`"
        );
    }
    assert!(
        runtime.contains("pub mod host"),
        "execution deadlines remain explicit host controls"
    );
}

#[test]
fn program_arguments_enter_through_the_explicit_main_abi() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    assert!(!catalog.contains("{ id = \"Args"));
    assert!(!catalog.contains("{ namespace = \"Args\""));

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in ["std::env::args", "RegIntrinsic::Args"] {
            assert!(
                !source.contains(forbidden),
                "VM core must not read ambient program arguments: {} contains `{forbidden}`",
                path.display()
            );
        }
    }

    let scheduler = read(&root.join("crates/rsscript-vm/src/reg_vm/scheduler.rs"));
    assert!(scheduler.contains("List<String>"));
    assert!(scheduler.contains("self.entry_args"));
}

#[test]
fn high_risk_state_machines_keep_dedicated_module_owners() {
    let root = workspace_root();
    let required = [
        "crates/rsscript-semantics/src/task_groups.rs",
        "crates/rsscript-package-review/src/bindings.rs",
        "crates/rsscript-vm/src/reg_vm/tier/admission.rs",
        "crates/rsscript-vm/src/reg_vm/tier/call_scratch.rs",
        "crates/rsscript-vm/src/reg_vm/tier/recursion.rs",
        "experiments/aot-backend/src/rust_lower/helpers/executable_declarations.rs",
        "experiments/aot-backend/src/rust_lower/helpers/semantic_projection.rs",
        "experiments/aot-runtime/src/json.rs",
        "crates/rsscript-jit-cranelift/src/analysis.rs",
        "crates/rsscript-jit-cranelift/src/executable_memory.rs",
        "experiments/reir/src/reconciliation/engine.rs",
        "experiments/reir/src/cli/safe_io.rs",
    ];
    let missing = required
        .iter()
        .filter(|relative| !root.join(relative).is_file())
        .copied()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "refactoring module owners must remain explicit: {}",
        missing.join(", ")
    );
}

#[test]
fn selfhost_frontend_does_not_restore_retired_language_contracts() {
    let root = workspace_root();
    let checker = read(&root.join("experiments/fixtures/selfhost/check.rss"));
    let syntax_declarations = read(
        &root.join("experiments/fixtures/selfhost/checker/diagnostics/syntax_declarations.rss"),
    );
    for retired_code in [
        "RS0004", "RS0006", "RS0009", "RS0010", "RS0011", "RS0012", "RS0014", "RS0016", "RS0017",
        "RS0018", "RS0019", "RS0020", "RS0101",
    ] {
        assert!(
            !checker.contains(retired_code),
            "self-hosted checker must not emit retired diagnostic `{retired_code}`"
        );
    }

    let scanner = read(&root.join("experiments/fixtures/selfhost/scan.rss"));
    for retired_mapping in [
        "word == \"features\"",
        "word == \"profile\"",
        "word == \"native\"",
        "word == \"effects\"",
        "word == \"unsafe\"",
    ] {
        assert!(
            !scanner.contains(retired_mapping),
            "self-hosted scanner must not restore retired keyword mapping `{retired_mapping}`"
        );
    }

    for retired_feature_check in [
        "RS0101 FEATURE_VIOLATION",
        "collect_feature_use_tokens",
        "file_local_use",
        "file_async_use",
        "file_unsafe_use",
    ] {
        assert!(
            !syntax_declarations.contains(retired_feature_check),
            "self-hosted diagnostics must not retain retired feature check `{retired_feature_check}`"
        );
    }
}
