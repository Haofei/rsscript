//! Shared structural type compatibility for call arguments and collection literals.

use super::*;

pub(super) fn callee_name(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { name, .. } => name.clone(),
        Callee::ReceiverCall { method, .. } => method.clone(),
    }
}

pub(super) fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            (*effect).unwrap_or(DataEffect::Read).as_str(),
            call_expr_label(receiver)
        ),
    }
}

pub(super) fn call_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::CharLiteral(value, _) | Expr::MultilineString(value, _) => {
            format!("{value:?}")
        }
        Expr::Field { base, name, .. } => format!("{}.{}", call_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", call_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            call_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

pub(super) fn expr_data_effect(expr: &HirExpr) -> Option<&'static str> {
    match expr {
        HirExpr::Effect { effect, .. } => Some(effect.as_str()),
        HirExpr::Call {
            callee:
                Callee::ReceiverCall {
                    effect: Some(DataEffect::Read) | None,
                    ..
                },
            ..
        } => Some(DataEffect::Read.as_str()),
        _ => None,
    }
}

pub(super) fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| builtin_value_type_name(name)),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Char { .. } => Some("Char"),
        HirExpr::ObjectLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::ArrayLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Call {
            callee, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| enum_variant_type_name(callee)),
        HirExpr::Effect {
            value, type_name, ..
        }
        | HirExpr::Manage {
            value, type_name, ..
        }
        | HirExpr::Spawn {
            value, type_name, ..
        }
        | HirExpr::Await {
            value, type_name, ..
        }
        | HirExpr::Try {
            value, type_name, ..
        } => type_name.as_deref().or_else(|| hir_expr_type_name(value)),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. }
        | HirExpr::Index { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn enum_variant_type_name(callee: &Callee) -> Option<&'static str> {
    match callee_name(callee).as_str() {
        "Some" | "None" => Some("Option<?>"),
        "Ok" | "Err" => Some("Result<?>"),
        _ => None,
    }
}

pub(crate) fn argument_type_matches(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    if expected == "Self" {
        return true;
    }
    if strip_fresh_type(expected) == strip_fresh_type(actual) {
        return true;
    }
    if function_type_matches(expected, actual) {
        return true;
    }
    if type_root_name(expected) == type_root_name(actual)
        && let (Some(expected_args), Some(actual_args)) =
            (type_arg_names(expected), type_arg_names(actual))
        && expected_args.len() == actual_args.len()
        && expected_args
            .into_iter()
            .zip(actual_args)
            .all(|(expected, actual)| argument_type_matches(expected.trim(), actual.trim()))
    {
        return true;
    }
    if actual == "Option<?>" {
        return type_root_name(expected) == "Option";
    }
    if actual == "Result<?>" {
        return type_root_name(expected) == "Result";
    }
    false
}

/// Function-type parameter effects are checked at closure/call boundaries.
/// Type identity therefore compares their parameter types after normalizing an
/// omitted effect to the same bare type as an explicit `read`.
pub(super) fn function_type_matches(expected: &str, actual: &str) -> bool {
    if !is_fn_type(expected)
        || !is_fn_type(actual)
        || fn_type_prefix(expected) != fn_type_prefix(actual)
    {
        return false;
    }
    let expected_params = fn_param_types(expected);
    let actual_params = fn_param_types(actual);
    if expected_params.len() != actual_params.len()
        || !expected_params
            .iter()
            .zip(actual_params.iter())
            .all(|(expected, actual)| argument_type_matches(expected, actual))
    {
        return false;
    }
    match (fn_return_type(expected), fn_return_type(actual)) {
        (Some(expected), Some(actual)) => argument_type_matches(expected, actual),
        (None, None) => true,
        _ => false,
    }
}

pub(super) fn json_value_accepts_literal(expected: &str, value: &HirExpr) -> bool {
    if strip_fresh_type(expected) != "JsonValue" {
        return false;
    }
    match value {
        HirExpr::ObjectLiteral { .. } | HirExpr::ArrayLiteral { .. } => true,
        HirExpr::Ident { name, .. } if name == "null" => true,
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            json_value_accepts_literal(expected, value)
        }
        _ => false,
    }
}

