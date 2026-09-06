use std::sync::Arc;

use rsscript_compiler::{
    Completeness, CompletionKind, ContinuationOptions, Effect, GenerationCoreInterfacePolicy,
    GenerationSession, ParserTerminal, SemanticValidity,
};

fn options(max_names: usize) -> ContinuationOptions {
    ContinuationOptions { max_names }
}

#[test]
fn same_revision_and_options_reuse_the_query_response() {
    let mut session = GenerationSession::with_source("main.rss", "fn main() -> Unit {");

    let first = session.query(options(20));
    let second = session.query(options(20));
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.identity, session.query_identity());
    assert!(serde_json::to_value(&*first).is_ok());
    assert!(serde_json::to_value(session.query_snapshot()).is_ok());
    assert!(serde_json::to_value(session.checkpoint()).is_ok());
}

#[test]
fn cache_reuses_full_facts_across_options_and_invalidates_on_policy_changes() {
    let mut session = GenerationSession::with_source("main.rss", "fn main() -> Unit {}");

    let one_name = session.query(options(1));
    let two_names = session.query(options(2));
    assert!(!Arc::ptr_eq(&one_name, &two_names));

    // A different projection replaces only the bounded response cache. The
    // full syntax/semantic facts are retained for the active revision.
    assert_eq!(session.stats().semantic_analyses, 1);
    let one_name_again = session.query(options(1));
    assert!(!Arc::ptr_eq(&one_name, &one_name_again));
    assert_eq!(session.stats().semantic_analyses, 1);
    assert!(Arc::ptr_eq(&one_name_again, &session.query(options(1))));

    assert!(session.set_core_interface_policy(GenerationCoreInterfacePolicy::WithoutCore));
    assert_eq!(
        session.core_interface_policy(),
        GenerationCoreInterfacePolicy::WithoutCore
    );
    assert!(!Arc::ptr_eq(&one_name_again, &session.query(options(1))));
    assert_eq!(session.stats().semantic_analyses, 2);
    assert!(!session.set_core_interface_policy(GenerationCoreInterfacePolicy::WithoutCore));
}

#[test]
fn append_restore_and_interface_edits_invalidate_cached_queries() {
    let mut session = GenerationSession::with_source("main.rss", "fn main() -> Unit {");
    let before = session.query(options(20));
    let checkpoint = session.checkpoint();

    session.append("\n");
    let after_append = session.query(options(20));
    assert!(!Arc::ptr_eq(&before, &after_append));

    session
        .restore(&checkpoint)
        .expect("same session checkpoint restores");
    let after_restore = session.query(options(20));
    assert!(!Arc::ptr_eq(&after_append, &after_restore));
    assert_eq!(session.source(), "fn main() -> Unit {");

    assert!(session.set_interface("host.rssi", "pub fn host() -> Unit"));
    let after_interface_edit = session.query(options(20));
    assert!(!Arc::ptr_eq(&after_restore, &after_interface_edit));
    assert!(!session.set_interface("host.rssi", "pub fn host() -> Unit"));
}

#[test]
fn restored_branches_never_reuse_source_or_interface_query_identities() {
    let mut session = GenerationSession::new("main.rss");
    let checkpoint = session.checkpoint();

    session.append("fn foo() -> Unit {}\n");
    session.set_interface("host.rssi", "pub fn foo() -> Unit");
    let first = session.query_snapshot();
    let first_response = session.query(options(20));

    session.restore(&checkpoint).unwrap();
    assert_eq!(session.source(), "");
    assert!(session.interface_snapshot().interfaces.is_empty());
    assert!(session.query_identity().revision > first.identity.revision);
    assert!(session.interface_snapshot().revision > first.interfaces.revision);

    session.append("fn bar() -> Unit {}\n");
    session.set_interface("host.rssi", "pub fn bar() -> Unit");
    let second = session.query_snapshot();
    let second_response = session.query(options(20));
    assert_eq!(first.identity.source_bytes, second.identity.source_bytes);
    assert_ne!(first.source, second.source);
    assert_ne!(first.identity, second.identity);
    assert_ne!(first.interfaces.revision, second.interfaces.revision);
    assert_eq!(first_response.identity, first.identity);
    assert_eq!(second_response.identity, second.identity);
}

