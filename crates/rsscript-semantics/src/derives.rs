//! Semantic validation for language-owned derives.

use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::Span;

/// Validate source derives independently of compiler orchestration. Resource
/// derives are part of the language ownership model, not Rust lowering policy.
pub fn derive_syntax_diagnostics(
    derives: &[String],
    span: &Span,
    is_resource: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for derive in derives {
        if !is_supported_derive(derive) {
            diagnostics.push(Diagnostic::error(
                code::UNSUPPORTED_SYNTAX,
                "unsupported RSScript syntax.",
                span.clone(),
                "unsupported derive",
            )
            .with_cause(
                "This name is not a compiler-owned RSScript derive. Supported derives are Debug, Clone, Eq, Ord, Hash, JsonEncode, JsonDecode, Schema, and ReviewSchema.",
            )
            .with_fix(
                "rewrite_supported_syntax",
                "Rewrite this construct using the currently supported RSScript syntax.",
                "manual",
            ));
            continue;
        }
        if is_resource && !matches!(derive.as_str(), "Debug" | "Schema" | "ReviewSchema") {
            diagnostics.push(
                Diagnostic::error(
                    code::RESOURCE_DERIVE_UNSUPPORTED,
                    format!("`{derive}` derive is not allowed on a resource."),
                    span.clone(),
                    "resources support only `Debug`, `Schema`, and `ReviewSchema`",
                )
                .with_cause(
                    "Resources are move-only RAII values. Value derives such as `Clone`, `Eq`, `Ord`, `Hash`, `JsonEncode`, and `JsonDecode` would copy or compare the resource, which contradicts its move-only model.",
                )
                .with_fix(
                    "remove_resource_derive",
                    format!("Remove `{derive}` from the resource, or model the data as a `struct`."),
                    "manual",
                ),
            );
        }
    }
    diagnostics
}

fn is_supported_derive(name: &str) -> bool {
    matches!(
        name,
        "Debug"
            | "Clone"
            | "Eq"
            | "Ord"
            | "Hash"
            | "JsonEncode"
            | "JsonDecode"
            | "Schema"
            | "ReviewSchema"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unknown_and_resource_value_derives_in_source_order() {
        let span = Span {
            file: "derives.rss".into(),
            line: 1,
            column: 1,
            length: 1,
        };
        let diagnostics = derive_syntax_diagnostics(
            &["Unknown".into(), "Clone".into(), "Debug".into()],
            &span,
            true,
        );

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, code::UNSUPPORTED_SYNTAX);
        assert_eq!(diagnostics[1].code, code::RESOURCE_DERIVE_UNSUPPORTED);
    }
}
