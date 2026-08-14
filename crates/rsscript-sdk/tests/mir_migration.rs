//! Reusable migration gate for the typed MIR rollout.
//!
//! Add a capability by declaring its stage and a small source case. A
//! `DualPath` case must lower directly from checked HIR to verified MIR and
//! produce the same result in the legacy VM, the feature-gated MIR reference
//! interpreter, and the verified-bytecode VM emitted directly from MIR.

use rsscript_abi_model::WireType;
use rsscript_compiler::{
    compile_source_to_ir, compile_validated_to_ir, validate_sources_with_interfaces,
};
use rsscript_mir::conformance::{MigrationCase, MigrationStage, execute_named};
use rsscript_mir::{
    BasicBlock, BlockId, FunctionId, MirFunction, MirFunctionDebug, MirFunctionSignature,
    MirInstruction, MirLiteral, MirModule, MirTerminator, TaskGroupId, TaskId, TypeId, ValueId,
};
use rsscript_sdk::{
    AsyncInterpreterFn, BlockingBehavior, CancellationBehavior, CancellationToken, Compiler,
    EvalError, ExternalFunction, ExternalFunctionRegistry, ExternalSymbol, FunctionSignature,
    MonotonicDeadline, NativeValue, ProviderCallMode, ProviderDescriptor, ProviderError,
    ProviderErrorCode, ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor,
    ProviderResource, ResourceCleanupContract, VmLimits, analyze_source_with_interfaces_result,
    reg_vm_compile_mir, reg_vm_compile_validated, reg_vm_eval_source_main,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const CASES: &[MigrationCase] = &[
    MigrationCase {
        name: "scalar_arithmetic",
        capability: "scalar arithmetic",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> Int {
    let left = 6
    let right = 7
    return left * right + 2
}
"#,
    },
    MigrationCase {
        name: "branching",
        capability: "branches",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> Int {
    local value = 41
    if value < 42 {
        return value + 1
    } else {
        return 0
    }
}
"#,
    },
    MigrationCase {
        name: "loop_and_assignment",
        capability: "loops and assignment",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> Int {
    let mut value = 0
    while value < 5 {
        value = value + 1
    }
    return value
}
"#,
    },
    MigrationCase {
        name: "direct_internal_call",
        capability: "direct internal calls",
        stage: MigrationStage::DualPath,
        source: r#"
fn helper() -> Int {
    return 7
}

fn main() -> Int {
    return helper()
}
"#,
    },
    MigrationCase {
        name: "direct_call_arguments",
        capability: "direct call arguments",
        stage: MigrationStage::DualPath,
        source: r#"
fn increment(value: Int) -> Int {
    return value + 1
}

fn main() -> Int {
    let seed = 41
    return increment(value: seed)
}
"#,
    },
    MigrationCase {
        name: "mutable_borrow_writeback",
        capability: "mutable call borrows",
        stage: MigrationStage::DualPath,
        source: r#"
fn increment_in_place(value: mut Int) -> Int {
    value = value + 1
    return value
}

fn main() -> Int {
    let mut value = 41
    increment_in_place(value: mut value)
    return value
}
"#,
    },
    MigrationCase {
        name: "take_moves_local",
        capability: "take call moves",
        stage: MigrationStage::DualPath,
        source: r#"
fn consume(value: take Int) -> Int {
    return value + 1
}

fn main() -> Int {
    local value = 41
    return consume(value: take value)
}
"#,
    },
    MigrationCase {
        name: "list_literal",
        capability: "owned list literals",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> List<Int> {
    return [1, 2, 3]
}
"#,
    },
    MigrationCase {
        name: "map_literal",
        capability: "owned map literals",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> Map<Int, Int> {
    return {1 => 2}
}
"#,
    },
    MigrationCase {
        name: "json_object_literal",
        capability: "JSON object literals",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> JsonValue {
    return {"count": 3}
}
"#,
    },
    MigrationCase {
        name: "record_constructor_and_field",
        capability: "resolved record construction and field access",
        stage: MigrationStage::DualPath,
        source: r#"
struct Box { count: Int }

fn main() -> Int {
    let item = Box(count: 42)
    return item.count
}
"#,
    },
    MigrationCase {
        name: "list_index",
        capability: "resolved list indexing",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> Int {
    let values = [40, 2]
    return values[1]
}
"#,
    },
    MigrationCase {
        name: "list_for_loop",
        capability: "checked List for loops",
        stage: MigrationStage::DualPath,
        source: r#"
fn main() -> Int {
    let values = [10, 20, 12]
    let mut total = 0
    for value in values {
        total = total + value
    }
    return total
}
"#,
    },
];

#[test]
fn dual_path_cases_match_the_legacy_vm() {
    let dual_path_cases = CASES
        .iter()
        .filter(|case| case.stage == MigrationStage::DualPath)
        .collect::<Vec<_>>();
    assert!(
        !dual_path_cases.is_empty(),
        "the migration harness needs at least one dual-path capability"
    );

    for case in dual_path_cases {
        let compiled = compile_source_to_ir(&format!("{}.rss", case.name), case.source)
            .unwrap_or_else(|diagnostics| {
                panic!(
                    "{} must remain compilable for {}: {diagnostics:#?}",
                    case.name, case.capability
                )
            });
        let mir = compiled.checked_hir_mir().unwrap_or_else(|error| {
            panic!(
                "{} must lower directly from checked HIR during dual-path migration: {error}",
                case.name
            )
        });
        let mir_value = execute_named(&mir, "main", Vec::new()).unwrap_or_else(|error| {
            panic!(
                "{} must execute in the MIR reference interpreter: {error}",
                case.name
            )
        });
        let legacy = reg_vm_eval_source_main(&format!("{}.rss", case.name), case.source)
            .unwrap_or_else(|error| {
                panic!("{} must execute in the legacy VM: {error:?}", case.name)
            });
        let mir_vm = reg_vm_compile_mir(
            &mir,
            compiled.source_hash(),
            compiled.interface_catalog_digest(),
        )
        .unwrap_or_else(|error| {
            panic!(
                "{} must emit verified bytecode directly from MIR: {error:?}",
                case.name
            )
        })
        .eval_main_with_args(std::iter::empty::<String>())
        .unwrap_or_else(|error| {
            panic!(
                "{} MIR-produced bytecode must execute in the VM: {error:?}",
                case.name
            )
        });
        assert_eq!(
            legacy.value,
            mir_value.render(),
            "legacy/MIR divergence for {} ({})",
            case.name,
            case.capability
        );
        assert_eq!(
            legacy.value, mir_vm.value,
            "legacy/MIR-bytecode VM divergence for {} ({})",
            case.name, case.capability
        );
    }
}

#[test]
fn supported_sdk_builds_use_the_mir_codegen_artifact() {
    let case = CASES
        .iter()
        .find(|case| case.name == "loop_and_assignment")
        .expect("scalar CFG migration fixture");
    let file = format!("{}.rss", case.name);
    // Match the reviewed snapshot API's multi-source frontend flavor exactly:
    // it includes the builtin interface set but has no supplied interfaces.
    // `compile_source_to_ir` uses the historical standard-package flavor and
    // therefore has a different immutable source hash even for this scalar
    // program.
    let validated = validate_sources_with_interfaces(&[(&file, case.source)], &[])
        .expect("snapshot fixture validates");
    let compiled = compile_validated_to_ir(&validated);
    let mut expected = rsscript_codegen_vm::emit_artifact(
        &compiled.mir().expect("fixture lowers to MIR"),
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
        env!("CARGO_PKG_VERSION"),
    )
    .expect("fixture emits with independent MIR codegen");
    let built = Compiler
        .compile(&file, case.source)
        .expect("SDK build succeeds");
    // Codegen owns the provider-neutral executable payload. The SDK owns the
    // workspace identity because it is the layer that captures the immutable
    // source snapshot. Bind the same snapshot before comparing envelopes.
    expected
        .bind_snapshot_digest(built.snapshot_digest())
        .expect("SDK snapshot identity binds to the emitted artifact");
    let expected = expected.to_bytes().expect("Artifact serializes");
    assert_eq!(
        built.artifact_bytes(),
        expected,
        "supported SDK compilation must use the MIR codegen Artifact rather than the legacy VM encoder"
    );
}

#[test]
fn linear_scalar_checked_hir_reaches_verified_bytecode_without_executable_ir() {
    let source = r#"
fn main() -> Int {
    let left = 40
    let right = 2
    return left + right
}
"#;
    let compiled =
        compile_source_to_ir("direct-hir-mir.rss", source).expect("direct-HIR fixture compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("linear scalar fixture uses direct checked-HIR lowering");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR MIR emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("direct HIR bytecode executes");
    assert_eq!(output.value, "42");
}

#[test]
fn direct_checked_hir_variant_construction_reaches_verified_bytecode() {
    let source = r#"
sum ResultValue {
    Value(count: Int)
}

fn main() -> ResultValue {
    return Value(count: 42)
}
"#;
    let compiled =
        compile_source_to_ir("direct-hir-variant.rss", source).expect("variant fixture compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("checked HIR variant uses direct lowering");
    assert!(mir.functions()[0]
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .any(|instruction| matches!(instruction, MirInstruction::MakeVariant { variant, .. } if variant == "Value")));
    reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR variant emits verified bytecode");
}

#[test]
fn direct_checked_hir_branch_reaches_verified_bytecode() {
    let source = r#"
fn main() -> Int {
    let value = 41
    if value < 42 {
        return value + 1
    } else {
        return 0
    }
}
"#;
    let compiled =
        compile_source_to_ir("direct-hir-branch.rss", source).expect("direct-HIR branch compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("checked HIR branch uses direct lowering");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR branch emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("direct HIR branch bytecode executes");
    assert_eq!(output.value, "42");
}

#[test]
fn direct_checked_hir_loop_reaches_verified_bytecode() {
    let source = r#"
fn main() -> Int {
    let mut value = 0
    while value < 5 {
        value = value + 1
    }
    return value
}
"#;
    let compiled =
        compile_source_to_ir("direct-hir-loop.rss", source).expect("direct-HIR loop compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("checked HIR loop uses direct lowering");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR loop emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("direct HIR loop bytecode executes");
    assert_eq!(output.value, "5");
}

#[test]
fn direct_checked_hir_loop_control_reaches_verified_bytecode() {
    let source = r#"
fn main() -> Int {
    let mut value = 0
    while value < 10 {
        value = value + 1
        if value == 3 {
            continue
        }
        if value == 5 {
            break
        }
    }
    return value
}
"#;
    let compiled = compile_source_to_ir("direct-hir-loop-control.rss", source)
        .expect("direct-HIR loop control compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("checked HIR loop control uses direct lowering");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR loop control emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("direct HIR loop control bytecode executes");
    assert_eq!(output.value, "5");
}

#[test]
fn direct_checked_hir_internal_calls_reach_verified_bytecode() {
    let source = r#"
fn helper() -> Int {
    return 42
}

fn main() -> Int {
    return helper()
}
"#;
    let compiled = compile_source_to_ir("direct-hir-call.rss", source)
        .expect("direct-HIR call fixture compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("resolved internal call uses direct checked-HIR lowering");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR call emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("direct HIR call bytecode executes");
    assert_eq!(output.value, "42");
}

#[test]
fn direct_checked_hir_effect_calls_reach_verified_bytecode() {
    let source = r#"
fn increment_in_place(value: mut Int) -> Int {
    value = value + 1
    return value
}

fn consume(value: take Int) -> Int {
    return value
}

fn main() -> Int {
    let mut value = 40
    increment_in_place(value: mut value)
    local taken = 41
    return consume(value: take taken)
}
"#;
    let compiled =
        compile_source_to_ir("direct-hir-effects.rss", source).expect("direct-HIR effects compile");
    let mir = compiled
        .checked_hir_mir()
        .expect("checked HIR effect calls use direct lowering");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct HIR effect calls emit verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("direct HIR effect call bytecode executes");
    assert_eq!(output.value, "41");
}

#[test]
fn capability_stages_stay_explicit() {
    for case in CASES {
        assert!(
            !case.capability.is_empty(),
            "{} needs a named migration capability",
            case.name
        );
        assert!(
            !case.source.trim().is_empty(),
            "{} needs a minimal conformance source",
            case.name
        );
    }
}

#[test]
fn resolved_external_call_reaches_verified_mir_bytecode() {
    let interface = "pub fn Host.increment(value: read Int) -> Int\n";
    let source = r#"
fn main() -> Int {
    let value = 41
    return Host.increment(value: read value)
}

"#;
    let validated = analyze_source_with_interfaces_result(
        "mir-external.rss",
        source,
        &[("host.rssi", interface)],
    )
    .into_validated()
    .expect("external call source should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("resolved external call should lower directly from checked HIR");
    assert_eq!(mir.external_imports().len(), 1);
    assert_eq!(
        mir.external_imports()[0].symbol().as_str(),
        "Host.increment"
    );
    assert!(mir.functions().iter().any(|function| {
        function.blocks().iter().flat_map(|block| block.instructions()).any(|instruction| {
            matches!(
                instruction,
                MirInstruction::Call {
                    target: rsscript_mir::MirCallTarget::External(_),
                    arguments,
                    ..
                } if matches!(arguments.as_slice(), [rsscript_mir::MirCallArgument::BorrowRead(_)])
            )
        })
    }));

    let host = ExternalFunction::new(|arguments| match arguments.as_slice() {
        [NativeValue::Int(value)] => Ok(NativeValue::Int(value + 1)),
        _ => Err(rsscript_sdk::ProviderError::internal(
            "unexpected host arguments",
        )),
    });
    let bindings = [("Host.increment", host.clone())];
    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy VM must compile the checked external call")
        .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), bindings.clone())
        .expect("legacy VM external call should execute");
    let mir_vm = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("MIR external call must emit verified bytecode")
    .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), bindings)
    .expect("MIR-produced bytecode external call should execute");

    assert_eq!(legacy.value, "42");
    assert_eq!(mir_vm.value, legacy.value);
}

#[test]
fn direct_checked_hir_retaining_call_emits_retain_fact() {
    let interface = "pub fn Store.put(value: read String) -> Unit retains(value)\n";
    let source = r#"
fn main() -> Unit {
    local value = "rss"
    Store.put(value: manage value)
    return
}
"#;
    let validated = analyze_source_with_interfaces_result(
        "mir-retain.rss",
        source,
        &[("store.rssi", interface)],
    )
    .into_validated()
    .expect("retaining call source should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("retaining call lowers directly from checked HIR");
    assert!(
        mir.functions()
            .iter()
            .flat_map(|function| function.blocks())
            .flat_map(|block| block.instructions())
            .any(|instruction| matches!(instruction, MirInstruction::Retain { .. }))
    );
    mir.verify()
        .expect("retention fact remains verifier-visible");

    let calls = Arc::new(AtomicU64::new(0));
    let provider = ExternalFunction::new({
        let calls = Arc::clone(&calls);
        move |arguments| match arguments.as_slice() {
            [NativeValue::String(value)] if value == "rss" => {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(NativeValue::Unit)
            }
            _ => Err(ProviderError::internal(
                "unexpected retaining-call arguments",
            )),
        }
    });
    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy retaining-call fixture compiles")
        .eval_main_with_args_and_external_bindings(
            std::iter::empty::<String>(),
            [("Store.put", provider.clone())],
        )
        .expect("legacy retaining-call fixture executes");
    let mir_vm = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("MIR retaining-call fixture emits verified bytecode")
    .eval_main_with_args_and_external_bindings(
        std::iter::empty::<String>(),
        [("Store.put", provider)],
    )
    .expect("MIR retaining-call fixture executes");
    assert_eq!(legacy.value, mir_vm.value);
    assert_eq!(legacy.usage.provider_calls, mir_vm.usage.provider_calls);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn direct_checked_hir_standalone_take_reaches_verified_bytecode() {
    let source = r#"
fn main() -> String {
    local value = "rss"
    return take value
}
"#;
    let compiled =
        compile_source_to_ir("direct-hir-take.rss", source).expect("standalone take compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("standalone take lowers directly from checked HIR");
    assert!(
        mir.functions()
            .iter()
            .flat_map(|function| function.blocks())
            .flat_map(|block| block.instructions())
            .any(|instruction| matches!(instruction, MirInstruction::TakePlace { .. }))
    );
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("standalone take emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("standalone take bytecode executes");
    assert_eq!(output.value, "rss");
}

#[test]
fn direct_checked_hir_builtin_literals_reach_verified_bytecode() {
    let source = r#"
fn main() -> Int {
    let mut value = 0
    while true {
        value = value + 1
        break
    }
    return value
}
"#;
    let compiled = compile_source_to_ir("direct-hir-builtin-literals.rss", source)
        .expect("builtin literal source compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("builtin literals lower directly from checked HIR");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("builtin literals emit verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("builtin literal bytecode executes");
    assert_eq!(output.value, "1");
}

#[test]
fn direct_checked_hir_literal_match_reaches_verified_bytecode() {
    let source = r#"
fn main() -> Int {
    let value = 1
    match value {
        0 => { return 0 }
        1 => { return 42 }
        _ => { return 2 }
    }
}
"#;
    let compiled = compile_source_to_ir("direct-hir-match.rss", source)
        .expect("literal match source compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("literal match lowers directly from checked HIR");
    assert!(
        mir.functions()[0]
            .blocks()
            .iter()
            .any(|block| { matches!(block.terminator(), MirTerminator::Branch { .. }) })
    );
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("literal match emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("literal match bytecode executes");
    assert_eq!(output.value, "42");
}

#[test]
fn direct_checked_hir_literal_match_expression_reaches_verified_bytecode() {
    let source = r#"
fn main() -> String {
    let value = 1
    return match value {
        0 => { "zero" }
        1 => { "one" }
        _ => { "many" }
    }
}
"#;
    let compiled = compile_source_to_ir("direct-hir-match-expression.rss", source)
        .expect("literal match expression source compiles");
    let mir = compiled
        .checked_hir_mir()
        .expect("literal match expression lowers directly from checked HIR");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("literal match expression emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("literal match expression bytecode executes");
    assert_eq!(output.value, "one");
}

#[test]
fn direct_checked_hir_resource_scope_emits_cleanup_before_return() {
    let interface = r#"
module Host
pub resource File
pub fn open() -> File
"#;
    let source = r#"
fn main() -> Int {
    with Host.open() as file {
        return 42
    }
}
"#;
    let validated = analyze_source_with_interfaces_result(
        "mir-resource.rss",
        source,
        &[("host.rssi", interface)],
    )
    .into_validated()
    .expect("resource scope source should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("resource scope lowers directly from checked HIR");
    let instructions = mir.functions()[0]
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .collect::<Vec<_>>();
    assert!(
        instructions
            .iter()
            .any(|instruction| { matches!(instruction, MirInstruction::AcquireResource { .. }) })
    );
    assert!(
        instructions
            .iter()
            .any(|instruction| { matches!(instruction, MirInstruction::ReleaseResource { .. }) })
    );
    mir.verify()
        .expect("resource scope MIR retains its cleanup proof");

    struct CountedResource(Arc<AtomicU64>);
    impl ProviderResource for CountedResource {
        fn cleanup(&mut self) -> Result<(), ProviderError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let cleanups = Arc::new(AtomicU64::new(0));
    let provider = ExternalFunction::new_contextual({
        let cleanups = Arc::clone(&cleanups);
        move |context, _| {
            context.register_resource(CountedResource(Arc::clone(&cleanups)))?;
            Ok(NativeValue::Native {
                type_name: "File".to_owned(),
                id: 1,
            })
        }
    });
    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy resource fixture compiles")
        .eval_main_with_args_and_external_bindings(
            std::iter::empty::<String>(),
            [("Host.open", provider.clone())],
        )
        .expect("legacy resource fixture executes");
    let direct = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct resource MIR emits verified bytecode")
    .eval_main_with_args_and_external_bindings(
        std::iter::empty::<String>(),
        [("Host.open", provider.clone())],
    )
    .expect("direct resource MIR executes");
    assert_eq!(legacy.value, direct.value);
    assert_eq!(legacy.usage.provider_calls, direct.usage.provider_calls);
    assert_eq!(
        legacy.usage.resources_created,
        direct.usage.resources_created
    );
    assert_eq!(
        legacy.usage.resources_cleaned,
        direct.usage.resources_cleaned
    );
    assert_eq!(cleanups.load(Ordering::SeqCst), 2);

    let exhausted_limits = VmLimits {
        resource_limit: Some(0),
        ..VmLimits::default()
    };
    let legacy_exhausted = reg_vm_compile_validated(&validated)
        .expect("legacy resource-limit fixture compiles")
        .execute_main_with_args_and_external_bindings_and_limits(
            std::iter::empty::<String>(),
            [("Host.open", provider.clone())],
            exhausted_limits.clone(),
        )
        .expect("legacy resource-limit fixture retains its report");
    let direct_exhausted = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct resource-limit MIR emits verified bytecode")
    .execute_main_with_args_and_external_bindings_and_limits(
        std::iter::empty::<String>(),
        [("Host.open", provider)],
        exhausted_limits,
    )
    .expect("direct resource-limit fixture retains its report");
    for report in [&legacy_exhausted, &direct_exhausted] {
        assert!(matches!(
            report.failure,
            Some(EvalError::Provider(ProviderError {
                code: ProviderErrorCode::ResourceExhausted,
                ..
            }))
        ));
    }
    assert_eq!(legacy_exhausted.usage, direct_exhausted.usage);
}

#[test]
fn direct_checked_hir_awaited_external_provider_matches_legacy_vm() {
    let source = "async fn main() -> Int { return await Host.async_value() }";
    let interface = "pub async fn Host.async_value() -> Int\n";
    let validated = analyze_source_with_interfaces_result(
        "direct-hir-async-provider.rss",
        source,
        &[("host-async.rssi", interface)],
    )
    .into_validated()
    .expect("async external fixture should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("awaited checked external calls lower directly to MIR");
    assert!(mir.functions().iter().any(|function| {
        function.blocks().iter().any(|block| {
            block.instructions().iter().any(|instruction| {
                matches!(
                    instruction,
                    MirInstruction::Call {
                        target: rsscript_mir::MirCallTarget::External(_),
                        ..
                    }
                )
            })
        })
    }));

    let symbol = ExternalSymbol::new("Host.async_value").expect("valid test symbol");
    let signature = FunctionSignature {
        parameters: Vec::new(),
        result: "Int".into(),
        asynchronous: true,
    };
    let descriptor = ProviderDescriptor {
        provider_id: "test.direct-async".into(),
        provider_version: "1.0.0".into(),
        supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "async_value".into(),
            call_mode: ProviderCallMode::Async,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::Cooperative,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    };
    let callable = AsyncInterpreterFn::new(|_, _| async {
        let mut first_poll = true;
        std::future::poll_fn(move |context| {
            if first_poll {
                first_poll = false;
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(Ok(NativeValue::Int(42)))
            }
        })
        .await
    });
    let mut registry = ExternalFunctionRegistry::new();
    registry
        .register_provider(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable,
                },
            )]),
        )
        .expect("async Provider registration should succeed");
    let bindings = registry.into_bindings().collect::<Vec<_>>();
    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy async external fixture compiles")
        .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), bindings.clone())
        .expect("legacy async external fixture executes");
    let direct = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct async external MIR emits verified bytecode")
    .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), bindings)
    .expect("direct async external MIR executes");

    assert_eq!(legacy.value, direct.value);
    assert_eq!(legacy.usage.provider_calls, 1);
    assert_eq!(legacy.usage.provider_calls, direct.usage.provider_calls);
    assert_eq!(legacy.usage, direct.usage);
    assert_matching_provider_trace(&legacy.provider_call_traces, &direct.provider_call_traces);
}

#[test]
fn direct_checked_hir_awaited_provider_cancellation_matches_legacy_vm() {
    let source = "async fn main() -> Int { return await Host.wait() }";
    let interface = "pub async fn Host.wait() -> Int\n";
    let validated = analyze_source_with_interfaces_result(
        "direct-hir-async-cancel.rss",
        source,
        &[("host-async.rssi", interface)],
    )
    .into_validated()
    .expect("async cancellation fixture should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("awaited external cancellation fixture lowers directly to MIR");

    fn bindings(cancellation: CancellationToken) -> Vec<(String, ExternalFunction)> {
        let symbol = ExternalSymbol::new("Host.wait").expect("valid test symbol");
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: "Int".into(),
            asynchronous: true,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.direct-async-cancel".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "wait".into(),
                call_mode: ProviderCallMode::Async,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        };
        let callable = AsyncInterpreterFn::new(move |_, _| {
            let cancellation = cancellation.clone();
            async move {
                let mut first_poll = true;
                std::future::poll_fn(move |context| {
                    if first_poll {
                        first_poll = false;
                        cancellation.cancel();
                        context.waker().wake_by_ref();
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(Ok(NativeValue::Int(42)))
                    }
                })
                .await
            }
        });
        let mut registry = ExternalFunctionRegistry::new();
        registry
            .register_provider(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable,
                    },
                )]),
            )
            .expect("async Provider registration should succeed");
        registry.into_bindings().collect()
    }

    let legacy_cancel = CancellationToken::new();
    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy async cancellation fixture compiles")
        .execute_main_with_args_and_external_bindings_and_limits(
            std::iter::empty::<String>(),
            bindings(legacy_cancel.clone()),
            VmLimits {
                cancel: Some(legacy_cancel),
                ..VmLimits::default()
            },
        )
        .expect("legacy cancellation retains its execution report");
    let direct_cancel = CancellationToken::new();
    let direct = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct async cancellation MIR emits verified bytecode")
    .execute_main_with_args_and_external_bindings_and_limits(
        std::iter::empty::<String>(),
        bindings(direct_cancel.clone()),
        VmLimits {
            cancel: Some(direct_cancel),
            ..VmLimits::default()
        },
    )
    .expect("direct cancellation retains its execution report");

    for report in [&legacy, &direct] {
        assert!(
            matches!(
                report.failure,
                Some(EvalError::Provider(ProviderError {
                    code: ProviderErrorCode::Cancelled,
                    ..
                }))
            ),
            "unexpected cancellation result: {:?}",
            report.failure
        );
    }
    assert_eq!(legacy.usage, direct.usage);
    assert_matching_provider_trace(&legacy.provider_call_traces, &direct.provider_call_traces);
}

#[test]
fn direct_checked_hir_awaited_provider_deadline_matches_legacy_vm() {
    let source = "async fn main() -> Int { return await Host.wait() }";
    let interface = "pub async fn Host.wait() -> Int\n";
    let validated = analyze_source_with_interfaces_result(
        "direct-hir-async-deadline.rss",
        source,
        &[("host-async.rssi", interface)],
    )
    .into_validated()
    .expect("async deadline fixture should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("awaited external deadline fixture lowers directly to MIR");

    fn bindings() -> Vec<(String, ExternalFunction)> {
        let symbol = ExternalSymbol::new("Host.wait").expect("valid test symbol");
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: "Int".into(),
            asynchronous: true,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.direct-async-deadline".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "wait".into(),
                call_mode: ProviderCallMode::Async,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        };
        let callable = AsyncInterpreterFn::new(|_, _| async move {
            std::thread::sleep(Duration::from_millis(5));
            Ok(NativeValue::Int(42))
        });
        let mut registry = ExternalFunctionRegistry::new();
        registry
            .register_provider(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable,
                    },
                )]),
            )
            .expect("async Provider registration should succeed");
        registry.into_bindings().collect()
    }

    let expired = || VmLimits {
        deadline: Some(MonotonicDeadline::after(Duration::from_millis(1))),
        ..VmLimits::default()
    };
    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy async deadline fixture compiles")
        .execute_main_with_args_and_external_bindings_and_limits(
            std::iter::empty::<String>(),
            bindings(),
            expired(),
        )
        .expect("legacy deadline retains its execution report");
    let direct = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct async deadline MIR emits verified bytecode")
    .execute_main_with_args_and_external_bindings_and_limits(
        std::iter::empty::<String>(),
        bindings(),
        expired(),
    )
    .expect("direct deadline retains its execution report");

    assert!(matches!(
        legacy.failure,
        Some(EvalError::Provider(ProviderError {
            code: ProviderErrorCode::DeadlineExceeded,
            ..
        }))
    ));
    assert_eq!(legacy.failure, direct.failure);
    assert_eq!(legacy.usage, direct.usage);
    assert_matching_provider_trace(&legacy.provider_call_traces, &direct.provider_call_traces);
}

