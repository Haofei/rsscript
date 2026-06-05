use std::collections::BTreeSet;

use rsscript::{interpreter_coverage_report, lower_coverage_report, vm_coverage_report};

#[test]
fn interpreter_coverage_report_tracks_hir_surface() {
    let report = interpreter_coverage_report();
    let hir_source = std::fs::read_to_string("src/hir.rs").expect("hir source should be readable");
    let interpreter_source = std::fs::read_to_string("src/interpreter.rs")
        .expect("interpreter source should be readable");
    let ast_source =
        std::fs::read_to_string("src/syntax/ast.rs").expect("ast source should be readable");

    assert_eq!(
        enum_variants(&hir_source, "HirStmt"),
        combined_bucket_all(&report.hir_statements, &report.hir_statement_recovery),
        "update interpreter_coverage_report() when HirStmt changes"
    );
    assert_eq!(
        enum_variants(&hir_source, "HirExpr"),
        combined_bucket_all(&report.hir_expressions, &report.hir_expression_recovery),
        "update interpreter_coverage_report() when HirExpr changes"
    );
    assert_eq!(
        enum_variants(&interpreter_source, "Value"),
        report.value_types.supported,
        "interpreter supported value types should track the executable Value enum"
    );
    assert_eq!(
        function_kinds_from_ast(&ast_source),
        report.function_kinds.all,
        "update interpreter_coverage_report() when FunctionDecl execution-mode fields change"
    );
}

#[test]
fn interpreter_coverage_baseline_is_explicit() {
    let report = interpreter_coverage_report();

    assert_bucket_counts(&report.runtime_intrinsics, 526, 526, 0);
    assert_bucket_counts(&report.hir_statements, 12, 12, 0);
    assert_bucket_counts(&report.hir_expressions, 17, 17, 0);
    assert_bucket_counts(&report.hir_statement_recovery, 1, 0, 1);
    assert_bucket_counts(&report.hir_expression_recovery, 1, 0, 1);
    assert_bucket_counts(&report.value_types, 14, 14, 0);
    assert_bucket_counts(&report.function_kinds, 3, 3, 0);
    assert_bucket_counts(&report.parity_features, 572, 572, 0);

    assert!(
        report
            .hir_statement_recovery
            .missing
            .contains(&"Unknown".to_string()),
        "statement recovery node should stay visible"
    );
    assert!(
        report
            .hir_expression_recovery
            .missing
            .contains(&"Unknown".to_string()),
        "expression recovery node should stay visible"
    );
    assert!(
        report.parity_features.missing.is_empty(),
        "supported interpreter features without parity fixture registry entries: {:?}",
        report.parity_features.missing
    );
}

#[test]
fn lower_coverage_report_tracks_ast_and_runtime_surface() {
    let report = lower_coverage_report();
    let ast_source =
        std::fs::read_to_string("src/syntax/ast.rs").expect("ast source should be readable");

    assert_eq!(
        enum_variants(&ast_source, "Stmt"),
        report.ast_statements.all,
        "update lower_coverage_report() when Stmt changes"
    );
    assert_eq!(
        enum_variants(&ast_source, "Expr"),
        report.ast_expressions.all,
        "update lower_coverage_report() when Expr changes"
    );
    assert_eq!(
        function_kinds_from_ast(&ast_source),
        report.function_kinds.all,
        "update lower_coverage_report() when FunctionDecl execution-mode fields change"
    );

    assert_bucket_counts(&report.runtime_intrinsics, 526, 526, 0);
    assert_bucket_counts(&report.ast_statements, 20, 14, 6);
    assert_bucket_counts(&report.ast_expressions, 19, 17, 2);
    assert_bucket_counts(&report.function_kinds, 3, 3, 0);

    assert!(
        report
            .ast_expressions
            .missing
            .contains(&"Spawn".to_string()),
        "known lower expression gap should stay visible"
    );
}

#[test]
fn vm_coverage_gap_is_explicit() {
    let interpreter = interpreter_coverage_report();
    let vm = vm_coverage_report();

    assert_bucket_counts(&vm.runtime_intrinsics, 526, 526, 0);
    assert_bucket_counts(&vm.hir_statements, 12, 12, 0);
    assert_bucket_counts(&vm.hir_expressions, 17, 17, 0);
    assert_bucket_counts(&vm.value_types, 14, 14, 0);
    assert_bucket_counts(&vm.function_kinds, 3, 3, 0);
    assert_bucket_counts(&vm.parity_features, 572, 572, 0);

    assert_vm_bucket_targets_interpreter(
        &vm.runtime_intrinsics,
        &interpreter.runtime_intrinsics,
        "runtime intrinsic",
    );
    assert_vm_bucket_targets_interpreter(
        &vm.hir_statements,
        &interpreter.hir_statements,
        "HIR statement",
    );
    assert_vm_bucket_targets_interpreter(
        &vm.hir_expressions,
        &interpreter.hir_expressions,
        "HIR expression",
    );
    assert_vm_bucket_targets_interpreter(&vm.value_types, &interpreter.value_types, "value type");
    assert_vm_bucket_targets_interpreter(
        &vm.function_kinds,
        &interpreter.function_kinds,
        "function kind",
    );
    assert_vm_bucket_targets_interpreter(
        &vm.parity_features,
        &interpreter.parity_features,
        "parity feature",
    );
}

