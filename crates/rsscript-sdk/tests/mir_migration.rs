//! Reusable migration gate for the typed MIR rollout.
//!
//! Add a capability by declaring its stage and a small source case. A
//! `DualPath` case must compile to verified MIR and produce the same result in
//! the legacy VM, the feature-gated MIR reference interpreter, and the
//! verified-bytecode VM emitted directly from MIR.

use rsscript_abi_model::WireType;
use rsscript_compiler::{compile_source_to_ir, compile_validated_to_ir};
use rsscript_mir::conformance::{MigrationCase, MigrationStage, execute_named};
use rsscript_mir::{
    BasicBlock, BlockId, FunctionId, MirFunction, MirFunctionDebug, MirFunctionSignature,
    MirInstruction, MirLiteral, MirModule, MirTerminator, TaskGroupId, TaskId, TypeId, ValueId,
};
use rsscript_sdk::{
    CancellationToken, Compiler, ExternalFunction, MonotonicDeadline, NativeValue, ProviderError,
    ProviderResource, VmLimits, analyze_source_with_interfaces_result, reg_vm_compile_mir,
    reg_vm_compile_validated, reg_vm_eval_source_main,
};
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
        let mir = compiled.mir().unwrap_or_else(|error| {
            panic!(
                "{} must lower to MIR during dual-path migration: {error}",
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
    let compiled = compile_source_to_ir(&file, case.source).expect("fixture compiles");
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