#[test]
fn direct_checked_hir_async_provider_call_order_matches_legacy_vm() {
    let source = r#"
async fn main() -> Int {
    let first = await Host.first()
    let second = await Host.second()
    return first + second
}
"#;
    let interface = "pub async fn Host.first() -> Int\npub async fn Host.second() -> Int\n";
    let validated = analyze_source_with_interfaces_result(
        "direct-hir-async-order.rss",
        source,
        &[("host-async.rssi", interface)],
    )
    .into_validated()
    .expect("async call-order fixture should validate");
    let compiled = compile_validated_to_ir(&validated);
    let mir = compiled
        .checked_hir_mir()
        .expect("sequential awaited external calls lower directly to MIR");

    fn bindings() -> Vec<(String, ExternalFunction)> {
        let first = ExternalSymbol::new("Host.first").expect("valid first symbol");
        let second = ExternalSymbol::new("Host.second").expect("valid second symbol");
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: "Int".into(),
            asynchronous: true,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.direct-async-order".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            functions: vec![
                ProviderFunctionDescriptor {
                    symbol: first.clone(),
                    signature: signature.clone(),
                    entry: "first".into(),
                    call_mode: ProviderCallMode::Async,
                    blocking: BlockingBehavior::NonBlocking,
                    cancellation: CancellationBehavior::Cooperative,
                    thread_safe: true,
                    reentrant: true,
                    resource_cleanup: ResourceCleanupContract::None,
                    error_mapping: ProviderErrorMapping::StructuredV1,
                },
                ProviderFunctionDescriptor {
                    symbol: second.clone(),
                    signature: signature.clone(),
                    entry: "second".into(),
                    call_mode: ProviderCallMode::Async,
                    blocking: BlockingBehavior::NonBlocking,
                    cancellation: CancellationBehavior::Cooperative,
                    thread_safe: true,
                    reentrant: true,
                    resource_cleanup: ResourceCleanupContract::None,
                    error_mapping: ProviderErrorMapping::StructuredV1,
                },
            ],
        };
        let mut registry = ExternalFunctionRegistry::new();
        registry
            .register_provider(
                &descriptor,
                BTreeMap::from([
                    (
                        first,
                        ProviderFunction {
                            signature: signature.clone(),
                            callable: AsyncInterpreterFn::new(|_, _| async {
                                Ok(NativeValue::Int(40))
                            }),
                        },
                    ),
                    (
                        second,
                        ProviderFunction {
                            signature,
                            callable: AsyncInterpreterFn::new(|_, _| async {
                                Ok(NativeValue::Int(2))
                            }),
                        },
                    ),
                ]),
            )
            .expect("async Provider registration should succeed");
        registry.into_bindings().collect()
    }

    let legacy = reg_vm_compile_validated(&validated)
        .expect("legacy async call-order fixture compiles")
        .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), bindings())
        .expect("legacy async call-order fixture executes");
    let direct = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("direct async call-order MIR emits verified bytecode")
    .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), bindings())
    .expect("direct async call-order fixture executes");

    assert_eq!(legacy.value, direct.value);
    assert_eq!(legacy.usage, direct.usage);
    assert_matching_provider_trace(&legacy.provider_call_traces, &direct.provider_call_traces);
    assert_eq!(
        direct
            .provider_call_traces
            .iter()
            .map(|trace| trace.symbol.as_str())
            .collect::<Vec<_>>(),
        vec!["Host.first", "Host.second"],
    );
}

