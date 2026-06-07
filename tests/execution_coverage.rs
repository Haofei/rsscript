use std::collections::BTreeSet;

use rsscript::{lower_coverage_report, vm_coverage_report};

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
    let vm = vm_coverage_report();

    assert_bucket_complete(&vm.runtime_intrinsics);
    assert_bucket_complete(&vm.special_forms);
    assert_bucket_counts(&vm.hir_statements, 12, 10, 2);
    assert_bucket_counts(&vm.hir_expressions, 17, 14, 3);
    assert_bucket_counts(&vm.value_types, 15, 15, 0);
    assert_bucket_counts(&vm.function_kinds, 3, 2, 1);
    assert_bucket_complete(&vm.parity_features);

    assert!(vm.runtime_intrinsics.missing.is_empty());
    assert!(vm.special_forms.missing.is_empty());
    assert_eq!(
        vm.hir_statements.missing,
        vec!["Match".to_string(), "Select".to_string()]
    );
    assert_eq!(
        vm.hir_expressions.missing,
        vec![
            "Await".to_string(),
            "Match".to_string(),
            "Spawn".to_string()
        ]
    );
    assert!(vm.value_types.missing.is_empty());
    assert_eq!(vm.function_kinds.missing, vec!["async".to_string()]);
    assert!(vm.parity_features.missing.is_empty());
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