#[test]
fn checkpoint_cannot_cross_session_identity_boundary() {
    let first = GenerationSession::with_source("main.rss", "fn main() -> Unit {}");
    let checkpoint = first.checkpoint();
    let mut second = GenerationSession::with_source("main.rss", "fn other() -> Unit {}");

    assert_ne!(first.session_id(), second.session_id());
    assert_eq!(
        second.restore(&checkpoint),
        Err(rsscript_compiler::GenerationRestoreError::DifferentSession)
    );
    assert_eq!(second.source(), "fn other() -> Unit {}");
}

#[test]
fn query_identity_captures_source_and_interface_revisions() {
    let mut session = GenerationSession::with_source("main.rss", "fn main() -> Unit {}");
    let original = session.query_identity();
    assert_eq!(original.source_bytes, 20);
    session.set_interface("host.rssi", "pub fn host() -> Unit");
    let with_interface = session.query_identity();
    assert_eq!(with_interface.session_id, original.session_id);
    assert!(with_interface.revision > original.revision);
    assert!(with_interface.interface_revision > original.interface_revision);
    session.append("\n");
    assert_eq!(
        session.query_identity().source_bytes,
        original.source_bytes + 1
    );
}

#[test]
fn response_identity_is_serialized_and_matches_the_session_query() {
    let mut session = GenerationSession::with_source("main.rss", "fn main() -> Unit {}");
    let expected = session.query_identity();
    let response = session.query(options(20));
    assert_eq!(response.identity, expected);
    let json = serde_json::to_value(&*response).expect("continuations serialize");
    assert_eq!(json["identity"]["session_id"], expected.session_id);
    assert_eq!(json["identity"]["revision"], expected.revision);
    assert_eq!(
        json["identity"]["interface_revision"],
        expected.interface_revision
    );
    assert_eq!(json["identity"]["source_bytes"], expected.source_bytes);
}

#[test]
fn interface_snapshot_is_ordered_and_visible_to_the_semantic_query() {
    let mut session = GenerationSession::with_source("main.rss", "fn main() -> Unit {\n    ");
    session.set_interface("z.rssi", "pub fn zebra() -> Unit");
    session.set_interface("a.rssi", "pub fn alpha() -> Unit");

    let snapshot = session.query_snapshot();
    assert_eq!(snapshot.interfaces.interfaces[0].path, "a.rssi");
    assert_eq!(snapshot.interfaces.interfaces[1].path, "z.rssi");

    let result = session.query(options(200));
    assert!(
        result.names.iter().any(
            |candidate| candidate.text == "zebra" && candidate.kind == CompletionKind::Function
        )
    );
}

#[test]
fn max_names_counts_all_discovered_names_before_truncating() {
    let mut session = GenerationSession::with_source(
        "main.rss",
        "fn alpha() -> Unit {}\nfn beta() -> Unit {}\nfn main() -> Unit {\n",
    );

    let result = session.query(options(1));
    assert_eq!(result.names.len(), 1);
    assert!(result.total_discovered_names > result.names.len());
    assert!(result.truncated);
}

#[test]
fn syntax_is_the_only_source_of_terminals() {
    let mut session = GenerationSession::new("main.rss");
    let empty = session.query(options(20));
    assert!(empty.terminals.iter().all(|terminal| {
        !matches!(terminal, ParserTerminal::Fixed { text, .. } if text == "features" || text == "native")
    }));

    session.append("features");
    let retired = session.query(options(20));
    assert!(!retired.may_stop);
    assert!(retired.terminals.is_empty());
}

#[test]
fn may_stop_requires_complete_syntax_and_a_valid_semantic_check() {
    let mut valid = GenerationSession::with_source("main.rss", "fn main() -> Unit {}");
    let valid_result = valid.query(options(20));
    assert_eq!(valid_result.semantic_validity, SemanticValidity::Valid);
    assert!(valid_result.may_stop);

    let mut invalid =
        GenerationSession::with_source("main.rss", "fn main() -> Unit {\n    unknown_name\n}");
    let invalid_result = invalid.query(options(20));
    assert_eq!(invalid_result.semantic_validity, SemanticValidity::Invalid);
    assert!(!invalid_result.may_stop);
}