pub(super) fn check_map_literal_type(
    analyzer: &mut Analyzer<'_>,
    expected: &str,
    value: &HirExpr,
    context: &str,
) -> bool {
    if type_root_name(strip_fresh_type(expected)) != "Map" {
        return false;
    }
    match value {
        HirExpr::MapLiteral { entries, .. } => {
            let Some(args) = type_arg_names(strip_fresh_type(expected)) else {
                return true;
            };
            let [key_type, value_type] = args.as_slice() else {
                return true;
            };
            for entry in entries {
                check_map_literal_entry_expr(analyzer, key_type, &entry.key, "key", context);
                check_map_literal_entry_expr(analyzer, value_type, &entry.value, "value", context);
            }
            true
        }
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            check_map_literal_type(analyzer, expected, value, context)
        }
        _ => false,
    }
}

pub(super) fn check_map_literal_entry_expr(
    analyzer: &mut Analyzer<'_>,
    expected: &str,
    value: &HirExpr,
    role: &str,
    context: &str,
) {
    if unresolved_generic_type(analyzer, expected) {
        return;
    }
    if json_value_accepts_literal(expected, value) {
        return;
    }
    if check_map_literal_type(analyzer, expected, value, context) {
        return;
    }
    let Some(actual) = hir_expr_type_name(value) else {
        return;
    };
    if unresolved_generic_type(analyzer, actual) || argument_type_matches(expected, actual) {
        return;
    }
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("map literal {role} has type `{actual}`, expected `{expected}`."),
        hir_expr_span(value).clone(),
        "map literal entry type mismatch",
        format!(
            "The {context} is typed as a `Map`, so every map literal {role} must match the corresponding `Map` type argument before Rust lowering."
        ),
        "match_map_literal_entry_type",
        format!("Use a {role} expression of type `{expected}`."),
    ));
}

pub(super) fn check_list_literal_type(
    analyzer: &mut Analyzer<'_>,
    expected: &str,
    value: &HirExpr,
    context: &str,
) -> bool {
    if type_root_name(strip_fresh_type(expected)) != "List" {
        return false;
    }
    match value {
        HirExpr::ArrayLiteral { items, .. } => {
            let Some(args) = type_arg_names(strip_fresh_type(expected)) else {
                return true;
            };
            let [item_type] = args.as_slice() else {
                return true;
            };
            for item in items {
                check_list_literal_item_expr(analyzer, item_type, item, context);
            }
            true
        }
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            check_list_literal_type(analyzer, expected, value, context)
        }
        _ => false,
    }
}

pub(super) fn check_list_literal_item_expr(
    analyzer: &mut Analyzer<'_>,
    expected: &str,
    value: &HirExpr,
    context: &str,
) {
    if unresolved_generic_type(analyzer, expected) {
        return;
    }
    if json_value_accepts_literal(expected, value) {
        return;
    }
    if check_map_literal_type(analyzer, expected, value, context) {
        return;
    }
    if check_list_literal_type(analyzer, expected, value, context) {
        return;
    }
    let Some(actual) = hir_expr_type_name(value) else {
        return;
    };
    if unresolved_generic_type(analyzer, actual) || argument_type_matches(expected, actual) {
        return;
    }
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("list literal item has type `{actual}`, expected `{expected}`."),
        hir_expr_span(value).clone(),
        "list literal item type mismatch",
        format!(
            "The {context} is typed as a `List`, so every array literal item must match the `List` item type before Rust lowering."
        ),
        "match_list_literal_item_type",
        format!("Use a `{expected}` value for this list literal item."),
    ));
}

pub(super) fn is_result_type_name(type_name: &str) -> bool {
    type_root_name(type_name) == "Result"
}

pub(super) fn is_option_type_name(type_name: &str) -> bool {
    type_root_name(type_name) == "Option"
}

