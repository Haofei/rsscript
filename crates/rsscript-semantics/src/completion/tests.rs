use super::*;
use rsscript_syntax::parse_source_prefix;

fn complete(source: &str) -> SemanticCompletionResult {
    semantic_completion(
        "completion.rss",
        source,
        &parse_source_prefix("completion.rss", source),
    )
}

fn candidate<'a>(result: &'a SemanticCompletionResult, name: &str) -> &'a SemanticCompletion {
    result
        .candidates
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("missing {name}: {:#?}", result.candidates))
}

fn candidate_of_kind<'a>(
    result: &'a SemanticCompletionResult,
    name: &str,
    kind: SemanticCompletionKind,
) -> &'a SemanticCompletion {
    result
        .candidates
        .iter()
        .find(|candidate| candidate.name == name && candidate.kind == kind)
        .unwrap_or_else(|| panic!("missing {name} ({kind:?}): {:#?}", result.candidates))
}

#[test]
fn offers_typed_params_and_locals_in_the_enclosing_scope() {
    let result = complete("fn run(input: read String) -> Unit {\n    let count: Int = 1\n    ");
    let input = candidate(&result, "input");
    assert_eq!(input.kind, SemanticCompletionKind::Param);
    assert_eq!(
        input.ty.as_ref().and_then(ResolvedType::root_name),
        Some("String")
    );
    assert_eq!(input.required_effect, None);
    let count = candidate(&result, "count");
    assert_eq!(count.kind, SemanticCompletionKind::Local);
    assert_eq!(
        count.ty.as_ref().and_then(ResolvedType::root_name),
        Some("Int")
    );
}

#[test]
fn nested_scope_shadows_outer_bindings() {
    let result = complete(
        "fn run(value: read Int) -> Unit {\n    let label: Int = 1\n    if true {\n        let label: String = \"inner\"\n        ",
    );
    let label = candidate(&result, "label");
    assert_eq!(
        label.ty.as_ref().and_then(ResolvedType::root_name),
        Some("String")
    );
    assert_eq!(label.scope_depth, 2);
}

#[test]
fn excludes_taken_bindings_and_marks_the_result_partial() {
    let result = complete(
        "fn run(value: take String, other: read String) -> Unit {\n    consume(value: take value)\n    ",
    );
    assert!(
        !result
            .candidates
            .iter()
            .any(|candidate| candidate.name == "value")
    );
    assert_eq!(result.completeness, SemanticCompletionCompleteness::Partial);
    assert!(
        result
            .candidates
            .iter()
            .any(|candidate| candidate.name == "other")
    );
}

#[test]
fn projects_expected_types_for_return_let_assignment_and_conditions() {
    let return_result = complete("fn run() -> String {\n    return ");
    assert_eq!(
        return_result
            .expected_type
            .as_ref()
            .and_then(ResolvedType::root_name),
        Some("String")
    );

    let let_result = complete("fn run() -> Unit {\n    let ok: Bool = ");
    assert_eq!(
        let_result
            .expected_type
            .as_ref()
            .and_then(ResolvedType::root_name),
        Some("Bool")
    );

    let assign_result = complete("fn run() -> Unit {\n    let ok: Bool = true\n    ok = ");
    assert_eq!(
        assign_result
            .expected_type
            .as_ref()
            .and_then(ResolvedType::root_name),
        Some("Bool")
    );

    let condition_result = complete("fn run() -> Unit {\n    if ");
    assert_eq!(
        condition_result
            .expected_type
            .as_ref()
            .and_then(ResolvedType::root_name),
        Some("Bool")
    );
}

#[test]
fn completes_only_remaining_named_arguments_with_effect_and_type() {
    let result = complete(
        "fn paint(canvas: mut Canvas, title: read String, retries: read Int) -> Unit { }\nfn run(canvas: mut Canvas, title: read String) -> Unit {\n    paint(canvas: mut canvas, ",
    );
    assert!(
        !result
            .candidates
            .iter()
            .any(|candidate| candidate.name == "canvas"
                && candidate.kind == SemanticCompletionKind::ArgumentName)
    );
    let title = candidate_of_kind(&result, "title", SemanticCompletionKind::ArgumentName);
    assert_eq!(title.kind, SemanticCompletionKind::ArgumentName);
    assert_eq!(title.required_effect, Some(ParamEffect::Read));
    assert_eq!(
        title.ty.as_ref().and_then(ResolvedType::root_name),
        Some("String")
    );
    assert_eq!(
        result
            .expected_type
            .as_ref()
            .and_then(ResolvedType::root_name),
        Some("String")
    );
}