#[test]
fn core_policy_controls_completion_and_stop_validity_from_the_same_analysis() {
    let source = "fn main() -> Unit {\n    List.is_empty(";
    let mut session = GenerationSession::with_source("main.rss", source);
    let with_core = session.query(options(100));
    assert!(with_core.names.iter().any(|candidate| {
        candidate.kind == CompletionKind::ArgName && candidate.text == "list"
    }));

    assert!(session.set_core_interface_policy(GenerationCoreInterfacePolicy::WithoutCore));
    let without_core = session.query(options(100));
    assert!(!without_core.names.iter().any(|candidate| {
        candidate.kind == CompletionKind::ArgName && candidate.text == "list"
    }));
    assert_eq!(without_core.semantic_validity, SemanticValidity::Invalid);
}

#[test]
fn scope_type_and_named_argument_facts_are_projected_conservatively() {
    let mut scope = GenerationSession::with_source(
        "main.rss",
        "fn main(value: read Int) -> Unit {\n    let count: Int = 1\n    ",
    );
    let scope_result = scope.query(options(200));
    let value = scope_result
        .names
        .iter()
        .find(|candidate| candidate.text == "value")
        .expect("parameter completion");
    assert_eq!(value.kind, CompletionKind::Param);
    assert_eq!(
        value.result_type.as_ref().map(|ty| ty.display.as_str()),
        Some("Int")
    );
    assert!(value.required_effect.is_none());

    let mut call = GenerationSession::with_source(
        "main.rss",
        "fn consume(input: take Int, label: read String) -> Unit {}\nfn main() -> Unit {\n    consume(",
    );
    let call_result = call.query(options(200));
    assert_eq!(
        call_result
            .expected_type
            .as_ref()
            .map(|ty| ty.display.as_str()),
        Some("Int")
    );
    let input = call_result
        .names
        .iter()
        .find(|candidate| candidate.text == "input")
        .expect("named argument completion");
    assert_eq!(input.kind, CompletionKind::ArgName);
    assert_eq!(input.insert_text, "input: take ");
    assert_eq!(input.required_effect, Some(Effect::Take));

    let mut read_call = GenerationSession::with_source(
        "main.rss",
        "fn labeler(label: read String) -> Unit {}\nfn main() -> Unit {\n    labeler(",
    );
    let read_result = read_call.query(options(200));
    let label = read_result
        .names
        .iter()
        .find(|candidate| candidate.text == "label")
        .expect("read named argument completion");
    assert_eq!(label.insert_text, "label: ");
    assert_eq!(label.required_effect, Some(Effect::Read));
    assert_eq!(read_result.name_completeness, Completeness::Partial);
}

#[test]
fn argument_effects_and_signatures_are_proved_at_their_call_sites() {
    let mut call = GenerationSession::with_source(
        "main.rss",
        "fn consume(read_value: read Int, mut_value: mut Int, take_value: take Int) -> Unit {}\nfn main() -> Unit {\n    consume(",
    );
    let result = call.query(options(200));
    for (name, expected_effect, expected_insert) in [
        ("read_value", Effect::Read, "read_value: "),
        ("mut_value", Effect::Mut, "mut_value: mut "),
        ("take_value", Effect::Take, "take_value: take "),
    ] {
        let candidate = result
            .names
            .iter()
            .find(|candidate| candidate.kind == CompletionKind::ArgName && candidate.text == name)
            .unwrap_or_else(|| panic!("missing named argument {name}"));
        assert_eq!(candidate.required_effect, Some(expected_effect));
        assert_eq!(candidate.insert_text, expected_insert);
    }

    let mut signature = GenerationSession::with_source(
        "main.rss",
        "fn consume(value: take String, target: mut Buffer) -> Unit {}\n",
    );
    let signature = signature.query(options(200));
    let consume = signature
        .names
        .iter()
        .find(|candidate| candidate.kind == CompletionKind::Function && candidate.text == "consume")
        .expect("function completion");
    assert_eq!(
        consume.signature.as_deref(),
        Some("consume(value: take String, target: mut Buffer) -> Unit")
    );
}

#[test]
fn receiver_method_completion_exposes_a_resolved_receiver_effect() {
    let mut session = GenerationSession::with_source(
        "main.rss",
        "fn String.inspect(self: read String) -> Int { return 0 }\nfn main(text: read String) -> Unit {\n    text.",
    );
    let response = session.query(options(200));
    let method = response
        .names
        .iter()
        .find(|candidate| candidate.kind == CompletionKind::Method && candidate.text == "inspect")
        .expect("typed receiver method completion");
    assert_eq!(method.required_effect, Some(Effect::Read));
    assert_eq!(
        method.result_type.as_ref().map(|ty| ty.display.as_str()),
        Some("Int")
    );
}
