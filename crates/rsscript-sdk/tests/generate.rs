use rsscript_sdk::{
    ContinuationOptions, GenerateContext, LiteralClass, PrefixStatus, SymbolCompleteness,
    prefix_status, valid_continuations,
};

fn context(source: &str) -> GenerateContext<'_> {
    GenerateContext {
        file: "generate-test.rss",
        partial_source: source,
        symbol_completeness: SymbolCompleteness::Complete,
    }
}

#[test]
fn prefix_status_accepts_complete_program() {
    let status = prefix_status(&context("fn main() -> Unit {\n    return Unit\n}\n"));

    assert_eq!(status, PrefixStatus::Complete);
}

#[test]
fn prefix_status_keeps_unclosed_block_incomplete() {
    let status = prefix_status(&context("fn main() -> Unit {\n    return Unit\n"));

    assert_eq!(status, PrefixStatus::Incomplete);
}

#[test]
fn prefix_status_reports_committed_unmatched_close_as_dead() {
    let status = prefix_status(&context("fn main() -> Unit {\n    return Unit\n}\n}\n"));

    match status {
        PrefixStatus::Dead { reason, .. } => {
            assert!(reason.contains("unmatched `}`"));
        }
        other => panic!("expected dead prefix, got {other:?}"),
    }
}

#[test]
fn continuations_offer_top_level_keywords() {
    let continuations = valid_continuations(&context("f"), ContinuationOptions::default());

    assert!(continuations.keywords.contains(&"fn".to_string()));
    assert!(!continuations.keywords.contains(&"let".to_string()));
}

#[test]
fn continuations_offer_effect_keywords_after_argument_label() {
    let continuations = valid_continuations(
        &context("fn main() -> Unit {\n    File.read_string(path: "),
        ContinuationOptions::default(),
    );

    assert_eq!(continuations.keywords, vec!["read", "mut", "take"]);
    assert!(continuations.literals.is_empty());
}

#[test]
fn continuations_offer_statement_keywords_and_literals_inside_block() {
    let continuations = valid_continuations(
        &context("fn main() -> Unit {\n    r"),
        ContinuationOptions::default(),
    );

    assert!(continuations.keywords.contains(&"return".to_string()));
    assert!(continuations.literals.contains(&LiteralClass::String));
}
