// Used by optional Rust lowering and compiler-only test support; the default
// frontend build intentionally does not select either consumer.
#[allow(unused_imports)]
pub(crate) use rsscript_semantics::ResolvedType;
pub use rsscript_semantics::{
    AnalysisResult, FrontendCompletion, FrontendStopReason, SemanticDatabase, SourceFileSnapshot,
    SourceSnapshot, ValidatedProgram,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_snapshot_owns_the_captured_text() {
        let mut source = "fn main() -> Unit { return Unit }\n".to_string();
        let snapshot = SourceSnapshot::single("main.rss", &source);
        source.clear();

        assert_eq!(
            snapshot.files()[0].text(),
            "fn main() -> Unit { return Unit }\n"
        );
    }

    #[test]
    fn validated_program_requires_complete_error_free_analysis() {
        let validated =
            crate::analyzer::validate_source("main.rss", "fn main() -> Unit { return Unit }\n")
                .expect("clean source should validate");
        assert_eq!(validated.database().sources().files()[0].path(), "main.rss");
        assert_eq!(validated.database().source_programs().len(), 1);

        let diagnostics = crate::analyzer::validate_source(
            "invalid.rss",
            "fn main() -> Int { return Missing.value }\n",
        )
        .expect_err("frontend errors must not construct ValidatedProgram");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
    }

    #[test]
    fn semantic_database_interns_shared_signature_and_field_types() {
        let source = r#"
struct Holder<U> {
    value: U
}

fn first<U, W>(left: read U, right: read W) -> U {
    return left
}

fn main() -> Unit {
    let value: Int = first(left: read 1, right: read "unused")
    return Unit
}
"#;
        let validated =
            crate::analyzer::validate_source("structural-types.rss", source).expect("valid source");
        let database = validated.database();
        let types = database.hir().semantic_types();
        let first = types
            .functions()
            .find(|(name, _)| *name == "first")
            .map(|(_, facts)| facts)
            .expect("first signature facts");
        let holder = types.named_type("Holder").expect("Holder type facts");

        assert_eq!(first.type_parameters.as_ref(), ["U", "W"]);
        assert_eq!(types.arena().get(first.parameters[0].1).to_string(), "U");
        assert_eq!(types.arena().get(first.parameters[1].1).to_string(), "W");
        assert_eq!(
            first
                .return_type
                .map(|ty| types.arena().get(ty).to_string())
                .as_deref(),
            Some("U")
        );
        assert_eq!(
            first.parameters[0].1, holder.fields[0].1,
            "structurally identical U facts must share one TypeId"
        );
        assert!(database.interned_type_count() >= 3);
    }
}
