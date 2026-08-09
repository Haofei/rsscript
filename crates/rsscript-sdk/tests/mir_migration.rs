//! Reusable migration gate for the typed MIR rollout.
//!
//! Add a capability by declaring its stage and a small source case. A
//! `DualPath` case must compile to verified MIR and produce the same result in
//! the legacy VM and the feature-gated MIR reference interpreter.

use rsscript_compiler::compile_source_to_ir;
use rsscript_mir::conformance::{MigrationCase, MigrationStage, execute_named};
use rsscript_sdk::reg_vm_eval_source_main;

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
        assert_eq!(
            legacy.value,
            mir_value.render(),
            "legacy/MIR divergence for {} ({})",
            case.name,
            case.capability
        );
    }
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