#[test]
fn exposes_top_level_and_interface_signatures_without_guessing_variants() {
    let source = "type Nick = String\nfn local() -> Unit { }\n";
    let prefix = parse_source_prefix("completion.rss", source);
    let interface = parse_source_raw("host.rssi", "pub fn external(value: read Int) -> String");
    let result =
        semantic_completion_with_interfaces("completion.rss", source, &[interface], &prefix);
    assert_eq!(
        candidate(&result, "local").kind,
        SemanticCompletionKind::Function
    );
    assert_eq!(
        candidate_of_kind(&result, "Nick", SemanticCompletionKind::Type).kind,
        SemanticCompletionKind::Type
    );
    let external = candidate(&result, "external");
    assert_eq!(external.kind, SemanticCompletionKind::Function);
    assert_eq!(
        external.ty.as_ref().and_then(ResolvedType::root_name),
        Some("String")
    );
    assert!(
        !result
            .candidates
            .iter()
            .any(|candidate| candidate.kind == SemanticCompletionKind::Variant)
    );
}

#[test]
fn receiver_methods_are_resolved_from_typed_hir_signatures() {
    let source = "fn String.inspect(self: read String) -> Int { return 0 }\nfn run(text: read String) -> Unit {\n    text.";
    let prefix = parse_source_prefix("completion.rss", source);
    assert_eq!(prefix.replace_range, source.len()..source.len());
    let result = semantic_completion("completion.rss", source, &prefix);
    let inspect = candidate_of_kind(&result, "inspect", SemanticCompletionKind::Method);
    assert_eq!(inspect.insert_text, "inspect()");
    assert_eq!(inspect.required_effect, Some(ParamEffect::Read));
    assert_eq!(
        inspect.ty.as_ref().and_then(ResolvedType::root_name),
        Some("Int")
    );
    assert!(
        !result
            .candidates
            .iter()
            .any(|candidate| candidate.name == "run"),
        "receiver completion must not mix unrelated global candidates"
    );
}

#[test]
fn variants_require_a_proven_sum_expected_type() {
    let typed = complete(
        "sum Status {\n    Ready\n    Failed(message: String)\n}\nfn run() -> Unit {\n    let status: Status = F",
    );
    let failed = candidate_of_kind(&typed, "Failed", SemanticCompletionKind::Variant);
    assert_eq!(failed.insert_text, "Failed()");
    assert_eq!(
        failed.ty.as_ref().and_then(ResolvedType::root_name),
        Some("Status")
    );

    let untyped = complete(
        "sum Status {\n    Ready\n    Failed(message: String)\n}\nfn run() -> Unit {\n    F",
    );
    assert!(
        !untyped
            .candidates
            .iter()
            .any(|candidate| candidate.kind == SemanticCompletionKind::Variant),
        "a bare expression does not prove a sum expected type"
    );

    let pattern = complete(
        "sum Status {\n    Ready\n    Failed(message: String)\n}\nfn run(status: read Status) -> Unit {\n    match status {\n        R",
    );
    assert!(pattern.candidates.iter().any(|candidate| {
        candidate.name == "Ready" && candidate.kind == SemanticCompletionKind::Variant
    }));
}

#[test]
fn reports_full_semantic_validity_from_the_completion_analysis() {
    let valid = complete("fn main() -> Unit {}");
    assert_eq!(valid.validity, SemanticCompletionValidity::Valid);

    let invalid = complete("fn main() -> Unit {\n    unknown_name\n}");
    assert_eq!(invalid.validity, SemanticCompletionValidity::Invalid);
}

#[test]
fn rejects_a_stale_prefix_instead_of_combining_two_source_revisions() {
    let original = "fn main() -> Unit {\n    ";
    let stale_prefix = parse_source_prefix("completion.rss", original);
    let result = semantic_completion(
        "completion.rss",
        "fn main() -> Unit {\n    unknown_name\n}",
        &stale_prefix,
    );
    assert!(result.candidates.is_empty());
    assert_eq!(result.completeness, SemanticCompletionCompleteness::Partial);
    assert_eq!(result.validity, SemanticCompletionValidity::Partial);
}