fn assert_matching_provider_trace(
    legacy: &[rsscript_sdk::ProviderCallTrace],
    direct: &[rsscript_sdk::ProviderCallTrace],
) {
    assert_eq!(legacy.len(), direct.len());
    for (legacy, direct) in legacy.iter().zip(direct) {
        assert_eq!(legacy.provider_id, direct.provider_id);
        assert_eq!(legacy.provider_version, direct.provider_version);
        assert_eq!(legacy.symbol, direct.symbol);
        assert_eq!(legacy.request_bytes, direct.request_bytes);
        assert_eq!(legacy.response_bytes, direct.response_bytes);
        assert_eq!(legacy.result, direct.result);
    }
}

#[test]
fn direct_checked_hir_async_task_group_emits_spawn_and_await() {
    let source = r#"
async fn worker() -> Int {
    return 42
}

fn main() -> Int {
    task_group {
        async let task = worker()
        let value = await task
        return value
    }
}
"#;
    let compiled = compile_source_to_ir("direct-hir-async.rss", source)
        .expect("structured async source should compile");
    let mir = compiled
        .checked_hir_mir()
        .expect("structured async lowers directly from checked HIR");
    let instructions = mir
        .functions()
        .iter()
        .flat_map(|function| function.blocks())
        .flat_map(|block| block.instructions())
        .collect::<Vec<_>>();
    assert!(
        instructions
            .iter()
            .any(|instruction| matches!(instruction, MirInstruction::Spawn { .. }))
    );
    assert!(
        instructions
            .iter()
            .any(|instruction| matches!(instruction, MirInstruction::Await { .. }))
    );
    mir.verify().expect("structured async MIR verifies");
    let output = reg_vm_compile_mir(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
    .expect("structured async MIR emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("structured async bytecode executes");
    assert_eq!(output.value, "42");
}

#[test]
fn spawned_mir_task_executes_through_verified_bytecode_vm() {
    let mir = MirModule::new(
        vec![WireType::Int {
            bits: 64,
            signed: true,
        }],
        vec![
            MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::Spawn {
                            task: TaskId::new(0),
                            group: TaskGroupId::new(0),
                            target: FunctionId::new(1),
                            arguments: vec![],
                        },
                        MirInstruction::Await {
                            destination: ValueId::new(0),
                            task: TaskId::new(0),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            ),
            MirFunction::new(
                FunctionId::new(1),
                MirFunctionSignature::new(vec![], TypeId::new(0), true),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Int(7),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            ),
        ],
        vec![
            MirFunctionDebug::new("main", vec![]),
            MirFunctionDebug::new("worker", vec![]),
        ],
        vec![],
    )
    .expect("task MIR verifies");
    let output = reg_vm_compile_mir(
        &mir,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
    )
    .expect("task MIR emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("task bytecode executes in the VM");
    assert_eq!(output.value, "7");
}

#[test]
fn task_group_join_drains_mir_children_before_returning() {
    let mir = MirModule::new(
        vec![WireType::Int {
            bits: 64,
            signed: true,
        }],
        vec![
            MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::Spawn {
                            task: TaskId::new(0),
                            group: TaskGroupId::new(0),
                            target: FunctionId::new(1),
                            arguments: vec![],
                        },
                        MirInstruction::Join {
                            group: TaskGroupId::new(0),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            ),
            MirFunction::new(
                FunctionId::new(1),
                MirFunctionSignature::new(vec![], TypeId::new(0), true),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Int(7),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            ),
        ],
        vec![
            MirFunctionDebug::new("main", vec![]),
            MirFunctionDebug::new("worker", vec![]),
        ],
        vec![],
    )
    .expect("task-group MIR verifies");
    let output = reg_vm_compile_mir(
        &mir,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
    )
    .expect("task-group MIR emits verified bytecode")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect("joined task bytecode executes in the VM");
    assert_eq!(output.value, "1");
    assert_eq!(output.usage.tasks_created, 2);
    assert_eq!(output.usage.tasks_completed, 2);
    assert_eq!(output.usage.tasks_live_at_return, 0);
}

#[test]
fn provider_resources_finalize_once_across_execution_terminal_paths() {
    const INTERFACE: &str = "pub fn Host.open() -> Unit\n";
    const SUCCESS: &str = r#"
fn main() -> Unit {
    Host.open()
    return
}
"#;
    const SCRIPT_ERROR: &str = r#"
fn main() -> Int {
    Host.open()
    return 1 / 0
}
"#;
    const LOOP_AFTER_OPEN: &str = r#"
fn main() -> Int {
    Host.open()
    let mut value = 0
    while value < 2000000 {
        value = value + 1
    }
    return value
}
"#;

    struct CountedResource {
        cleanups: Arc<AtomicU64>,
        fail: bool,
    }

    impl ProviderResource for CountedResource {
        fn cleanup(&mut self) -> Result<(), ProviderError> {
            self.cleanups.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(ProviderError::internal("intentional cleanup failure"))
            } else {
                Ok(())
            }
        }
    }

    fn executable(source: &str) -> rsscript_sdk::RegVmExecutable {
        let validated = analyze_source_with_interfaces_result(
            "resource-terminal.rss",
            source,
            &[("host.rssi", INTERFACE)],
        )
        .into_validated()
        .expect("resource fixture validates");
        reg_vm_compile_validated(&validated).expect("resource fixture compiles")
    }

    fn resource_provider(
        cleanups: Arc<AtomicU64>,
        fail_after_register: bool,
        cancel: Option<CancellationToken>,
        sleep_before_return: bool,
        fail_cleanup: bool,
    ) -> ExternalFunction {
        ExternalFunction::new_contextual(move |context, _| {
            context.register_resource(CountedResource {
                cleanups: Arc::clone(&cleanups),
                fail: fail_cleanup,
            })?;
            if let Some(cancel) = &cancel {
                cancel.cancel();
            }
            if sleep_before_return {
                std::thread::sleep(Duration::from_millis(5));
            }
            if fail_after_register {
                Err(ProviderError::internal("intentional Provider failure"))
            } else {
                Ok(NativeValue::Unit)
            }
        })
    }

    let cases = [
        ("success", SUCCESS, false, None, false, false, false, false),
        (
            "script error",
            SCRIPT_ERROR,
            false,
            None,
            false,
            false,
            false,
            false,
        ),
        (
            "Provider error",
            SUCCESS,
            true,
            None,
            false,
            false,
            false,
            false,
        ),
        (
            "cancellation",
            LOOP_AFTER_OPEN,
            false,
            Some(CancellationToken::new()),
            false,
            false,
            false,
            false,
        ),
        (
            "deadline",
            LOOP_AFTER_OPEN,
            false,
            None,
            true,
            false,
            true,
            false,
        ),
        (
            "cleanup failure",
            SUCCESS,
            false,
            None,
            false,
            true,
            false,
            true,
        ),
    ];

    for (
        name,
        source,
        provider_fails,
        cancellation,
        sleeps,
        cleanup_fails,
        expires,
        expect_cleanup_failure,
    ) in cases
    {
        let cleanups = Arc::new(AtomicU64::new(0));
        let executable = executable(source);
        let limits = VmLimits {
            deadline: expires.then(|| MonotonicDeadline::after(Duration::from_millis(1))),
            ..VmLimits::default()
        };
        let report = executable
            .execute_main_with_args_and_external_bindings_and_limits(
                std::iter::empty::<String>(),
                [(
                    "Host.open",
                    resource_provider(
                        Arc::clone(&cleanups),
                        provider_fails,
                        cancellation,
                        sleeps,
                        cleanup_fails,
                    ),
                )],
                limits,
            )
            .expect("terminal path must retain an execution report");
        assert_eq!(cleanups.load(Ordering::SeqCst), 1, "{name}");
        assert_eq!(report.usage.resources_created, 1, "{name}");
        assert_eq!(report.usage.resources_live_at_return, 0, "{name}");
        assert_eq!(
            report.usage.resources_cleaned,
            u64::from(!expect_cleanup_failure),
            "{name}"
        );
        assert_eq!(
            report.usage.resource_cleanup_failures,
            u64::from(expect_cleanup_failure),
            "{name}"
        );
    }
}
