use crate::diagnostic::{Diagnostic, Span, code};
use crate::syntax::ast::{Item, TypeRef};
use crate::syntax::parse_source;

const MAX_PUBLIC_PARAMS: usize = 6;
const MAX_PUBLIC_GENERICS: usize = 3;
const MAX_PUBLIC_EFFECTS: usize = 4;
const MAX_TYPE_DEPTH: usize = 4;

pub fn lint_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let program = parse_source(file, source);
    let mut diagnostics = Vec::new();

    for item in program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if !function.is_public {
            continue;
        }

        if function.params.len() > MAX_PUBLIC_PARAMS {
            signature_complexity_warning(
                &mut diagnostics,
                function.span.clone(),
                format!(
                    "Public function `{}` has {} parameters.",
                    function.name,
                    function.params.len()
                ),
                format!(
                    "Keep public signatures at or below {MAX_PUBLIC_PARAMS} parameters, or group related values into a reviewable struct."
                ),
            );
        }

        if function.type_params.len() > MAX_PUBLIC_GENERICS {
            signature_complexity_warning(
                &mut diagnostics,
                function.span.clone(),
                format!(
                    "Public function `{}` has {} generic parameters.",
                    function.name,
                    function.type_params.len()
                ),
                format!(
                    "Keep public signatures at or below {MAX_PUBLIC_GENERICS} generic parameters."
                ),
            );
        }

        if function.effects.len() > MAX_PUBLIC_EFFECTS {
            signature_complexity_warning(
                &mut diagnostics,
                function.span.clone(),
                format!(
                    "Public function `{}` declares {} effects.",
                    function.name,
                    function.effects.len()
                ),
                format!("Keep public effect clauses at or below {MAX_PUBLIC_EFFECTS} entries."),
            );
        }

        for param in &function.params {
            let depth = type_depth(&param.ty);
            if depth > MAX_TYPE_DEPTH {
                signature_complexity_warning(
                    &mut diagnostics,
                    param.span.clone(),
                    format!(
                        "Parameter `{}` in public function `{}` has nested type depth {}.",
                        param.name, function.name, depth
                    ),
                    format!(
                        "Keep public parameter types at or below nesting depth {MAX_TYPE_DEPTH}."
                    ),
                );
            }
        }

        if let Some(return_ty) = &function.return_ty {
            let depth = type_depth(return_ty);
            if depth > MAX_TYPE_DEPTH {
                signature_complexity_warning(
                    &mut diagnostics,
                    return_ty.span.clone(),
                    format!(
                        "Return type of public function `{}` has nested type depth {}.",
                        function.name, depth
                    ),
                    format!("Keep public return types at or below nesting depth {MAX_TYPE_DEPTH}."),
                );
            }
        }
    }

    diagnostics
}

fn signature_complexity_warning(
    diagnostics: &mut Vec<Diagnostic>,
    span: Span,
    summary: impl Into<String>,
    cause: impl Into<String>,
) {
    diagnostics.push(
        Diagnostic::warning(
            code::LINT_SIGNATURE_COMPLEXITY,
            summary,
            span,
            "signature complexity",
        )
        .with_cause(cause)
        .with_fix(
            "simplify_public_signature",
            "Simplify this public signature so it remains reviewable in one screen.",
            "manual",
        ),
    );
}

fn type_depth(ty: &TypeRef) -> usize {
    1 + ty.args.iter().map(type_depth).max().unwrap_or(0)
}
