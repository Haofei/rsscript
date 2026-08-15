//! Checked-in source → semantic → MIR regression corpus.
//!
//! The migration differential proves two execution paths agree today. This
//! corpus protects the preceding boundaries independently, so a diagnostic,
//! checked HIR, or owned MIR change is deliberate and reviewable rather than
//! hidden behind a later bytecode result change.

use rsscript_compiler::{
    compile_validated_to_ir, format_diagnostics_json, standard_package_interfaces,
    validate_sources_with_interfaces,
};
use rsscript_mir::{MirModule, TypeId};
use rsscript_semantics::hir::{CallResolution, FunctionSig, Hir};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, serde::Deserialize)]
struct GoldenCase {
    kind: String,
    diagnostics_sha256: String,
    hir_sha256: Option<String>,
    mir_sha256: Option<String>,
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/semantic_mir")
}

fn digest(value: impl AsRef<[u8]>) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_ref()))
}

/// `Hir` deliberately owns hash maps for lookup. A `Debug` dump of those maps
/// is not a golden format because hash iteration order changes between runs.
/// Keep this projection ordered and semantic: lookup aliases, structural types,
/// resolved calls, ownership effects, and return proofs all participate, while
/// private cache layout does not.
fn canonical_hir(hir: &Hir) -> String {
    let mut lines = Vec::new();

    let mut signatures = hir.signatures().collect::<Vec<_>>();
    signatures.sort_by_key(|(key, _)| *key);
    for (key, signature) in signatures {
        lines.push(format!(
            "signature {key} {}",
            canonical_signature(signature)
        ));
    }

    let mut types = hir.types().collect::<Vec<_>>();
    types.sort_by_key(|ty| ty.name.as_str());
    for ty in types {
        let fields = ty
            .fields_ordered
            .iter()
            .map(|field| {
                format!(
                    "{}:{:?}:handle={}:weak={}",
                    field.name, field.ty, field.is_handle, field.is_weak
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        lines.push(format!("type {} {:?} [{fields}]", ty.name, ty.kind));
    }

    let mut bodies = hir.function_bodies().collect::<Vec<_>>();
    bodies.sort_by_key(|(name, _)| *name);
    for (name, body) in bodies {
        lines.push(format!("body {name}"));
        for binding in &body.bindings {
            lines.push(format!(
                " binding {} {:?} {:?} {:?}",
                binding.name, binding.kind, binding.effect, binding.ty
            ));
        }
        for call in &body.call_sites {
            lines.push(format!(
                " call {:?} {}",
                call.callee,
                canonical_call_resolution(&call.resolution)
            ));
        }
        for event in &body.effect_events {
            lines.push(format!(" effect {} {:?}", event.binding_name, event.kind));
        }
        for returned in &body.returns {
            lines.push(format!(" return {:?}", returned.proof));
        }
    }
    lines.join("\n")
}

fn canonical_signature(signature: &FunctionSig) -> String {
    let parameters = signature
        .params
        .iter()
        .map(|parameter| {
            format!(
                "{}:{:?}:{:?}",
                parameter.name, parameter.effect, parameter.ty
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut retained = signature
        .retained_params
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    retained.sort_unstable();
    format!(
        "namespace={:?} name={} public={} async={} params=[{parameters}] return={:?} fresh={} retained=[{}] builtin={} external={}",
        signature.namespace,
        signature.name,
        signature.is_public,
        signature.is_async,
        signature.return_ty,
        signature.returns_fresh,
        retained.join(","),
        signature.is_builtin,
        signature.is_external,
    )
}

fn canonical_call_resolution(resolution: &CallResolution) -> String {
    match resolution {
        CallResolution::Resolved { signature, kind } => {
            format!("resolved:{kind:?}:{}", canonical_signature(signature))
        }
        CallResolution::EnumVariant => "enum_variant".to_string(),
        CallResolution::Ambiguous { candidates } => {
            let mut candidates = candidates.clone();
            candidates.sort();
            format!("ambiguous:{}", candidates.join(","))
        }
        CallResolution::Unknown => "unknown".to_string(),
    }
}

/// MIR's type arena assigns numeric IDs while interning the complete standard
/// interface set. That insertion order is intentionally not part of program
/// meaning. Project each referenced ID back to its structural wire type and
/// sort the unreferenced type inventory so this golden checks executable
/// structure rather than hash-map iteration order.
fn canonical_mir(module: &MirModule) -> String {
    let type_name = |id: TypeId| {
        format!(
            "{:?}",
            module.ty(id).expect("verified MIR type id resolves")
        )
    };
    let mut lines = module
        .types()
        .iter()
        .map(|ty| format!("type {ty:?}"))
        .collect::<Vec<_>>();
    lines.sort();

    for function in module.functions() {
        let signature = function.signature();
        let parameters = signature
            .parameter_types()
            .iter()
            .zip(signature.parameter_modes())
            .map(|(ty, mode)| format!("{}:{mode:?}", type_name(*ty)))
            .collect::<Vec<_>>()
            .join(",");
        let captures = function
            .captures()
            .iter()
            .map(|capture| format!("{}:{:?}", type_name(capture.ty()), capture.mode()))
            .collect::<Vec<_>>()
            .join(",");
        let debug = module
            .function_debug(function.id())
            .expect("verified MIR has function debug metadata");
        lines.push(format!(
            "function {} id={} params=[{parameters}] result={} async={} captures=[{captures}] places={} values={}",
            debug.name(),
            function.id().index(),
            type_name(signature.result()),
            signature.is_async(),
            function.place_count(),
            function.value_count(),
        ));
        if let Some(source) = debug.source() {
            lines.push(format!(
                " source {}:{}:{}:{}",
                source.file(),
                source.line(),
                source.column(),
                source.length()
            ));
        }
        for instruction_source in debug.instruction_sources() {
            let source = instruction_source.source();
            lines.push(format!(
                " instruction-source {}:{} {}:{}:{}:{}",
                instruction_source.block().index(),
                instruction_source.instruction_index(),
                source.file(),
                source.line(),
                source.column(),
                source.length(),
            ));
        }
        for block in function.blocks() {
            lines.push(format!(" block {}", block.id().index()));
            for instruction in block.instructions() {
                lines.push(format!("  {instruction:?}"));
            }
            lines.push(format!("  terminator {:?}", block.terminator()));
        }
    }
    lines.join("\n")
}

fn checked_cases() -> Vec<(PathBuf, GoldenCase)> {
    let mut cases = fs::read_dir(corpus_root())
        .expect("read semantic/MIR corpus")
        .map(|entry| entry.expect("read semantic/MIR corpus entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .map(|path| {
            let contents = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let case = toml::from_str(&contents)
                .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
            (path, case)
        })
        .collect::<Vec<_>>();
    cases.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(!cases.is_empty(), "semantic/MIR corpus must not be empty");
    cases
}

#[test]
fn source_semantic_and_mir_goldens_remain_stable() {
    for (sidecar, expected) in checked_cases() {
        let source_path = sidecar.with_extension("rss");
        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", source_path.display()));
        let logical_path = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("fixture file name");
        let result = validate_sources_with_interfaces(
            &[(logical_path, source.as_str())],
            standard_package_interfaces(),
        );

        match expected.kind.as_str() {
            "valid" => {
                let validated = result.unwrap_or_else(|diagnostics| {
                    panic!(
                        "{} should validate: {diagnostics:#?}",
                        source_path.display()
                    )
                });
                let diagnostic_hash = digest(format_diagnostics_json(validated.diagnostics()));
                let hir_hash = digest(canonical_hir(validated.database().hir()));
                let compiled = compile_validated_to_ir(&validated);
                let mir = compiled.checked_hir_mir().unwrap_or_else(|error| {
                    panic!("{} should lower to MIR: {error}", source_path.display())
                });
                let mir_hash = digest(canonical_mir(mir.module()));

                assert_eq!(
                    diagnostic_hash,
                    expected.diagnostics_sha256,
                    "{} diagnostic golden changed",
                    source_path.display()
                );
                assert_eq!(
                    Some(hir_hash),
                    expected.hir_sha256,
                    "{} checked-HIR golden changed",
                    source_path.display()
                );
                assert_eq!(
                    Some(mir_hash),
                    expected.mir_sha256,
                    "{} MIR golden changed",
                    source_path.display()
                );
            }
            "diagnostics" => {
                let diagnostics = result.expect_err("diagnostic fixture must fail validation");
                assert_eq!(
                    digest(format_diagnostics_json(&diagnostics)),
                    expected.diagnostics_sha256,
                    "{} diagnostic golden changed",
                    source_path.display()
                );
                assert!(expected.hir_sha256.is_none());
                assert!(expected.mir_sha256.is_none());
            }
            other => panic!("{} has unknown corpus kind `{other}`", sidecar.display()),
        }
    }
}
