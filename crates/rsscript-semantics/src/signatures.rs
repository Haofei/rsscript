//! Source signature diagnostics owned by the semantic layer.

use std::collections::HashSet;

use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{FunctionDecl, Item, Program, TypeRef};

/// Validate explicit signature shape, protocol receivers, and retention
/// declarations for all source functions.
pub fn signature_diagnostics(program: &Program) -> Vec<Diagnostic> {
    let protocol_names = program
        .protocols
        .iter()
        .map(|protocol| protocol.name.as_str())
        .collect::<HashSet<_>>();
    let mut diagnostics = Vec::new();
    for function in program.items.iter().filter_map(|item| match item {
        Item::Function(function) => Some(function),
        _ => None,
    }) {
        let is_qualified_method = function.name.contains('.');
        let is_protocol_method = function
            .name
            .split_once('.')
            .is_some_and(|(namespace, _)| protocol_names.contains(namespace));
        collect_return_type_diagnostic(function, &mut diagnostics);
        collect_parameter_diagnostics(function, is_qualified_method, &mut diagnostics);
        collect_protocol_self_diagnostics(function, is_protocol_method, &mut diagnostics);
        collect_retains_diagnostics(function, &mut diagnostics);
    }
    diagnostics
}

fn collect_return_type_diagnostic(function: &FunctionDecl, diagnostics: &mut Vec<Diagnostic>) {
    if function.return_ty.is_none() {
        diagnostics.push(
            Diagnostic::error(
                code::MISSING_RETURN_TYPE,
                format!("function `{}` must declare an explicit return type.", function.name),
                function.span.clone(),
                "missing return type",
            )
            .with_cause("Public APIs must not rely on inference; this checker applies the canonical rule to all functions.")
            .with_fix("add_return_type", "Add `-> Unit` or another explicit return type.", "manual"),
        );
    }
}

fn collect_parameter_diagnostics(
    function: &FunctionDecl,
    is_qualified_method: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, param) in function.params.iter().enumerate() {
        if param.name == "self" && (!is_qualified_method || index != 0) {
            diagnostics.push(invalid_self_parameter_diagnostic(
                function,
                &param.span,
                "`self` may only be the first parameter of a qualified method signature.",
            ));
        }
        if param.ty.name.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    code::MISSING_PARAMETER_TYPE,
                    format!(
                        "parameter `{}` in `{}` must declare an explicit type.",
                        param.name, function.name
                    ),
                    param.span.clone(),
                    "missing parameter type",
                )
                .with_fix(
                    "add_parameter_type",
                    "Add an explicit parameter type.",
                    "manual",
                ),
            );
        }
    }
}

fn collect_protocol_self_diagnostics(
    function: &FunctionDecl,
    is_protocol_method: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !is_protocol_method {
        return;
    }
    match function.params.first() {
        Some(param)
            if param.name == "self"
                && param.ty.name == "Self"
                && param.effective_effect().is_some() => {}
        Some(param) => diagnostics.push(invalid_self_parameter_diagnostic(
            function,
            &param.span,
            "Protocol methods must declare `self: read|mut|take Self` as their first parameter.",
        )),
        None => diagnostics.push(
            Diagnostic::error(
                code::INVALID_SELF_PARAMETER,
                format!(
                    "protocol method `{}` must declare an explicit `self` parameter.",
                    function.name
                ),
                function.span.clone(),
                "missing protocol self parameter",
            )
            .with_cause("Protocol calls are explicit external_binding calls, so the receiver must be review-visible as `self: read|mut|take Self`.")
            .with_fix(
                "add_protocol_self",
                "Add `self: read Self`, `self: mut Self`, or `self: take Self` as the first parameter.",
                "manual",
            ),
        ),
    }
}

fn collect_retains_diagnostics(function: &FunctionDecl, diagnostics: &mut Vec<Diagnostic>) {
    let param_names = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<HashSet<_>>();
    for param in &function.retained_params {
        if !param_names.contains(param.as_str()) {
            diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_RETAINED_PARAMETER,
                    format!(
                        "`{}` declares `retains({param})`, but `{param}` is not a parameter.",
                        function.name
                    ),
                    function.span.clone(),
                    "unknown retained parameter",
                )
                .with_cause(
                    "Retention effects must name a parameter from the same function signature.",
                )
                .with_fix(
                    "fix_retains_parameter",
                    "Rename the retained parameter or remove this retention effect.",
                    "manual",
                ),
            );
            continue;
        }
        if let Some(function_param) = function
            .params
            .iter()
            .find(|function_param| function_param.name == *param)
            && type_ref_is_copy(&function_param.ty)
        {
            diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_RETAINED_PARAMETER,
                    format!(
                        "`{}` declares `retains({param})`, but `{param}` is Copy.",
                        function.name
                    ),
                    function_param.span.clone(),
                    "Copy parameter cannot be retained",
                )
                .with_cause("`retains(x)` marks a managed retention boundary. Copy values have no managed handle to retain.")
                .with_fix("remove_copy_retains", format!("Remove `retains({param})`."), "manual"),
            );
            continue;
        }
        if function.params.iter().any(|function_param| {
            function_param.name == *param && type_ref_is_noescape(&function_param.ty)
        }) {
            diagnostics.push(
                Diagnostic::error(
                    code::NOESCAPE_CALLBACK_ESCAPE,
                    format!(
                        "`{}` cannot retain noescape callback parameter `{param}`.",
                        function.name
                    ),
                    function.span.clone(),
                    "noescape callback escapes",
                )
                .with_cause("`noescape Fn()` parameters may be called or forwarded to another noescape parameter, but they cannot be retained after return.")
                .with_fix(
                    "remove_noescape_retention",
                    format!("Remove `retains({param})`, or use an ordinary managed callback type."),
                    "manual",
                ),
            );
        }
    }
}

fn invalid_self_parameter_diagnostic(
    function: &FunctionDecl,
    span: &rsscript_syntax::Span,
    cause: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::INVALID_SELF_PARAMETER,
        format!("invalid `self` parameter in `{}`.", function.name),
        span.clone(),
        "invalid self parameter",
    )
    .with_cause(cause)
    .with_fix(
        "fix_self_parameter",
        "Use a different parameter name, or make this the first parameter of an explicit method/protocol signature.",
        "manual",
    )
}

fn type_ref_is_noescape(ty: &TypeRef) -> bool {
    ty.is_noescape || ty.args.iter().any(type_ref_is_noescape)
}

fn type_ref_is_copy(ty: &TypeRef) -> bool {
    !ty.is_fresh
        && !ty.is_noescape
        && ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && matches!(
            ty.name.as_str(),
            "Bool"
                | "Byte"
                | "Char"
                | "Float"
                | "Float32"
                | "Float64"
                | "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Unit"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_signature_and_retention_contracts() {
        let program = rsscript_syntax::parse_source(
            "signatures.rss",
            r#"
fn missing(value: Int) { Unit }
fn retain(value: Int) -> Unit retains(value) { Unit }
"#,
        );
        let diagnostics = signature_diagnostics(&program);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, code::MISSING_RETURN_TYPE);
        assert_eq!(diagnostics[1].code, code::UNKNOWN_RETAINED_PARAMETER);
    }
}