#[test]
fn parity_fixture_annotations_cover_supported_interpreter_features() {
    let report = interpreter_coverage_report();
    let source = std::fs::read_to_string("tests/interpreter.rs")
        .expect("interpreter parity test source should be readable");
    let annotated = parity_features_from_source(&source);
    let required = report
        .parity_features
        .all
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    let missing = required.difference(&annotated).cloned().collect::<Vec<_>>();
    let stale = annotated.difference(&required).cloned().collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "supported interpreter features without parity fixture annotations: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "parity fixture annotations not recognized as supported interpreter features: {stale:?}"
    );
}

fn assert_bucket_counts(
    bucket: &rsscript::CoverageBucket,
    total: usize,
    supported: usize,
    missing: usize,
) {
    assert_eq!(
        bucket.total(),
        total,
        "total changed for bucket: {bucket:?}"
    );
    assert_eq!(
        bucket.supported_count(),
        supported,
        "supported count changed for bucket: {bucket:?}"
    );
    assert_eq!(
        bucket.missing_count(),
        missing,
        "missing count changed for bucket: {bucket:?}"
    );
}

fn combined_bucket_all(
    left: &rsscript::CoverageBucket,
    right: &rsscript::CoverageBucket,
) -> Vec<String> {
    left.all
        .iter()
        .chain(right.all.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn assert_vm_bucket_targets_interpreter(
    vm: &rsscript::CoverageBucket,
    interpreter: &rsscript::CoverageBucket,
    label: &str,
) {
    assert_eq!(
        vm.all, interpreter.supported,
        "VM {label} coverage target should be the interpreter-supported surface"
    );
    assert_eq!(
        vm.supported.len() + vm.missing.len(),
        interpreter.supported.len(),
        "VM {label} coverage should partition the interpreter-supported surface"
    );
}

fn enum_variants(source: &str, enum_name: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let mut in_enum = false;
    let mut depth = 0_i32;

    for line in source.lines() {
        let trimmed = line.trim();
        if !in_enum {
            if trimmed == format!("pub enum {enum_name} {{")
                || trimmed == format!("enum {enum_name} {{")
            {
                in_enum = true;
                depth = 1;
            }
            continue;
        }

        depth += trimmed.matches('{').count() as i32;
        depth -= trimmed.matches('}').count() as i32;
        if depth <= 0 {
            break;
        }

        if !line.starts_with("    ") || line.starts_with("        ") {
            continue;
        }
        let Some(first) = trimmed.chars().next() else {
            continue;
        };
        if !first.is_ascii_uppercase() {
            continue;
        }
        let variant = trimmed
            .split(|ch: char| ch == ' ' || ch == '{' || ch == '(' || ch == ',')
            .next()
            .expect("variant token should exist");
        variants.push(variant.to_string());
    }

    variants.sort();
    variants
}

fn function_kinds_from_ast(source: &str) -> Vec<String> {
    let fields = struct_fields(source, "FunctionDecl");
    let mut kinds = Vec::new();
    if fields.contains(&"has_body".to_string()) {
        kinds.push("sync".to_string());
    }
    if fields.contains(&"is_async".to_string()) {
        kinds.push("async".to_string());
    }
    if fields.contains(&"is_native".to_string()) {
        kinds.push("native".to_string());
    }
    kinds.sort();
    kinds
}

fn struct_fields(source: &str, struct_name: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut in_struct = false;
    let mut depth = 0_i32;

    for line in source.lines() {
        let trimmed = line.trim();
        if !in_struct {
            if trimmed == format!("pub struct {struct_name} {{")
                || trimmed == format!("struct {struct_name} {{")
            {
                in_struct = true;
                depth = 1;
            }
            continue;
        }

        depth += trimmed.matches('{').count() as i32;
        depth -= trimmed.matches('}').count() as i32;
        if depth <= 0 {
            break;
        }

        let trimmed = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
        let Some((name, _)) = trimmed.split_once(':') else {
            continue;
        };
        if name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        {
            fields.push(name.to_string());
        }
    }

    fields.sort();
    fields
}

fn parity_features_from_source(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("// parity:"))
        .flat_map(|line| line.split_whitespace())
        .map(str::to_string)
        .collect()
}
