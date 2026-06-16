use std::collections::BTreeSet;

use rsscript::{lower_coverage_report, vm_coverage_report};

// Intentional, behaviorally-covered execution gaps. A construct may appear here
// only if it is handled by another mechanism (a parse-error node that never
// lowers, a desugaring, or the async scheduler) and its VM<->compiler parity is
// verified elsewhere (tests/vm_eval.rs parity_select / parity_task_group and
// tests/backend_differential.rs). These lists make the gap check CONVERGE:
// closing a gap (a construct leaving `missing`) is always safe, but a construct
// becoming missing that is NOT on the list fails the test — i.e. a regression or
// an undocumented new gap.
const ALLOWED_COMPILER_STMT_GAPS: &[&str] = &[
    // Parse-error / malformed nodes are diagnostics; they never lower.
    "MalformedFor",
    "MalformedIf",
    "MalformedLoop",
    "MalformedMatch",
    "MalformedWith",
    "Unknown",
];
const ALLOWED_COMPILER_EXPR_GAPS: &[&str] = &[
    "Spawn",   // desugared by async lowering before expression lowering
    "Unknown", // parse-error node
];
const ALLOWED_VM_HIR_STMT_GAPS: &[&str] = &["Match", "Select"]; // desugared / scheduler
const ALLOWED_VM_HIR_EXPR_GAPS: &[&str] = &["Await", "Match", "Spawn"]; // desugared / scheduler
const ALLOWED_VM_FUNCTION_KIND_GAPS: &[&str] = &["async"]; // run via the cooperative scheduler

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

    assert_bucket_complete(&report.runtime_intrinsics);
    assert_bucket_consistent(&report.ast_statements);
    assert_bucket_consistent(&report.ast_expressions);
    assert_bucket_complete(&report.function_kinds);

    assert_gaps_within(
        "compiler AST statements",
        &report.ast_statements.missing,
        ALLOWED_COMPILER_STMT_GAPS,
    );
    assert_gaps_within(
        "compiler AST expressions",
        &report.ast_expressions.missing,
        ALLOWED_COMPILER_EXPR_GAPS,
    );
}

#[test]
fn vm_coverage_gaps_are_within_allowlist() {
    let vm = vm_coverage_report();

    // Fully-supported surfaces — these must never regress.
    assert_bucket_complete(&vm.runtime_intrinsics);
    assert_bucket_complete(&vm.special_forms);
    assert_bucket_complete(&vm.value_types);
    assert_bucket_complete(&vm.parity_features);

    assert_bucket_consistent(&vm.hir_statements);
    assert_bucket_consistent(&vm.hir_expressions);
    assert_bucket_consistent(&vm.function_kinds);

    // Converging gap check (see the allowlist comment above): closing a gap stays
    // green; a new, undocumented gap fails.
    assert_gaps_within(
        "VM HIR statements",
        &vm.hir_statements.missing,
        ALLOWED_VM_HIR_STMT_GAPS,
    );
    assert_gaps_within(
        "VM HIR expressions",
        &vm.hir_expressions.missing,
        ALLOWED_VM_HIR_EXPR_GAPS,
    );
    assert_gaps_within(
        "VM function kinds",
        &vm.function_kinds.missing,
        ALLOWED_VM_FUNCTION_KIND_GAPS,
    );
}

#[test]
fn parity_fixture_annotations_cover_supported_vm_features() {
    let report = vm_coverage_report();
    let source = std::fs::read_to_string("tests/vm_eval.rs")
        .expect("VM parity test source should be readable");
    let annotated = parity_features_from_source(&source);
    let required = report
        .parity_features
        .supported
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    let missing = required.difference(&annotated).cloned().collect::<Vec<_>>();
    let stale = annotated.difference(&required).cloned().collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "supported VM features without parity fixture annotations: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "parity fixture annotations not recognized as supported VM features: {stale:?}"
    );
}

/// Every missing construct must be on the documented allowlist; otherwise it is
/// a regression or an undocumented new gap. (Closing a gap is always allowed.)
fn assert_gaps_within(label: &str, missing: &[String], allowed: &[&str]) {
    let unexpected: Vec<&String> = missing
        .iter()
        .filter(|item| !allowed.contains(&item.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "{label}: execution gap(s) not in the allowlist (regression / undocumented): \
         {unexpected:?}\n  allowed: {allowed:?}"
    );
}

/// `total` must account for exactly the supported and missing entries.
fn assert_bucket_consistent(bucket: &rsscript::CoverageBucket) {
    assert_eq!(
        bucket.total(),
        bucket.supported_count() + bucket.missing_count(),
        "bucket total must equal supported + missing: {bucket:?}"
    );
}

fn assert_bucket_complete(bucket: &rsscript::CoverageBucket) {
    assert_eq!(
        bucket.total(),
        bucket.supported_count(),
        "bucket should be fully supported: {bucket:?}"
    );
    assert_eq!(
        bucket.missing_count(),
        0,
        "bucket should not have missing entries: {bucket:?}"
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
            .split([' ', '{', '(', ','])
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