pub(super) fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(type_ref_name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

pub(super) fn type_contains_unresolved_generic(type_name: &str, generics: &[String]) -> bool {
    generics.iter().any(|generic| {
        type_name == generic
            || fresh_type_target(type_name)
                .is_some_and(|target| type_contains_unresolved_generic(target, generics))
            || noescape_return_type(type_name)
                .is_some_and(|return_type| type_contains_unresolved_generic(return_type, generics))
            || noescape_param_types(type_name)
                .iter()
                .any(|param_type| type_contains_unresolved_generic(param_type, generics))
            || type_arg_names(type_name).is_some_and(|args| {
                args.iter()
                    .any(|arg| type_contains_unresolved_generic(arg, generics))
            })
    })
}

pub(crate) fn unresolved_generic_type(analyzer: &Analyzer<'_>, type_name: &str) -> bool {
    let root = type_root_name(type_name);
    let declared_type = analyzer
        .syntax_program
        .items
        .iter()
        .chain(
            analyzer
                .interface_programs
                .iter()
                .flat_map(|program| program.items.iter()),
        )
        .any(|item| match item {
            Item::Type(decl) => decl.name == root,
            Item::SumType(decl) => decl.name == root,
            Item::TypeAlias(decl) => decl.name == root,
            _ => false,
        });
    (root.len() == 1 && root.chars().all(|ch| ch.is_ascii_uppercase()) && !declared_type)
        || fresh_type_target(type_name)
            .is_some_and(|target| unresolved_generic_type(analyzer, target))
        || type_arg_names(type_name).is_some_and(|args| {
            args.iter()
                .any(|arg| unresolved_generic_type(analyzer, arg))
        })
        || noescape_return_type(type_name)
            .is_some_and(|return_type| unresolved_generic_type(analyzer, return_type))
        || noescape_param_types(type_name)
            .iter()
            .any(|param_type| unresolved_generic_type(analyzer, param_type))
}

pub(super) fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Char { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

#[cfg(test)]
mod substitution_tests {
    use super::*;
    use crate::checks::budget::{AnalysisBudget, AnalysisBudgetLimits};
    use crate::diagnostic::Span;
    use std::collections::HashMap;

    fn budget() -> std::rc::Rc<AnalysisBudget> {
        AnalysisBudget::new(AnalysisBudgetLimits::default(), Span::default())
    }

    #[test]
    fn substitute_type_params_substitutes_normal_types() {
        let mut subs = HashMap::new();
        subs.insert("T".to_string(), "Int".to_string());
        subs.insert("E".to_string(), "String".to_string());
        let budget = budget();
        assert_eq!(
            substitute_type_params(&budget, "T", &subs),
            Ok("Int".into())
        );
        assert_eq!(
            substitute_type_params(&budget, "List<T>", &subs),
            Ok("List<Int>".into())
        );
        assert_eq!(
            substitute_type_params(&budget, "Result<T, E>", &subs),
            Ok("Result<Int, String>".into())
        );
        assert_eq!(
            substitute_type_params(&budget, "Map<T, List<E>>", &subs),
            Ok("Map<Int, List<String>>".into())
        );
        assert_eq!(
            substitute_type_params(&budget, "fresh T", &subs),
            Ok("fresh Int".into())
        );
    }

    #[test]
    fn deep_substitution_fails_explicitly_and_marks_analysis_incomplete() {
        let mut ty = "T".to_string();
        for _ in 0..300 {
            ty = format!("List<{ty}>");
        }
        let mut subs = HashMap::new();
        subs.insert("T".to_string(), "Int".to_string());
        let budget = budget();

        assert_eq!(substitute_type_params(&budget, &ty, &subs), Err(()));
        let diagnostic = crate::checks::budget::incomplete_diagnostic(&budget)
            .expect("deep substitution should exhaust recursion budget");
        assert_eq!(
            diagnostic.code,
            crate::diagnostic::code::ANALYSIS_INCOMPLETE
        );
        assert!(
            diagnostic
                .causes
                .iter()
                .any(|cause| cause.contains("recursion"))
        );
    }

    #[test]
    fn function_type_matching_normalizes_omitted_read() {
        assert!(argument_type_matches(
            "Fn(Int) -> Int",
            "Fn(read Int) -> Int"
        ));
        assert!(argument_type_matches(
            "List<owned Fn(Int) -> Int>",
            "List<owned Fn(read Int) -> Int>"
        ));
        assert!(!argument_type_matches(
            "Fn(Int) -> Int",
            "Fn(Int) -> String"
        ));
        assert!(!argument_type_matches(
            "noescape Fn(Int) -> Int",
            "Fn(read Int) -> Int"
        ));
    }
}
